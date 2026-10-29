//! Session capture: what the daemon was asked and decided reaches the log, and never the public bus.

mod common;

use common::{GUARD, TempDir, answer, cfg_with, eventually, say, scripted_backend};
use futures_util::StreamExt;
use jevalan_daemon::session::log::{group, read_dir};
use jevalan_daemon::{AppState, router};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const SECRET: &str = "clave-que-no-debe-salir-por-el-bus";
const CLIENT: &str = "crush/crush:243a";

/// Daemon with the guard flow and the session log on (or off) in a temporary directory.
async fn daemon(record: bool) -> (String, common::Scripted, TempDir, PathBuf) {
    let (backend, script) = scripted_backend().await;
    let flows = TempDir::with(&[("agent-guard.yaml", GUARD)]);
    let logs = std::env::temp_dir().join(format!("jevalan-sessions-test-{}", jevalan_daemon::bus::new_trace_id()));
    let mut cfg =
        cfg_with(&[("ollama", &backend, None), ("typesafe", &backend, None), ("openrouter", &backend, None)], None);
    cfg.flows_dir = flows.0.clone();
    cfg.sessions.enabled = record;
    cfg.sessions.dir = Some(logs.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(cfg);
    state.start_recording();
    let watcher = state.load_flows().unwrap();
    tokio::spawn(async move {
        let _watcher = watcher;
        axum::serve(listener, router(state)).await
    });
    (format!("http://{addr}"), script, flows, logs)
}

async fn decide(daemon: &str) -> Value {
    let body = json!({ "state": SECRET, "client": CLIENT });
    let resp = reqwest::Client::new().post(format!("{daemon}/v1/decide/agent-guard")).json(&body).send().await.unwrap();
    resp.json().await.unwrap()
}

#[tokio::test]
async fn a_decision_is_recorded_with_its_input_and_backend_calls_and_the_bus_stays_clean() {
    let (daemon, script, _flows, logs) = daemon(true).await;
    say(&script, "destructive", 200, answer("destructive", "noul", json!(0.1), 0.9));
    say(&script, "kind", 200, answer("kind", "choice", json!("code-edit"), 0.9));
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.trim_start_matches("http://"))).await.unwrap();

    let reply = decide(&daemon).await;
    assert_eq!(reply["outcome"], "allow");

    // D2: nothing the client sent travels on the public bus.
    let mut seen = String::new();
    loop {
        let Message::Text(text) = ws.next().await.unwrap().unwrap() else { panic!("se esperaba texto") };
        seen.push_str(&text);
        if text.contains("DECISION_MADE") {
            break;
        }
    }
    assert!(!seen.contains(SECRET), "el bus público llevó la entrada: {seen}");

    // The log gets it all: the input in clear, two backend calls, the decision and the events.
    assert!(eventually(|| async { read_dir(&logs).iter().any(|l| l["type"] == "DECISION") }).await);
    assert!(eventually(|| async { read_dir(&logs).iter().filter(|l| l["type"] == "EVENT").count() >= 8 }).await);
    let sessions = group(read_dir(&logs));
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].client, CLIENT);
    let trace = &sessions[0].traces[0];
    assert_eq!(trace.first("INPUT").unwrap()["input"], SECRET);
    assert_eq!(trace.all("BACKEND_CALL").count(), 2);
    let call = trace.first("BACKEND_CALL").unwrap();
    assert_eq!(call["response"]["status"], 200);
    assert!(call["request"]["questions"].is_object(), "{call}");
    assert_eq!(trace.first("DECISION").unwrap()["decision"]["outcome"], "allow");
    assert_eq!(trace.trace_id, reply["trace_id"]);
    let _ = std::fs::remove_dir_all(logs);
}

#[tokio::test]
async fn a_dead_backend_is_recorded_as_a_failed_call() {
    let (daemon, _script, _flows, logs) = daemon(true).await;

    // Nothing is scripted, so the backend answers 500: the call is in the log with that status.
    decide(&daemon).await;

    assert!(eventually(|| async { read_dir(&logs).iter().any(|l| l["type"] == "DECISION") }).await);
    let lines = read_dir(&logs);
    let call = lines.iter().find(|l| l["type"] == "BACKEND_CALL").expect("sin llamada registrada");
    assert_eq!(call["response"]["status"], 500);
    let _ = std::fs::remove_dir_all(logs);
}

#[tokio::test]
async fn with_capture_off_nothing_is_written() {
    let (daemon, script, _flows, logs) = daemon(false).await;
    say(&script, "destructive", 200, answer("destructive", "noul", json!(0.1), 0.9));
    say(&script, "kind", 200, answer("kind", "choice", json!("code-edit"), 0.9));

    decide(&daemon).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    assert!(!logs.exists() || read_dir(&logs).is_empty());
}
