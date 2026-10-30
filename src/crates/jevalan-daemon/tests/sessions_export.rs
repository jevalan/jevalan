//! The OTLP mirror of the session log, against a collector that is only a listener.

mod common;

use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use common::{TempDir, eventually};
use jevalan_daemon::config::Otlp;
use jevalan_daemon::session::Recorder;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Received = Arc<Mutex<Vec<(Option<String>, Value)>>>;

/// A collector that keeps what it is sent, and the `authorization` header it came with.
async fn collector(status: u16) -> (String, Received) {
    let received: Received = Arc::default();
    let handler = move |State(got): State<Received>, headers: HeaderMap, axum::Json(body): axum::Json<Value>| async move {
        let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_string);
        got.lock().unwrap().push((auth, body));
        axum::http::StatusCode::from_u16(status).unwrap()
    };
    let app = Router::new().route("/v1/traces", post(handler)).with_state(received.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/traces", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (url, received)
}

fn recorder(dir: &TempDir, endpoint: String, content: bool) -> Recorder {
    let recorder = Recorder::open(dir.0.clone(), 14);
    let headers = vec![("authorization".to_string(), "Basic clave".to_string())];
    recorder.start_export(Otlp { endpoint, content, headers });
    recorder
}

fn one_trace(recorder: &Recorder) {
    recorder.input("t1", "crush/demo:1", "tool.risk", &json!({ "command": "SECRETO" }));
    recorder.decision("t1", "tool.risk", "ok", Some(&json!({ "verdict": "allow" })), None);
}

#[tokio::test]
async fn a_finished_trace_reaches_the_collector_with_its_headers_and_without_the_content() {
    let (endpoint, received) = collector(200).await;
    let dir = TempDir::with(&[]);
    let recorder = recorder(&dir, endpoint, false);

    one_trace(&recorder);

    assert!(eventually(|| async { !received.lock().unwrap().is_empty() }).await, "no llegó nada");
    let (auth, body) = received.lock().unwrap()[0].clone();
    assert_eq!(auth.as_deref(), Some("Basic clave"));
    let text = body.to_string();
    assert!(text.contains("invoke_workflow tool.risk") && text.contains("crush/demo:1"), "{text}");
    assert!(!text.contains("SECRETO"), "el contenido solo viaja si se pide: {text}");
    assert_eq!(recorder.export_failures(), 0);
}

#[tokio::test]
async fn the_content_travels_when_asked_for() {
    let (endpoint, received) = collector(200).await;
    let dir = TempDir::with(&[]);
    let recorder = recorder(&dir, endpoint, true);

    one_trace(&recorder);

    assert!(eventually(|| async { !received.lock().unwrap().is_empty() }).await);
    assert!(received.lock().unwrap()[0].1.to_string().contains("SECRETO"));
}

#[tokio::test]
async fn a_collector_that_refuses_costs_the_mirror_and_never_the_log() {
    let (endpoint, received) = collector(503).await;
    let dir = TempDir::with(&[]);
    let recorder = recorder(&dir, endpoint, false);

    one_trace(&recorder);

    assert!(eventually(|| async { recorder.export_failures() == 1 }).await, "se esperaba un fallo contado");
    assert_eq!(received.lock().unwrap().len(), 1);
    assert_eq!(recorder.failures(), 0);
    let log = jevalan_daemon::session::files(&dir.0);
    assert_eq!(log.len(), 1, "el JSONL se escribió igualmente");
    assert!(std::fs::read_to_string(&log[0]).unwrap().contains("DECISION"));
}

#[tokio::test]
async fn a_collector_that_is_down_is_counted_not_raised() {
    let dir = TempDir::with(&[]);
    let recorder = recorder(&dir, "http://127.0.0.1:1/v1/traces".into(), false);

    one_trace(&recorder);

    assert!(eventually(|| async { recorder.export_failures() == 1 }).await);
    assert_eq!(recorder.failures(), 0);
}
