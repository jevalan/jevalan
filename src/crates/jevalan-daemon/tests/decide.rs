mod common;

use common::{GUARD, Scripted, TempDir, answer, say, scripted_backend, start_with_flows};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const STATE: &str = "secreto-del-usuario";

async fn decide(daemon: &str, flow: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new().post(format!("{daemon}/v1/decide/{flow}")).json(&body).send().await.unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

async fn guard_daemon() -> (String, Scripted, TempDir) {
    let (backend, script) = scripted_backend().await;
    let dir = TempDir::with(&[("agent-guard.yaml", GUARD)]);
    (start_with_flows(&backend, &dir).await, script, dir)
}

fn ports(reply: &Value) -> Vec<&str> {
    reply["path"].as_array().unwrap().iter().map(|s| s["port"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn decide_follows_the_flow_and_returns_the_path() {
    let (daemon, script, _dir) = guard_daemon().await;
    say(&script, "destructive", 200, answer("destructive", "noul", json!(0.1), 0.9));
    say(&script, "kind", 200, answer("kind", "choice", json!("code-edit"), 0.9));

    let (status, reply) = decide(&daemon, "agent-guard", json!({ "state": STATE })).await;

    assert_eq!(status, 200);
    assert_eq!(reply["outcome"], "allow");
    assert_eq!(ports(&reply), ["false", "code-edit"]);
    assert_eq!(reply["flow"], "agent-guard");
    assert!(reply["trace_id"].is_string());
}

#[tokio::test]
async fn outcome_data_is_returned() {
    let (daemon, script, _dir) = guard_daemon().await;
    say(&script, "destructive", 200, answer("destructive", "noul", json!(0.95), 0.9));

    let (_, reply) = decide(&daemon, "agent-guard", json!({ "state": STATE })).await;

    assert_eq!((reply["outcome"].as_str(), reply["data"]["reason"].as_str()), (Some("deny"), Some("destructivo")));
}

#[tokio::test]
async fn backend_failure_takes_the_error_port() {
    let (daemon, _script, _dir) = guard_daemon().await;

    let (status, reply) = decide(&daemon, "agent-guard", json!({ "state": STATE })).await;

    assert_eq!(status, 200);
    assert_eq!(reply["outcome"], "abstain");
    assert_eq!(reply["path"][0]["port"], "error");
}

#[tokio::test]
async fn malformed_backend_answer_takes_the_error_port() {
    let (daemon, script, _dir) = guard_daemon().await;
    say(&script, "destructive", 200, r#"{"answers":{}}"#.into());

    let (status, reply) = decide(&daemon, "agent-guard", json!({ "state": STATE })).await;

    assert_eq!((status, reply["outcome"].as_str()), (200, Some("abstain")));
}

#[tokio::test]
async fn failure_without_error_port_is_502_with_partial_path() {
    let (daemon, script, _dir) = guard_daemon().await;
    say(&script, "destructive", 200, answer("destructive", "noul", json!(0.1), 0.9));

    let (status, reply) = decide(&daemon, "agent-guard", json!({ "state": STATE })).await;

    assert_eq!(status, 502);
    assert!(reply["error"].as_str().unwrap().starts_with("kind: "));
    assert_eq!(reply["path"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn unknown_flow_is_404() {
    let (daemon, _script, _dir) = guard_daemon().await;
    let (status, reply) = decide(&daemon, "nope", json!({ "state": STATE })).await;
    assert_eq!(status, 404);
    assert!(reply["error"].is_string());
}

#[tokio::test]
async fn missing_state_is_400() {
    let (daemon, _script, _dir) = guard_daemon().await;
    let (status, _) = decide(&daemon, "agent-guard", json!({ "client": "x" })).await;
    assert_eq!(status, 400);

    let resp = reqwest::Client::new().post(format!("{daemon}/v1/decide/agent-guard")).body("no es json").send().await;
    assert_eq!(resp.unwrap().status(), 400);
}

#[tokio::test]
async fn decide_emits_node_events() {
    let (daemon, script, _dir) = guard_daemon().await;
    say(&script, "destructive", 200, answer("destructive", "noul", json!(0.1), 0.9));
    say(&script, "kind", 200, answer("kind", "choice", json!("code-edit"), 0.9));
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.trim_start_matches("http://"))).await.unwrap();

    decide(&daemon, "agent-guard", json!({ "state": STATE, "client": "crush" })).await;

    let mut types = Vec::new();
    let mut seen = Vec::new();
    while types.last().map(String::as_str) != Some("DECISION_MADE") {
        let Message::Text(text) = ws.next().await.unwrap().unwrap() else { panic!("se esperaba texto") };
        assert!(!text.contains(STATE) && !text.contains("borra o sobrescribe"), "{text}");
        let event: Value = serde_json::from_str(&text).unwrap();
        types.push(event["type"].as_str().unwrap().to_string());
        seen.push(event);
    }

    assert_eq!(types.remove(0), "BACKENDS");
    seen.remove(0);
    assert_eq!(types[0], "FLOW_LOADED");
    let trace = &types[1..];
    let node = ["NODE_START", "BACKEND_REQUEST", "BACKEND_RESPONSE", "NODE_END"];
    let expected: Vec<&str> =
        ["REQUEST_RECEIVED"].into_iter().chain(node).chain(node).chain(["DECISION_MADE"]).collect();
    assert_eq!(trace, expected);
    assert_eq!(seen[1]["payload"]["client"], "crush");
    assert_eq!(seen[1]["payload"]["endpoint"], "decide");
    assert_eq!(seen[2]["payload"]["type"], "noul");
    let seqs: Vec<_> = seen[1..].iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, (1..=10).collect::<Vec<_>>());
}

#[tokio::test]
async fn client_trace_id_is_echoed_not_used() {
    let (daemon, script, _dir) = guard_daemon().await;
    let mut ids = Vec::new();
    for _ in 0..2 {
        say(&script, "destructive", 200, answer("destructive", "noul", json!(0.95), 0.9));
        let body = json!({ "state": STATE, "client_trace_id": "abc", "trace_id": "abc" });
        let (status, reply) = decide(&daemon, "agent-guard", body).await;
        assert_eq!((status, reply["client_trace_id"].as_str()), (200, Some("abc")));
        ids.push(reply["trace_id"].as_str().unwrap().to_string());
    }
    assert_ne!(ids[0], ids[1]);
    assert!(ids.iter().all(|id| id != "abc"));
}

#[tokio::test]
async fn long_client_trace_id_is_400() {
    let (daemon, _script, _dir) = guard_daemon().await;
    for bad in [json!("x".repeat(129)), json!(7)] {
        let (status, reply) = decide(&daemon, "agent-guard", json!({ "state": STATE, "client_trace_id": bad })).await;
        assert_eq!(status, 400);
        assert!(reply["error"].is_string());
    }
}

const BAD_RULE: &str = r#"
defaults: { backend: ollama, model: nimble }
start: r
nodes:
  r: { type: rule, when: "input.n > 1", out: { "true": done, "false": done } }
outputs:
  done: { outcome: ok }
"#;

#[tokio::test]
async fn rule_failure_events_do_not_leak_input() {
    let (backend, _script) = scripted_backend().await;
    let dir = TempDir::with(&[("bad-rule.yaml", BAD_RULE)]);
    let daemon = start_with_flows(&backend, &dir).await;
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.trim_start_matches("http://"))).await.unwrap();
    let marker = "MARCADOR-SECRETO-77c1";

    let (status, reply) = decide(&daemon, "bad-rule", json!({ "state": { "n": marker } })).await;

    assert_eq!(status, 502, "{reply}");
    let mut lines = Vec::new();
    let error = loop {
        let next =
            tokio::time::timeout(std::time::Duration::from_secs(3), ws.next()).await.expect("sin ERROR a tiempo");
        let Message::Text(text) = next.unwrap().unwrap() else { continue };
        lines.push(text.to_string());
        let event: Value = serde_json::from_str(&text).unwrap();
        if event["type"] == "ERROR" {
            break event;
        }
    };
    assert_eq!(error["payload"]["stage"], "rule", "{error}");
    assert_eq!(error["payload"]["message"], "la regla r no pudo evaluarse");
    assert!(lines.iter().all(|l| !l.contains(marker)), "{lines:?}");
}
