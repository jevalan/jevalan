mod common;

use common::{GUARD, ROUTE, TempDir, answer, connect, eventually, next_json, say, scripted_backend, start_with_flows};
use serde_json::{Value, json};

const CYCLE: &str = r#"
start: a
nodes:
  a: { type: noul, instructions: "x", out: { "true": b, "false": b } }
  b: { type: noul, instructions: "x", out: { "true": a, "false": a } }
outputs:
  end: { outcome: o }
"#;

async fn get_flows(daemon: &str) -> Value {
    reqwest::get(format!("{daemon}/v1/flows")).await.unwrap().json().await.unwrap()
}

async fn decide_status(daemon: &str, flow: &str) -> u16 {
    let resp = reqwest::Client::new().post(format!("{daemon}/v1/decide/{flow}")).body(r#"{"state":"x"}"#).send().await;
    resp.unwrap().status().as_u16()
}

#[tokio::test]
async fn ws_replays_loaded_flows_on_connect() {
    let (backend, _script) = scripted_backend().await;
    let dir = TempDir::with(&[("agent-guard.yaml", GUARD), ("model-route.yaml", ROUTE)]);
    let daemon = start_with_flows(&backend, &dir).await;

    let mut ws = connect(&daemon).await;
    let first = next_json(&mut ws).await;
    assert_eq!(first["type"], "BACKENDS");
    assert_eq!(first["payload"]["backends"], json!(["ollama", "openrouter", "typesafe"]));
    assert_eq!(first["seq"], 1);
    assert!(first["trace_id"].as_str().unwrap().starts_with("backends:"));
    assert!(!first.to_string().contains("http"), "ni urls ni claves");
    let (a, b) = (next_json(&mut ws).await, next_json(&mut ws).await);

    assert_eq!((a["type"].as_str(), b["type"].as_str()), (Some("FLOW_LOADED"), Some("FLOW_LOADED")));
    assert_eq!(a["payload"]["definition"]["name"], "agent-guard");
    assert_eq!(b["payload"]["definition"]["name"], "model-route");
    assert_eq!((a["trace_id"].as_str(), a["seq"].as_u64()), (Some("flow:agent-guard:1"), Some(1)));
    assert!(!a.to_string().contains("instructions"));
}

#[tokio::test]
async fn flows_endpoint_lists_valid_and_invalid() {
    let (backend, _script) = scripted_backend().await;
    let dir = TempDir::with(&[("agent-guard.yaml", GUARD), ("cycle.yaml", CYCLE), ("notas.txt", "ignorado")]);
    let daemon = start_with_flows(&backend, &dir).await;

    let listing = get_flows(&daemon).await;

    assert_eq!(listing["flows"].as_object().unwrap().keys().collect::<Vec<_>>(), ["agent-guard"]);
    assert_eq!(listing["flows"]["agent-guard"]["start"], "destructive");
    assert!(listing["invalid"]["cycle"].is_string());
    assert!(listing["invalid"].get("notas").is_none());
}

#[tokio::test]
async fn new_file_is_loaded_while_running() {
    let (backend, _script) = scripted_backend().await;
    let dir = TempDir::with(&[]);
    let daemon = start_with_flows(&backend, &dir).await;
    assert!(get_flows(&daemon).await["flows"].as_object().unwrap().is_empty());

    std::fs::write(dir.0.join("model-route.yaml"), ROUTE).unwrap();

    assert!(eventually(|| async { get_flows(&daemon).await["flows"].get("model-route").is_some() }).await);
}

#[tokio::test]
async fn invalid_reload_keeps_the_previous_version() {
    let (backend, script) = scripted_backend().await;
    let dir = TempDir::with(&[("agent-guard.yaml", GUARD)]);
    let daemon = start_with_flows(&backend, &dir).await;
    say(&script, "destructive", 200, answer("destructive", "noul", json!(0.95), 0.9));
    let mut ws = connect(&daemon).await;
    assert_eq!(next_json(&mut ws).await["type"], "BACKENDS");
    assert_eq!(next_json(&mut ws).await["type"], "FLOW_LOADED");

    std::fs::write(dir.0.join("agent-guard.yaml"), "start: [esto no es un flujo").unwrap();

    loop {
        let event = next_json(&mut ws).await;
        if event["type"] == "FLOW_INVALID" {
            assert_eq!(event["payload"]["flow"], "agent-guard");
            break;
        }
    }
    assert_eq!(decide_status(&daemon, "agent-guard").await, 200);
    assert!(get_flows(&daemon).await["invalid"]["agent-guard"].is_string());
}

#[tokio::test]
async fn deleted_file_stops_serving() {
    let (backend, _script) = scripted_backend().await;
    let dir = TempDir::with(&[("agent-guard.yaml", GUARD)]);
    let daemon = start_with_flows(&backend, &dir).await;
    assert_eq!(decide_status(&daemon, "agent-guard").await, 200);

    std::fs::remove_file(dir.0.join("agent-guard.yaml")).unwrap();

    assert!(eventually(|| async { decide_status(&daemon, "agent-guard").await == 404 }).await);
}
