//! Servidor MCP por stdio: expone las capacidades del hub como herramientas.

mod daemon;
mod protocol;
mod tools;

use protocol::{Daemon, Tool, handle_line, list_changed_notification};
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Cada cuánto se comprueba si las herramientas cambiaron para avisar al cliente.
const POLL: Duration = Duration::from_secs(5);

pub struct McpOptions {
    pub addr: Option<String>,
    pub token: Option<String>,
    pub allow_live: bool,
}

/// Huella de la lista de herramientas: cambia si aparece, desaparece o muda de esquema alguna.
fn signature(tools: &[Tool]) -> u64 {
    let mut h = DefaultHasher::new();
    for t in tools {
        t.name.hash(&mut h);
        t.input_schema.to_string().hash(&mut h);
    }
    h.finish()
}

fn emit(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Entrega cada línea de `reader` a `send` (que devuelve `false` para parar). Tolera bytes que no son UTF-8
/// —se sustituyen— para que una línea rara no mate el servidor, y quita el `\r` de los finales CRLF.
fn forward_lines<R: BufRead>(mut reader: R, mut send: impl FnMut(String) -> bool) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                while matches!(buf.last(), Some(b'\n' | b'\r')) {
                    buf.pop();
                }
                if !send(String::from_utf8_lossy(&buf).into_owned()) {
                    break;
                }
            }
        }
    }
}

/// Envuelve un `Daemon` y recuerda la huella de la última lista de herramientas que entregó, ya sea a un
/// `tools/list` del cliente o a un sondeo: así un cambio posterior a cualquiera de los dos no se pierde.
struct Watching<D> {
    inner: D,
    last: Cell<Option<u64>>,
}

impl<D: Daemon> Watching<D> {
    fn new(inner: D) -> Self {
        Self { inner, last: Cell::new(None) }
    }

    /// `true` si la lista cambió desde la última vez que se miró. La primera vez solo fija la línea base.
    fn poll(&self) -> bool {
        let before = self.last.get();
        let now = signature(&self.list_tools());
        before.is_some_and(|b| b != now)
    }
}

impl<D: Daemon> Daemon for Watching<D> {
    fn list_tools(&self) -> Vec<Tool> {
        let tools = self.inner.list_tools();
        self.last.set(Some(signature(&tools)));
        tools
    }

    fn call(&self, name: &str, args: serde_json::Value) -> Result<protocol::ToolOutcome, protocol::ProtocolError> {
        self.inner.call(name, args)
    }
}

/// Atiende MCP por stdin/stdout hasta que el cliente cierra la entrada. Stdout es solo protocolo.
/// Un único hilo habla con el daemon; otro solo lee líneas y las pasa por un canal.
pub fn run(opts: McpOptions) -> ExitCode {
    let catalog = Watching::new(tools::Catalog::new(daemon::HttpDaemon::new(opts.addr, opts.token), opts.allow_live));
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || forward_lines(std::io::stdin().lock(), |line| tx.send(line).is_ok()));
    // El plazo del sondeo es fijo: con tráfico constante no se retrasa indefinidamente.
    let mut next_poll = Instant::now() + POLL;
    loop {
        match rx.recv_timeout(next_poll.saturating_duration_since(Instant::now())) {
            Ok(line) => {
                if let Some(reply) = handle_line(&catalog, &line) {
                    emit(&reply);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return ExitCode::SUCCESS,
        }
        if Instant::now() >= next_poll {
            if catalog.poll() {
                emit(&list_changed_notification());
            }
            next_poll = Instant::now() + POLL;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::protocol::Tool;
    use super::*;
    use serde_json::json;

    fn tool(name: &str, schema: serde_json::Value) -> Tool {
        Tool { name: name.into(), description: String::new(), input_schema: schema }
    }

    #[test]
    fn signature_changes_when_a_tool_appears_or_its_schema_changes() {
        let a = vec![tool("x", json!({}))];
        assert_eq!(signature(&a), signature(&a.clone()));
        let mut b = a.clone();
        b.push(tool("y", json!({})));
        assert_ne!(signature(&a), signature(&b));
        assert_ne!(signature(&a), signature(&[tool("x", json!({ "type": "object" }))]));
    }

    #[test]
    fn forward_lines_survives_invalid_utf8_and_strips_cr() {
        let input: &[u8] = b"\xff\xfe\n{\"a\":1}\r\nultima";
        let mut got = Vec::new();
        forward_lines(input, |l| {
            got.push(l);
            true
        });
        assert_eq!(got.len(), 3);
        assert!(got[0].contains('\u{FFFD}'));
        assert_eq!(got[1], "{\"a\":1}");
        assert_eq!(got[2], "ultima");
    }

    struct Changing(std::cell::RefCell<Vec<Tool>>);

    impl Daemon for Changing {
        fn list_tools(&self) -> Vec<Tool> {
            self.0.borrow().clone()
        }
        fn call(&self, _: &str, _: serde_json::Value) -> Result<protocol::ToolOutcome, protocol::ProtocolError> {
            unreachable!()
        }
    }

    #[test]
    fn watching_first_poll_is_a_baseline_and_later_changes_are_reported() {
        let w = Watching::new(Changing(std::cell::RefCell::new(vec![tool("x", json!({}))])));
        assert!(!w.poll(), "la primera comprobación solo fija la línea base");
        assert!(!w.poll());
        w.inner.0.borrow_mut().push(tool("y", json!({})));
        assert!(w.poll());
        assert!(!w.poll());
    }

    #[test]
    fn a_tools_list_fixes_the_baseline_so_a_change_right_after_is_not_lost() {
        let w = Watching::new(Changing(std::cell::RefCell::new(vec![tool("x", json!({}))])));
        let _ = w.list_tools(); // lo que haría el cliente con tools/list
        w.inner.0.borrow_mut().push(tool("y", json!({})));
        assert!(w.poll());
    }
}
