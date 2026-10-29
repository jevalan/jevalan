mod common;

use common::{TempDir, answer, cfg_with, say, scripted_backend};
use jevalan_daemon::AppState;
use jevalan_daemon::hub::execute::{Executed, execute};
use jevalan_daemon::hub::transport::{Recordings, Transport, system_one_reply};
use jevalan_flow::Flow;
use jevalan_hub::capability::{Capability, parse_capability};
use jevalan_hub::envelope::parse_request;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const WAKE_FLOW: &str = include_str!("../../../../config/flows/agent-wakeup.yaml");
const SPECIALIST: &str = env!("CARGO_BIN_EXE_fake-specialist");
const DEAD: &str = "http://127.0.0.1:1";

fn caps_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../config/capabilities")
}

struct Lab {
    state: AppState,
    cap: Capability,
    _dir: TempDir,
    _watcher: notify::RecommendedWatcher,
}

fn lab(typesafe: &str, ollama: &str, allow_exec: bool, name: &str, yaml: String) -> Lab {
    let dir = TempDir::with(&[("agent-wakeup.yaml", WAKE_FLOW)]);
    let mut cfg =
        cfg_with(&[("ollama", ollama, None), ("typesafe", typesafe, None), ("openrouter", typesafe, None)], None);
    cfg.flows_dir = dir.0.clone();
    cfg.hub.allow_exec = allow_exec;
    let state = AppState::new(cfg);
    let watcher = state.load_flows().unwrap();
    let cap = parse_capability(name, &yaml, &caps_dir(), &|f| state.flows.get(f).is_some()).unwrap();
    Lab { state, cap, _dir: dir, _watcher: watcher }
}

fn wakeup(typesafe: &str, ollama: &str) -> Lab {
    let yaml = std::fs::read_to_string(caps_dir().join("agent.wakeup.yaml")).unwrap();
    lab(typesafe, ollama, false, "agent.wakeup", yaml)
}

fn risk(allow_exec: bool) -> Lab {
    let def = json!({
        "manifest_version": "2",
        "namespace": "io.github.prueba",
        "version": "1",
        "input_schema": { "type": "object" },
        "decision_schema": { "$file": "schemas/tool.risk.decision.json" },
        "provider": { "id": "toolgate", "exec": { "command": [SPECIALIST], "env": [] } },
        "defaults": { "deadline_ms": 5000 },
    });
    lab(DEAD, DEAD, allow_exec, "tool.risk", serde_yaml::to_string(&def).unwrap())
}

async fn run(lab: &Lab, input: Value, flow: Option<Arc<Flow>>, transport: &Transport) -> Executed {
    let body = json!({
        "protocol_version": "jev-hub/0.1", "request_id": "r1",
        "capability": lab.cap.name, "capability_version": "1", "input": input,
    });
    let req = parse_request(body.to_string().as_bytes()).unwrap();
    execute(&lab.state, &req, Some(&lab.cap), flow, transport).await
}

fn event() -> Value {
    json!({ "waiting_for": "un correo", "skipped": 0, "event": "llegó" })
}

/// The response without the trace, which carries ids and times.
fn bare(mut response: Value) -> Value {
    response.as_object_mut().unwrap().remove("trace");
    response
}

fn replay(entries: &[(&str, Value)]) -> Transport {
    let rec: Recordings = entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    Transport::Replay(Arc::new(rec))
}

#[tokio::test]
async fn replay_matches_live() {
    let (url, script) = scripted_backend().await;
    say(&script, "worth", 200, answer("worth", "noul", json!(0.7), 0.9));
    let live = run(&wakeup(&url, &url), event(), None, &Transport::Live).await;

    let recorded = system_one_reply("worth", "noul", &json!(0.7), 0.9);
    let replayed = run(&wakeup(DEAD, DEAD), event(), None, &replay(&[("worth", recorded)])).await;

    assert_eq!(live.response["status"], "ok", "{}", live.response);
    assert_eq!(bare(live.response), bare(replayed.response));
    assert_eq!(replayed.missing, None);
}

#[tokio::test]
async fn missing_recording_is_reported() {
    let out = run(&wakeup(DEAD, DEAD), event(), None, &replay(&[])).await;
    assert_eq!(out.missing.as_deref(), Some("worth"));
    assert_eq!(out.response["status"], "error", "{}", out.response);
    assert!(out.response["error"]["message"].as_str().unwrap().contains("missing_recording:worth"));
}

#[tokio::test]
async fn record_then_replay_reproduces() {
    let (url, script) = scripted_backend().await;
    say(&script, "worth", 200, answer("worth", "noul", json!(0.3), 0.8));
    let store = Arc::new(Mutex::new(Recordings::new()));
    let live = run(&wakeup(&url, &url), event(), None, &Transport::Record(store.clone())).await;
    let rec = store.lock().unwrap().clone();
    assert_eq!(rec.keys().collect::<Vec<_>>(), ["worth"]);
    assert_eq!(rec["worth"].as_object().unwrap().keys().collect::<Vec<_>>(), ["body", "status"]);
    let again = run(&wakeup(DEAD, DEAD), event(), None, &Transport::Replay(Arc::new(rec))).await;
    assert_eq!(bare(live.response), bare(again.response));

    // Specialist: recorded with exec enabled and replayed with exec disabled, launching nothing.
    let store = Arc::new(Mutex::new(Recordings::new()));
    let live = run(&risk(true), json!({ "mode": "allow" }), None, &Transport::Record(store.clone())).await;
    assert_eq!(live.response["status"], "ok", "{}", live.response);
    let rec = store.lock().unwrap().clone();
    assert_eq!(rec.keys().collect::<Vec<_>>(), ["toolgate"]);
    let again = run(&risk(false), json!({ "mode": "allow" }), None, &Transport::Replay(Arc::new(rec))).await;
    assert_eq!(bare(live.response), bare(again.response));
}

#[tokio::test]
async fn flow_override_switches_backend() {
    let (typesafe, a) = scripted_backend().await;
    let (ollama, b) = scripted_backend().await;
    say(&a, "worth", 200, answer("worth", "noul", json!(0.1), 0.9));
    say(&b, "worth", 200, answer("worth", "noul", json!(0.7), 0.9));
    let lab = wakeup(&typesafe, &ollama);

    let default = run(&lab, event(), None, &Transport::Live).await;
    assert_eq!(default.response["decision"]["wake"], false, "{}", default.response);

    let text = WAKE_FLOW.replace("backend: openrouter", "backend: ollama");
    let flow = Arc::new(
        jevalan_flow::parse("agent-wakeup", &text, &["ollama", "typesafe", "openrouter"], "openrouter").unwrap(),
    );
    let switched = run(&lab, event(), Some(flow), &Transport::Live).await;
    assert_eq!(switched.response["decision"]["wake"], true, "{}", switched.response);
}

#[tokio::test]
async fn measure_reports_tokens_and_confidence() {
    let (url, script) = scripted_backend().await;
    let body = json!({
        "answers": { "worth": { "type": "noul", "noul": 0.7, "confidence": 0.85 } },
        "usage": { "input_tokens": 10, "output_tokens": 3 },
    });
    say(&script, "worth", 200, body.to_string());
    let out = run(&wakeup(&url, &url), event(), None, &Transport::Live).await;
    let m = out.measure;
    assert_eq!((m.input_tokens, m.output_tokens, m.confidence), (Some(10), Some(3), Some(0.7)));
    // The measured confidence is the probability of the answer given (noul 0.7), not the reported `confidence`.

    // Rules only: nothing to measure.
    let rules = run(&wakeup(DEAD, DEAD), json!({ "waiting_for": "x", "skipped": 0 }), None, &Transport::Live).await;
    assert_eq!((rules.measure.input_tokens, rules.measure.confidence), (None, None));
}

#[tokio::test]
async fn failed_node_via_error_port_invents_no_confidence() {
    let (url, script) = scripted_backend().await;
    say(&script, "worth", 500, json!({ "error": "caído" }).to_string());
    let out = run(&wakeup(&url, &url), event(), None, &Transport::Live).await;
    assert_eq!(out.response["decision"]["reason"], "error", "{}", out.response);
    assert_eq!(out.measure.confidence, None, "un fallo no aporta confianza, ni 0.0");
    assert_eq!((out.measure.backend_calls, out.measure.unmetered_calls), (1, 1));
}

#[tokio::test]
async fn measure_counts_backend_calls_and_unmetered_ones() {
    let (url, script) = scripted_backend().await;
    let rules = run(&wakeup(DEAD, DEAD), json!({ "waiting_for": "x", "skipped": 0 }), None, &Transport::Live).await;
    assert_eq!((rules.measure.backend_calls, rules.measure.unmetered_calls), (0, 0));

    let with = json!({ "answers": { "worth": { "type": "noul", "noul": 0.7, "confidence": 0.8 } }, "usage": { "input_tokens": 1, "output_tokens": 1 } });
    say(&script, "worth", 200, with.to_string());
    let out = run(&wakeup(&url, &url), event(), None, &Transport::Live).await;
    assert_eq!((out.measure.backend_calls, out.measure.unmetered_calls), (1, 0));

    say(&script, "worth", 200, answer("worth", "noul", json!(0.7), 0.8));
    let out = run(&wakeup(&url, &url), event(), None, &Transport::Live).await;
    assert_eq!((out.measure.backend_calls, out.measure.unmetered_calls), (1, 1));
}

#[tokio::test]
async fn measure_sums_reported_cost_only_when_every_call_declares_it() {
    let (url, script) = scripted_backend().await;
    let usage = |extra: Value| {
        let mut u = json!({ "input_tokens": 1, "output_tokens": 1 });
        u.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        json!({ "answers": { "worth": { "type": "noul", "noul": 0.7 } }, "usage": u }).to_string()
    };
    say(&script, "worth", 200, usage(json!({ "cost": 0.0125 })));
    let out = run(&wakeup(&url, &url), event(), None, &Transport::Live).await;
    assert_eq!(out.measure.reported_cost_usd, Some(0.0125));

    say(&script, "worth", 200, usage(json!({})));
    let out = run(&wakeup(&url, &url), event(), None, &Transport::Live).await;
    assert_eq!(out.measure.reported_cost_usd, None);

    // Without calls to backends there is no declared cost either.
    let rules = run(&wakeup(DEAD, DEAD), json!({ "waiting_for": "x", "skipped": 0 }), None, &Transport::Live).await;
    assert_eq!(rules.measure.reported_cost_usd, None);
}

#[tokio::test]
async fn io_mixes_costed_and_uncosted_calls_into_none() {
    use jevalan_daemon::hub::transport::Io;
    let reply = |usage: Value| json!({ "status": 200, "body": { "usage": usage } });
    let rec = Recordings::from([
        ("a".to_string(), reply(json!({ "input_tokens": 1, "output_tokens": 1, "cost": 0.5 }))),
        ("b".to_string(), reply(json!({ "input_tokens": 1, "output_tokens": 1, "cost": 0.25 }))),
        ("c".to_string(), reply(json!({ "input_tokens": 1, "output_tokens": 1 }))),
    ]);
    let transport = Transport::Replay(Arc::new(rec));
    let state = AppState::new(cfg_with(&[("x", DEAD, None)], None));
    let io = Io::new(&transport);
    for node in ["a", "b"] {
        io.backend(&state, "x", node, Default::default()).await.unwrap();
    }
    assert_eq!(io.reported_cost(), Some(0.75));
    io.backend(&state, "x", "c", Default::default()).await.unwrap();
    assert_eq!(io.reported_cost(), None);
}
