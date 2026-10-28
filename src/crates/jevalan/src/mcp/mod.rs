//! Servidor MCP por stdio: expone las capacidades del hub como herramientas.

mod daemon;
mod protocol;
mod tools;

use protocol::{Daemon, Tool, handle_line, list_changed_notification};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

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

/// Atiende MCP por stdin/stdout hasta que el cliente cierra la entrada. Stdout es solo protocolo.
/// Un único hilo habla con el daemon; otro solo lee líneas y las pasa por un canal.
pub fn run(opts: McpOptions) -> ExitCode {
    let catalog = tools::Catalog::new(daemon::HttpDaemon::new(opts.addr, opts.token), opts.allow_live);
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut seen: Option<u64> = None;
    loop {
        match rx.recv_timeout(POLL) {
            Ok(line) => {
                if let Some(reply) = handle_line(&catalog, &line) {
                    emit(&reply);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let now = signature(&catalog.list_tools());
                if seen.is_some_and(|before| before != now) {
                    emit(&list_changed_notification());
                }
                seen = Some(now);
            }
            Err(RecvTimeoutError::Disconnected) => return ExitCode::SUCCESS,
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
}
