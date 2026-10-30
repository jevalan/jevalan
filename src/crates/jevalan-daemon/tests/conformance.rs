//! Conformance suite of the hub profile: fixed cases, replayed offline against a stand-in specialist.
//! Nothing here reaches a network or a paid provider, and two runs give the same answers. The invariants it fixes are
//! those of the profile (`jev-hub/0.1`), not of this implementation: an implementation that brings its own daemon
//! meets the profile when these same requests give the same answers.

mod common;

use common::{TempDir, cfg_with};
use jevalan_daemon::{AppState, router};
use jevalan_hub::capability::parse_capability;
use serde_json::{Value, json};
use std::path::PathBuf;

const SPECIALIST: &str = env!("CARGO_BIN_EXE_fake-specialist");

fn caps_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../config/capabilities")
}

/// `tool.risk` served by the stand-in; `env` is what the manifest lets the specialist see.
fn manifest(deadline_ms: u64, requires_env: &[&str]) -> String {
    let def = json!({
        "manifest_version": "2",
        "namespace": "io.github.prueba",
        "version": "1",
        "description": "riesgo de una llamada a herramienta",
        "publisher": { "repo": "https://github.com/prueba/toolgate", "license": "Apache-2.0" },
        "requires": { "runtime": "node", "env": requires_env, "network": ["api.example.org"], "platform": "any" },
        "input_schema": { "type": "object" },
        "decision_schema": { "$file": "schemas/tool.risk.decision.json" },
        "provider": {
            "id": "toolgate",
            "exec": { "command": [SPECIALIST, "--marca-secreta"], "env": ["SOLO_EN_EXEC"] },
            "abstain_when": "response.source == 'no-opinion'",
            "error_when": "response.source == 'fail-mode'",
        },
        "defaults": { "deadline_ms": deadline_ms },
    });
    serde_yaml::to_string(&def).unwrap()
}

struct Daemon {
    url: String,
    _dir: TempDir,
}

async fn start(manifest: &str) -> Daemon {
    let dir = TempDir::with(&[]);
    let mut cfg = cfg_with(&[("ollama", "http://127.0.0.1:1", None)], None);
    cfg.flows_dir = dir.0.clone();
    cfg.hub.allow_exec = true;
    let state = AppState::new(cfg);
    let cap = parse_capability("tool.risk", manifest, &caps_dir(), &|_| false).unwrap();
    state.capabilities.insert(cap);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await });
    Daemon { url, _dir: dir }
}

fn envelope(capability: &str, version: &str, input: Value) -> Value {
    json!({
        "protocol_version": "jev-hub/0.1", "request_id": "conformance",
        "capability": capability, "capability_version": version, "input": input,
    })
}

async fn ask(daemon: &Daemon, body: &Value) -> Value {
    let resp = reqwest::Client::new().post(format!("{}/v1/hub", daemon.url)).json(body).send().await.unwrap();
    resp.json().await.unwrap()
}

async fn get_text(daemon: &Daemon, path: &str) -> String {
    reqwest::get(format!("{}{path}", daemon.url)).await.unwrap().text().await.unwrap()
}

/// Drops what legitimately varies between runs (durations, timestamps and trace ids), so the rest compares byte
/// for byte.
fn stable(mut v: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(map) => {
                map.retain(|k, _| !matches!(k.as_str(), "duration_ms" | "ts" | "trace_id" | "trace"));
                map.values_mut().for_each(walk);
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

/// The fixed cases: a request and what the profile demands of its answer.
fn cases() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        ("ok carries a decision", envelope("tool.risk", "1", json!({ "mode": "allow" })), "ok"),
        ("no opinion is an abstention", envelope("tool.risk", "1", json!({ "mode": "none" })), "abstain"),
        ("a failing provider is an error", envelope("tool.risk", "1", json!({ "mode": "fail" })), "error"),
        ("a decision outside the schema is an error", envelope("tool.risk", "1", json!({ "mode": "maybe" })), "error"),
        ("input outside the schema is an error", envelope("tool.risk", "1", json!(7)), "error"),
        ("another contract version is an error", envelope("tool.risk", "9", json!({})), "error"),
        ("an unknown capability is an error", envelope("otra.cosa", "1", json!({})), "error"),
    ]
}

#[tokio::test]
async fn every_case_gets_the_status_the_profile_demands_and_only_ok_carries_a_decision() {
    let daemon = start(&manifest(5000, &[])).await;
    for (name, body, status) in cases() {
        let reply = ask(&daemon, &body).await;
        assert_eq!(reply["status"], status, "{name}: {reply}");
        assert_eq!(reply.get("decision").is_some(), status == "ok", "{name}: {reply}");
        assert_eq!(reply["request_id"], "conformance", "{name}: {reply}");
    }
}

#[tokio::test]
async fn an_exhausted_deadline_is_an_error_with_a_stable_code_and_never_a_default_decision() {
    let daemon = start(&manifest(300, &[])).await;
    let mut body = envelope("tool.risk", "1", json!({ "mode": "sleep" }));
    body["constraints"] = json!({ "deadline_ms": 10_000 });
    let reply = ask(&daemon, &body).await;
    assert_eq!((reply["status"].as_str(), reply["error"]["code"].as_str()), (Some("error"), Some("deadline_exceeded")));
    assert!(reply.get("decision").is_none(), "{reply}");
}

#[tokio::test]
async fn two_runs_give_the_same_answers_byte_for_byte() {
    let run = || async {
        let daemon = start(&manifest(5000, &[])).await;
        let mut answers = Vec::new();
        for (_, body, _) in cases() {
            answers.push(stable(ask(&daemon, &body).await));
        }
        serde_json::to_string(&answers).unwrap()
    };
    let (first, second) = (run().await, run().await);
    assert_eq!(first, second);
    assert!(first.contains("\"abstain\"") && first.contains("\"ok\""), "{first}");
}

#[tokio::test]
async fn discovery_is_the_same_at_both_addresses_and_publishes_no_binding() {
    let daemon = start(&manifest(5000, &["TYPESAFE_API_KEY"])).await;
    let listing = get_text(&daemon, "/v1/hub/capabilities").await;
    assert_eq!(listing, get_text(&daemon, "/.well-known/jev-hub.json").await);

    let body: Value = serde_json::from_str(&listing).unwrap();
    let cap = &body["capabilities"]["tool.risk"];
    assert_eq!(cap["id"], "io.github.prueba/tool.risk");
    assert!(cap["digest"].as_str().unwrap().starts_with("sha256:"), "{cap}");
    assert_eq!(cap["publisher"]["repo"], "https://github.com/prueba/toolgate");
    // The needs it declares travel by name; the binding (command, environment actually injected) never does.
    assert_eq!(cap["requires"]["env"], json!(["TYPESAFE_API_KEY"]));
    for secret in [SPECIALIST, "--marca-secreta", "SOLO_EN_EXEC", "\"command\"", "\"url\""] {
        assert!(!listing.contains(secret), "se filtró {secret}");
    }
}

#[tokio::test]
async fn the_digest_follows_the_contract_and_not_the_binding_or_the_machine() {
    let digest_of = |daemon: Daemon| async move {
        let body: Value = serde_json::from_str(&get_text(&daemon, "/v1/hub/capabilities").await).unwrap();
        body["capabilities"]["tool.risk"]["digest"].as_str().unwrap().to_string()
    };
    let a = digest_of(start(&manifest(5000, &[])).await).await;
    let b = digest_of(start(&manifest(5000, &[])).await).await;
    assert_eq!(a, b);
    // Another command and another injected environment: the same contract.
    let rebound = manifest(5000, &[]).replace("SOLO_EN_EXEC", "OTRA_VARIABLE").replace("--marca-secreta", "--otra");
    assert_eq!(a, digest_of(start(&rebound).await).await);
    // Another deadline is another contract.
    assert_ne!(a, digest_of(start(&manifest(4000, &[])).await).await);
}
