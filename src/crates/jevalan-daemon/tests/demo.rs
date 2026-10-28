mod common;

use common::{GUARD, ROUTE, TempDir, eventually};
use futures_util::StreamExt;
use jevalan_daemon::mock::spawn_mock;
use jevalan_daemon::simulate::spawn_simulator;
use jevalan_daemon::{AppState, router};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Daemon con los dos flujos de ejemplo contra dos mocks fiables; `typesafe` lleva clave.
async fn demo_daemon() -> (String, TempDir, Arc<AtomicBool>) {
    let dir = TempDir::with(&[("agent-guard.yaml", GUARD), ("model-route.yaml", ROUTE)]);
    let typesafe = spawn_mock(1, (0, 5), 0.0).await.unwrap().url;
    let ollama_mock = spawn_mock(2, (0, 5), 0.0).await.unwrap();
    let ollama = ollama_mock.url;
    let mut cfg = common::cfg_with(&[("ollama", &ollama, None), ("typesafe", &typesafe, Some("dummy"))], None);
    cfg.flows_dir = dir.0.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(cfg);
    let watcher = state.load_flows().unwrap();
    tokio::spawn(async move {
        let _watcher = watcher;
        axum::serve(listener, router(state)).await
    });
    (format!("http://{addr}"), dir, ollama_mock.down)
}

#[tokio::test]
async fn agent_guard_runs_end_to_end_against_the_mocks() {
    let (daemon, _dir, _down) = demo_daemon().await;
    for _ in 0..10 {
        let resp = reqwest::Client::new()
            .post(format!("{daemon}/v1/decide/agent-guard"))
            .json(&json!({ "state": "ls -la" }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let reply: Value = resp.json().await.unwrap();
        let ports: Vec<&str> = reply["path"].as_array().unwrap().iter().map(|s| s["port"].as_str().unwrap()).collect();
        assert!(!ports.contains(&"error"), "{reply}");
    }
}

#[tokio::test]
async fn simulator_produces_decisions_and_passthroughs() {
    let (daemon, _dir, _down) = demo_daemon().await;
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.trim_start_matches("http://"))).await.unwrap();
    let seen: Arc<Mutex<BTreeSet<String>>> = Arc::default();
    let sink = seen.clone();
    tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let event: Value = serde_json::from_str(&text).unwrap();
            sink.lock().unwrap().insert(event["type"].as_str().unwrap_or_default().to_string());
        }
    });

    let sim = spawn_simulator(daemon, None, 100.0, 5);

    let done = eventually(|| async {
        let seen = seen.lock().unwrap();
        seen.contains("DECISION_MADE") && seen.contains("PASSTHROUGH_DONE")
    })
    .await;
    sim.abort();
    assert!(done, "{:?}", seen.lock().unwrap());
}

#[tokio::test]
async fn a_downed_backend_sends_the_flow_through_error() {
    let (daemon, _dir, down) = demo_daemon().await;
    down.store(true, Ordering::Relaxed);
    let resp = reqwest::Client::new()
        .post(format!("{daemon}/v1/decide/model-route"))
        .json(&json!({ "state": "refactor" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let reply: Value = resp.json().await.unwrap();
    let ports: Vec<&str> = reply["path"].as_array().unwrap().iter().map(|s| s["port"].as_str().unwrap()).collect();
    assert!(ports.contains(&"error"), "{reply}");
}

/// Daemon de demo con el binario `fake-specialist` en lugar de `jevalan mock-specialist`.
async fn demo_hub_daemon() -> String {
    let specialist = vec![env!("CARGO_BIN_EXE_fake-specialist").to_string()];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (mut cfg, _switches) = jevalan_daemon::mock::demo_config(addr, 7, specialist).await.unwrap();
    cfg.flows_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../config/flows");
    cfg.hub.signals_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../config/signals");
    let state = AppState::new(cfg);
    let (flows, caps) = (state.load_flows().unwrap(), state.load_capabilities().unwrap());
    tokio::spawn(async move {
        let _watchers = (flows, caps);
        axum::serve(listener, router(state)).await
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn demo_serves_both_capabilities() {
    let daemon = demo_hub_daemon().await;
    for (capability, input) in [
        ("agent.wakeup", json!({ "waiting_for": "x", "from_user": true, "skipped": 0 })),
        ("tool.risk", json!({ "tool_name": "Read", "tool_input": {} })),
    ] {
        let body = json!({
            "protocol_version": "jev-hub/0.1", "request_id": "r", "capability": capability,
            "capability_version": "1", "input": input,
        });
        let resp = reqwest::Client::new().post(format!("{daemon}/v1/hub")).json(&body).send().await.unwrap();
        let reply: Value = resp.json().await.unwrap();
        let status = reply["status"].as_str().unwrap_or_default();
        assert!(["ok", "abstain"].contains(&status), "{capability}: {reply}");
        assert_ne!(reply["error"]["code"], "provider_disabled", "{reply}");
    }
}

/// Sin `from_user`, `agent.wakeup` pregunta al backend `openrouter` de la demo (su mock) y decide, no falla.
#[tokio::test]
async fn demo_wakeup_asks_its_openrouter_mock() {
    let daemon = demo_hub_daemon().await;
    let body = json!({
        "protocol_version": "jev-hub/0.1", "request_id": "r", "capability": "agent.wakeup",
        "capability_version": "1", "input": { "waiting_for": "CI", "event": "build failed", "skipped": 0 },
    });
    let resp = reqwest::Client::new().post(format!("{daemon}/v1/hub")).json(&body).send().await.unwrap();
    let reply: Value = resp.json().await.unwrap();
    assert_eq!(reply["status"], "ok", "{reply}");
    let path = reply["trace"]["path"].as_array().unwrap();
    assert!(path.iter().any(|n| n["node"] == "worth" && n["backend"] == "openrouter"), "{reply}");
}

#[tokio::test]
async fn simulator_reaches_the_hub() {
    let daemon = demo_hub_daemon().await;
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.trim_start_matches("http://"))).await.unwrap();
    let seen = Arc::new(Mutex::new(BTreeSet::<String>::new()));
    let sink = seen.clone();
    tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let event: Value = serde_json::from_str(&text).unwrap();
            sink.lock().unwrap().insert(event["type"].as_str().unwrap_or_default().to_string());
        }
    });
    let sim = spawn_simulator(daemon, None, 100.0, 3);
    let done = eventually(|| async { seen.lock().unwrap().contains("HUB_RESPONSE") }).await;
    sim.abort();
    assert!(done, "{:?}", seen.lock().unwrap());
}
