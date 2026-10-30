//! Replay and import of recorded sessions.

use jevalan_daemon::config::{Backend, Config, load};
use jevalan_daemon::session::log::{Trace, group};
use jevalan_lab::Lab;
use jevalan_lab::session::{Outcome, import, load_state, passed, render, replay};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn cfg() -> Config {
    let mut cfg = load(None, &|_| None).unwrap();
    for name in ["typesafe", "ollama", "openrouter"] {
        let backend = Backend {
            url: "http://127.0.0.1:1".into(),
            path: "/v1/systemone".into(),
            api_key: None,
            timeout: std::time::Duration::from_secs(1),
            cloud: false,
        };
        cfg.backends.insert(name.into(), backend);
    }
    cfg.flows_dir = repo().join("config/flows");
    cfg.hub.capabilities_dir = repo().join("config/capabilities");
    cfg
}

const INPUT: &str = r#"{"task":"arreglar el fallo de exportación","claim":"ya está arreglado","blocked":0,"files_changed":2,"tests_passed":true}"#;

fn line(ty: &str, extra: Value) -> Value {
    let mut v = json!({ "ts": "2026-10-29T10:00:00Z", "trace_id": "t1", "client": "claude-code/demo:1", "type": ty });
    for (k, val) in extra.as_object().into_iter().flatten() {
        v[k] = val.clone();
    }
    v
}

/// A recorded `agent.stop` run whose only model call, the node `done`, answered `reply`.
fn trace(reply: Value, decision: Value) -> Trace {
    let input: Value = serde_json::from_str(INPUT).unwrap();
    let lines = vec![
        line("INPUT", json!({ "capability": "agent.stop", "input": input })),
        line(
            "BACKEND_CALL",
            json!({ "backend": "openrouter", "node": "done", "request": { "questions": { "done": {} } }, "response": reply }),
        ),
        line("DECISION", json!({ "capability": "agent.stop", "status": "ok", "decision": decision })),
    ];
    group(lines).remove(0).traces.remove(0)
}

fn verified() -> Trace {
    let reply =
        json!({ "status": 200, "body": { "answers": { "done": { "type": "noul", "noul": 0.9, "confidence": 0.9 } } } });
    trace(reply, json!({ "stop": true, "reason": "verified", "probability": 0.9 }))
}

#[tokio::test]
async fn a_recorded_session_replays_to_the_same_decision() {
    let state = load_state(&cfg());
    let results = replay(&state, &[verified()]).await;
    assert_eq!(results[0].outcome, Outcome::Same, "{}", render(&results));
    assert!(passed(&results));
}

#[tokio::test]
async fn a_different_decision_is_reported_with_both_sides() {
    let state = load_state(&cfg());
    let mut t = verified();
    for l in &mut t.lines {
        if l["type"] == "DECISION" {
            l["decision"]["reason"] = json!("unverified");
        }
    }
    let results = replay(&state, &[t]).await;
    let Outcome::Different { expected, got } = &results[0].outcome else { panic!("{}", render(&results)) };
    assert_eq!(expected["decision"]["reason"], "unverified");
    assert_eq!(got["decision"]["reason"], "verified");
    assert!(!passed(&results));
}

#[tokio::test]
async fn a_missing_recording_is_reported_by_its_key() {
    let state = load_state(&cfg());
    let mut t = verified();
    t.lines.retain(|l| l["type"] != "BACKEND_CALL");
    let results = replay(&state, &[t]).await;
    assert_eq!(results[0].outcome, Outcome::Missing { key: "done".into() });
    assert!(!passed(&results));
}

#[tokio::test]
async fn a_failed_backend_replays_to_the_decision_it_took() {
    let state = load_state(&cfg());
    let t = trace(
        json!({ "status": 500, "body": { "error": "backend caído" } }),
        json!({ "stop": true, "reason": "error" }),
    );
    let results = replay(&state, &[t]).await;
    assert_eq!(results[0].outcome, Outcome::Same, "{}", render(&results));
}

#[tokio::test]
async fn traces_that_are_not_hub_capabilities_are_skipped_not_failed() {
    let state = load_state(&cfg());
    let mut t = verified();
    t.lines[0]["capability"] = json!("flow:agent-guard");
    let results = replay(&state, &[t]).await;
    assert!(matches!(results[0].outcome, Outcome::Skipped { .. }));
}

#[tokio::test]
async fn an_imported_session_is_a_fixture_the_bench_loads_and_passes() {
    let cfg = cfg();
    let state = load_state(&cfg);
    let lab = std::env::temp_dir().join(format!("jevalan-session-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&lab);
    let failed = trace(json!({ "status": 500, "body": { "error": "x" } }), json!({ "stop": true, "reason": "error" }));

    let done = import(&state, &[verified(), failed], &lab).unwrap();

    assert_eq!((done.written, done.identical, done.skipped.len(), done.lab_yaml_written), (1, 0, 1, true));
    // Importing again writes nothing new.
    assert_eq!(import(&state, &[verified()], &lab).unwrap().identical, 1);
    let bench = Lab::load(&lab, &cfg).unwrap_or_else(|p| panic!("{p:?}"));
    assert_eq!(bench.fixture_count(), 1);
    assert_eq!(bench.check(), Vec::<String>::new());
    let run = bench.offline(&jevalan_lab::Filter::default()).await;
    assert!(!run.failed, "{}", run.report);
    let _ = std::fs::remove_dir_all(lab);
}
