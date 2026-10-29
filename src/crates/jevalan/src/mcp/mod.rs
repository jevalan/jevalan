//! MCP server over stdio: exposes the hub's capabilities as tools.

mod daemon;
mod protocol;
mod tools;

use protocol::{Daemon, Tool, handle_line, list_changed_notification};
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, Read, Write};
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How often to check whether the tools changed, in order to notify the client.
const POLL: Duration = Duration::from_secs(5);

pub struct McpOptions {
    pub addr: Option<String>,
    pub token: Option<String>,
    pub allow_live: bool,
}

/// Fingerprint of the tool list: changes if any tool appears, disappears or changes schema.
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

/// Hands each line of `reader` to `send` (which returns `false` to stop). Tolerates bytes that are not UTF-8
/// (they are replaced) so that an odd line does not kill the server, and strips the `\r` of CRLF endings.
const MAX_LINE: usize = 4 * 1024 * 1024;

fn forward_lines<R: BufRead>(reader: R, send: impl FnMut(String) -> bool) {
    forward_lines_capped(reader, MAX_LINE, send);
}

/// Like `forward_lines`, but a line longer than `max` bytes is cut at `max` and the rest of it is discarded
/// without being stored, so a runaway line cannot exhaust memory. The cut line fails to parse as JSON and the
/// client gets a parse error instead of silence.
fn forward_lines_capped<R: BufRead>(mut reader: R, max: usize, mut send: impl FnMut(String) -> bool) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match (&mut reader).take(max as u64 + 1).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.len() > max && buf.last() != Some(&b'\n') {
            buf.truncate(max);
            let mut scratch = Vec::new();
            loop {
                scratch.clear();
                match (&mut reader).take(64 * 1024).read_until(b'\n', &mut scratch) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if scratch.last() == Some(&b'\n') => break,
                    Ok(_) => {}
                }
            }
        }
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        if !send(String::from_utf8_lossy(&buf).into_owned()) {
            break;
        }
    }
}

/// Wraps a `Daemon` and remembers the fingerprint of the last tool list it delivered, whether to a
/// `tools/list` from the client or to a poll: this way a later change is not lost to either of them.
struct Watching<D> {
    inner: D,
    last: Cell<Option<u64>>,
}

impl<D: Daemon> Watching<D> {
    fn new(inner: D) -> Self {
        Self { inner, last: Cell::new(None) }
    }

    /// `true` if the list changed since the last time it was looked at. The first time it only sets the baseline.
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

/// Serves MCP over stdin/stdout until the client closes the input. Stdout is protocol only.
/// A single thread talks to the daemon; another only reads lines and passes them through a channel.
pub fn run(opts: McpOptions) -> ExitCode {
    let catalog = Watching::new(tools::Catalog::new(daemon::HttpDaemon::new(opts.addr, opts.token), opts.allow_live));
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || forward_lines(std::io::stdin().lock(), |line| tx.send(line).is_ok()));
    // The poll deadline is fixed: with constant traffic it is not delayed indefinitely.
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
        let _ = w.list_tools(); // what the client would do with tools/list
        w.inner.0.borrow_mut().push(tool("y", json!({})));
        assert!(w.poll());
    }

    #[test]
    fn forward_lines_truncates_a_giant_line_and_keeps_going() {
        let input: &[u8] = b"aaaaaaaaaa\nok\n";
        let mut got = Vec::new();
        forward_lines_capped(input, 4, |l| {
            got.push(l);
            true
        });
        assert_eq!(got, vec!["aaaa".to_string(), "ok".to_string()]);
    }
}
