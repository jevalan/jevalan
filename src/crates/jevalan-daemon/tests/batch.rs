//! The `batch` node: several System One questions in a single call to the backend.

mod common;

use common::{BATCH, Scripted, TempDir, answers, say, scripted_backend, start_with_flows};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

async fn batch_daemon() -> (String, Scripted, TempDir) {
    let (backend, script) = scripted_backend().await;
    let dir = TempDir::with(&[("tool-risk.yaml", BATCH)]);
    (start_with_flows(&backend, &dir).await, script, dir)
}

async fn decide(daemon: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new().post(format!("{daemon}/v1/decide/tool-risk")).json(&body).send().await.unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

/// The request the backend of the test double received, or `None` if none arrived.
fn first_request(script: &Scripted) -> Option<Value> {
    let captured = script.log.lock().unwrap();
    captured.first().map(|c| serde_json::from_slice(&c.body).unwrap())
}

#[tokio::test]
async fn batch_makes_one_backend_call() {
    let (daemon, script, _dir) = batch_daemon().await;
    say(
        &script,
        "risks",
        200,
        answers(&[
            ("risks.destructive", "noul", json!(0.1), 0.9),
            ("risks.exfiltration", "noul", json!(0.1), 0.9),
            ("risks.privilege", "noul", json!(0.1), 0.9),
            ("risks.secret_exposure", "noul", json!(0.95), 0.9),
        ]),
    );

    let (status, reply) = decide(&daemon, json!({ "state": "x" })).await;

    assert_eq!(status, 200, "{reply}");
    // A single step for the node, with the whole response as an object (D2).
    assert_eq!(reply["outcome"], "deny");
    let path = reply["path"].as_array().unwrap();
    assert_eq!(path.len(), 1, "{reply}");
    assert_eq!(path[0]["node"], "risks");
    assert_eq!(path[0]["port"], "deny");
    assert_eq!(path[0]["answer"]["secret_exposure"], 0.95);
    assert_eq!(reply["data"]["probabilities"]["destructive"], 0.1);
}

#[tokio::test]
async fn batch_sends_every_question_in_one_request() {
    let (daemon, script, _dir) = batch_daemon().await;
    say(&script, "risks", 200, answers(&[("risks.destructive", "noul", json!(0.1), 0.9)]));

    decide(&daemon, json!({ "state": "x" })).await;

    let body = first_request(&script).expect("el backend debía recibir una petición");
    let questions = body["questions"].as_object().unwrap();
    assert_eq!(questions.len(), 4, "{body}");
    for (key, ty) in [
        ("risks.destructive", "noul"),
        ("risks.exfiltration", "noul"),
        ("risks.privilege", "noul"),
        ("risks.secret_exposure", "noul"),
    ] {
        assert_eq!(questions[key]["type"], ty, "{body}");
        assert!(questions[key]["instructions"].is_string(), "{body}");
    }
    assert_eq!(body["model"], "nimble");
    assert_eq!(body["state"], "x");
}

#[tokio::test]
async fn batch_response_missing_a_question_is_an_error() {
    let (daemon, script, _dir) = batch_daemon().await;
    // Only one of the four: no decision is made with what arrives.
    say(&script, "risks", 200, answers(&[("risks.destructive", "noul", json!(0.99), 0.9)]));

    let (status, reply) = decide(&daemon, json!({ "state": "x" })).await;

    assert_eq!((status, reply["outcome"].as_str()), (200, Some("abstain")));
    assert_eq!(reply["path"][0]["port"], "error");
}

#[tokio::test]
async fn batch_events_keep_one_request_and_one_node_end() {
    let (daemon, script, _dir) = batch_daemon().await;
    say(
        &script,
        "risks",
        200,
        answers(&[
            ("risks.destructive", "noul", json!(0.1), 0.9),
            ("risks.exfiltration", "noul", json!(0.1), 0.8),
            ("risks.privilege", "noul", json!(0.1), 0.8),
            ("risks.secret_exposure", "noul", json!(0.1), 0.8),
        ]),
    );
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.trim_start_matches("http://"))).await.unwrap();

    decide(&daemon, json!({ "state": "x" })).await;

    let mut seen = Vec::new();
    while seen.last().and_then(|e: &Value| e["type"].as_str()) != Some("DECISION_MADE") {
        let Message::Text(text) = ws.next().await.unwrap().unwrap() else { panic!("se esperaba texto") };
        seen.push(serde_json::from_str::<Value>(&text).unwrap());
    }
    let trace: Vec<&Value> = seen[2..].iter().collect();
    let types: Vec<&str> = trace.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(
        types,
        ["REQUEST_RECEIVED", "NODE_START", "BACKEND_REQUEST", "BACKEND_RESPONSE", "NODE_END", "DECISION_MADE"]
    );

    let request = trace.iter().find(|e| e["type"] == "BACKEND_REQUEST").unwrap();
    assert_eq!(request["payload"]["questions"], 4, "el evento solo lleva el contador");
    let start = trace.iter().find(|e| e["type"] == "NODE_START").unwrap();
    assert_eq!((start["payload"]["node"].as_str(), start["payload"]["type"].as_str()), (Some("risks"), Some("batch")));
    let ends: Vec<&&Value> = trace.iter().filter(|e| e["type"] == "NODE_END").collect();
    assert_eq!(ends.len(), 1, "un nodo, un NODE_END");
    assert_eq!(ends[0]["payload"]["answer"]["destructive"], 0.1);
}

#[tokio::test]
async fn batch_events_never_carry_the_questions() {
    let (daemon, script, _dir) = batch_daemon().await;
    // All the responses: if one were missing, the trace would be that of the error port.
    say(
        &script,
        "risks",
        200,
        answers(&[
            ("risks.destructive", "noul", json!(0.1), 0.9),
            ("risks.exfiltration", "noul", json!(0.1), 0.9),
            ("risks.privilege", "noul", json!(0.1), 0.9),
            ("risks.secret_exposure", "noul", json!(0.1), 0.9),
        ]),
    );
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.trim_start_matches("http://"))).await.unwrap();

    decide(&daemon, json!({ "state": "MARCADOR-SECRETO" })).await;

    let mut lines = Vec::new();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("sin eventos a tiempo");
        let Message::Text(text) = next.unwrap().unwrap() else { continue };
        let done = serde_json::from_str::<Value>(&text).map(|e| e["type"] == "DECISION_MADE").unwrap_or(false);
        lines.push(text.to_string());
        if done {
            break;
        }
    }
    for line in lines {
        assert!(!line.contains("MARCADOR-SECRETO"), "la entrada no viaja: {line}");
        // The load event carries the node's shape, never its instructions (which are the prompt).
        assert!(!line.contains("instructions"), "ni el prompt de las preguntas: {line}");
        assert!(!line.contains("¿Rompe algo?"), "ni las preguntas: {line}");
    }
}

#[tokio::test]
async fn batch_backend_failure_takes_the_error_port() {
    let (daemon, _script, _dir) = batch_daemon().await;

    // Nothing scripted: the fake backend answers 500.
    let (status, reply) = decide(&daemon, json!({ "state": "x" })).await;

    assert_eq!((status, reply["outcome"].as_str()), (200, Some("abstain")));
    assert_eq!(reply["path"][0]["port"], "error");
    assert_eq!(reply["path"][0]["answer"], Value::Null);
}
