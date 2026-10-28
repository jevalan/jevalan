//! Fuente en vivo: se conecta por WebSocket al daemon y reintenta con paciencia.

use crate::screen::{Feed, SourceStatus};
use jevalan_events::Event;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::{Message, connect};

const BACKOFF_START: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// Lanza el hilo de conexión; termina cuando se suelta el `Receiver`.
pub fn spawn(url: String, token: Option<String>) -> Receiver<Feed> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || run(&url, token.as_deref(), &tx));
    rx
}

fn run(url: &str, token: Option<&str>, tx: &Sender<Feed>) {
    let mut backoff = BACKOFF_START;
    loop {
        if tx.send(Feed::Status(SourceStatus::Connecting)).is_err() {
            return;
        }
        let reason = match session(url, token, tx, &mut backoff) {
            Ok(reason) => reason,
            Err(Closed) => return,
        };
        if tx.send(Feed::Status(SourceStatus::Disconnected(reason))).is_err() {
            return;
        }
        thread::sleep(backoff);
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// La TUI cerró el canal.
struct Closed;

/// Una conexión entera; devuelve el motivo por el que terminó.
fn session(url: &str, token: Option<&str>, tx: &Sender<Feed>, backoff: &mut Duration) -> Result<String, Closed> {
    let mut request = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => return Ok(e.to_string()),
    };
    if let Some(value) = token.and_then(|t| HeaderValue::from_str(&format!("Bearer {t}")).ok()) {
        request.headers_mut().insert("Authorization", value);
    }
    let (mut socket, _) = match connect(request) {
        Ok(ok) => ok,
        Err(e) => return Ok(e.to_string()),
    };
    tx.send(Feed::Status(SourceStatus::Connected)).map_err(|_| Closed)?;
    *backoff = BACKOFF_START;
    let mut skipped = 0u32;
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => match serde_json::from_str::<Event>(&text) {
                Ok(e) => tx.send(Feed::Event(e)).map_err(|_| Closed)?,
                Err(_) => skipped += 1,
            },
            Ok(Message::Close(_)) => return Ok(with_skipped("cerrada por el servidor", skipped)),
            Ok(_) => {}
            Err(e) => return Ok(with_skipped(&e.to_string(), skipped)),
        }
    }
}

fn with_skipped(reason: &str, skipped: u32) -> String {
    if skipped == 0 { reason.to_string() } else { format!("{reason}; {skipped} mensajes ilegibles") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevalan_events::{EventKind, new_event};
    use std::net::TcpListener;
    use tungstenite::accept_hdr;
    use tungstenite::handshake::server::{ErrorResponse, Request, Response};

    fn event(trace: &str) -> Message {
        let e = new_event(trace, 1, "test", EventKind::FlowRemoved { flow: "f".into() });
        Message::text(serde_json::to_string(&e).unwrap())
    }

    /// Servidor de prueba: atiende una conexión por guion, exigiendo `Bearer tok`, y la cierra.
    fn server(scripts: Vec<Vec<Message>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/events", listener.local_addr().unwrap());
        thread::spawn(move || {
            for script in scripts {
                let (stream, _) = listener.accept().unwrap();
                let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                    match req.headers().get("Authorization").and_then(|v| v.to_str().ok()) {
                        Some("Bearer tok") => Ok(resp),
                        _ => Err(ErrorResponse::new(Some("sin token".into()))),
                    }
                };
                let Ok(mut ws) = accept_hdr(stream, check) else { continue };
                for m in script {
                    ws.send(m).unwrap();
                }
                let _ = ws.close(None);
                let _ = ws.flush();
            }
        });
        url
    }

    fn next(rx: &Receiver<Feed>) -> Feed {
        rx.recv_timeout(Duration::from_secs(5)).expect("la fuente emite")
    }

    #[test]
    fn receives_status_then_events() {
        let rx = spawn(server(vec![vec![event("a"), event("b")]]), Some("tok".into()));
        assert_eq!(next(&rx), Feed::Status(SourceStatus::Connecting));
        assert_eq!(next(&rx), Feed::Status(SourceStatus::Connected));
        for trace in ["a", "b"] {
            assert!(matches!(next(&rx), Feed::Event(e) if e.trace_id == trace));
        }
        assert!(matches!(next(&rx), Feed::Status(SourceStatus::Disconnected(_))));
    }

    #[test]
    fn reconnects_after_a_drop() {
        let rx = spawn(server(vec![vec![], vec![event("a")]]), Some("tok".into()));
        loop {
            if matches!(next(&rx), Feed::Status(SourceStatus::Disconnected(_))) {
                break;
            }
        }
        assert_eq!(next(&rx), Feed::Status(SourceStatus::Connecting));
        assert_eq!(next(&rx), Feed::Status(SourceStatus::Connected));
        assert!(matches!(next(&rx), Feed::Event(_)));
    }

    #[test]
    fn bad_json_is_skipped() {
        let rx = spawn(server(vec![vec![Message::text("basura"), event("ok")]]), Some("tok".into()));
        let got: Vec<Feed> = std::iter::from_fn(|| Some(next(&rx))).take(4).collect();
        let events: Vec<_> = got.iter().filter(|f| matches!(f, Feed::Event(_))).collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Feed::Event(e) if e.trace_id == "ok"));
    }

    #[test]
    fn a_missing_token_is_refused() {
        let rx = spawn(server(vec![vec![event("a")]]), None);
        assert_eq!(next(&rx), Feed::Status(SourceStatus::Connecting));
        assert!(matches!(next(&rx), Feed::Status(SourceStatus::Disconnected(_))));
    }
}
