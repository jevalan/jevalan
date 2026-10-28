mod common;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::post;
use common::{TempDir, answer, cfg_with, say, scripted_backend};
use futures_util::StreamExt;
use jevalan_daemon::mock::spawn_mock;
use jevalan_daemon::{AppState, router};
use jevalan_hub::capability::parse_capability;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const WAKE_FLOW: &str = include_str!("../../../../config/flows/agent-wakeup.yaml");
const SPECIALIST_ENV_PROBE: &str = "SOLO_EN_DOTENV";
const SPECIALIST: &str = env!("CARGO_BIN_EXE_fake-specialist");

fn caps_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../config/capabilities")
}

fn wakeup_yaml() -> String {
    std::fs::read_to_string(caps_dir().join("agent.wakeup.yaml")).unwrap()
}

/// `tool.risk` servida por el especialista de pega; el esquema de entrada admite `mode`.
fn exec_yaml(env: &[&str], deadline_ms: u64) -> String {
    let def = json!({
        "manifest_version": "2",
        "namespace": "io.github.prueba",
        "version": "1",
        "description": "riesgo de pega",
        "input_schema": { "type": "object" },
        "decision_schema": { "$file": "schemas/tool.risk.decision.json" },
        "provider": {
            "id": "toolgate",
            "exec": { "command": [SPECIALIST, "--marca-secreta"], "env": env },
            "abstain_when": "response.source == 'no-opinion'",
            "error_when": "response.source == 'fail-mode'",
        },
        "defaults": { "deadline_ms": deadline_ms },
    });
    serde_yaml::to_string(&def).unwrap()
}

fn http_yaml(url: &str) -> String {
    let def = json!({
        "manifest_version": "2",
        "namespace": "io.github.prueba",
        "version": "1",
        "input_schema": { "type": "object" },
        "decision_schema": { "$file": "schemas/tool.risk.decision.json" },
        "provider": { "id": "remote", "version": "9.9", "http": { "url": url } },
        "defaults": { "deadline_ms": 2000 },
    });
    serde_yaml::to_string(&def).unwrap()
}

struct Daemon {
    url: String,
    _dir: TempDir,
}

/// Daemon con el flujo `agent-wakeup`, ambos backends en `backend` y las capacidades dadas (nombre, YAML).
async fn start(backend: &str, allow_exec: bool, caps: &[(&str, String)]) -> Daemon {
    start_with(backend, allow_exec, &[], caps).await
}

async fn start_with(backend: &str, allow_exec: bool, dotenv: &[(&str, &str)], caps: &[(&str, String)]) -> Daemon {
    let dir = TempDir::with(&[("agent-wakeup.yaml", WAKE_FLOW)]);
    let mut cfg =
        cfg_with(&[("ollama", backend, None), ("typesafe", backend, None), ("openrouter", backend, None)], None);
    cfg.flows_dir = dir.0.clone();
    cfg.hub.allow_exec = allow_exec;
    cfg.hub.dotenv = dotenv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let state = AppState::new(cfg);
    let watcher = state.load_flows().unwrap();
    for (name, yaml) in caps {
        let cap = parse_capability(name, yaml, &caps_dir(), &|f| state.flows.get(f).is_some())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        state.capabilities.insert(cap);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _watcher = watcher;
        axum::serve(listener, router(state)).await
    });
    Daemon { url, _dir: dir }
}

async fn exec_daemon(env: &[&str], deadline_ms: u64) -> Daemon {
    start("http://127.0.0.1:1", true, &[("tool.risk", exec_yaml(env, deadline_ms))]).await
}

fn envelope(capability: &str, version: &str, input: Value, deadline_ms: Option<u64>) -> Value {
    let mut req = json!({
        "protocol_version": "jev-hub/0.1", "request_id": "req-1",
        "capability": capability, "capability_version": version, "input": input,
    });
    if let Some(ms) = deadline_ms {
        req["constraints"] = json!({ "deadline_ms": ms });
    }
    req
}

async fn post_hub(daemon: &Daemon, body: &Value) -> (u16, Value) {
    let resp = reqwest::Client::new().post(format!("{}/v1/hub", daemon.url)).json(body).send().await.unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

async fn risk(daemon: &Daemon, input: Value, deadline_ms: Option<u64>) -> Value {
    post_hub(daemon, &envelope("tool.risk", "1", input, deadline_ms)).await.1
}

fn code(reply: &Value) -> Option<&str> {
    reply["error"]["code"].as_str()
}

fn wake_input(extra: Value) -> Value {
    let mut input = json!({ "waiting_for": "un correo", "skipped": 0 });
    for (k, v) in extra.as_object().unwrap() {
        input[k] = v.clone();
    }
    input
}

async fn wake(daemon: &Daemon, input: Value) -> Value {
    post_hub(daemon, &envelope("agent.wakeup", "1", input, None)).await.1
}

#[tokio::test]
async fn agent_wakeup_decides_through_its_flow() {
    let mock = spawn_mock(7, (0, 5), 0.0).await.unwrap();
    let daemon = start(&mock.url, false, &[("agent.wakeup", wakeup_yaml())]).await;

    let reply = wake(&daemon, wake_input(json!({ "event": { "from": "ana" } }))).await;

    assert_eq!(reply["status"], "ok", "{reply}");
    assert!(reply["decision"]["wake"].is_boolean());
    assert_eq!(reply["provider"]["id"], "jevalan-flow");
    assert_eq!(reply["trace"]["cost"], "not_measured");
    assert!(reply["trace"]["path"].as_array().is_some_and(|p| p.len() >= 4), "{reply}");
}

#[tokio::test]
async fn agent_wakeup_reasons() {
    let (backend, script) = scripted_backend().await;
    let daemon = start(&backend, false, &[("agent.wakeup", wakeup_yaml())]).await;
    let event = json!({ "event": "llegó" });
    let verdict =
        |reply: &Value| (reply["decision"]["wake"].as_bool(), reply["decision"]["reason"].as_str().map(String::from));
    let expect = |wake: bool, reason: &str| (Some(wake), Some(reason.to_string()));

    let user = wake(&daemon, wake_input(json!({ "from_user": true }))).await;
    assert_eq!(verdict(&user), expect(true, "user-message"), "{user}");

    let nothing = wake(&daemon, wake_input(json!({}))).await;
    assert_eq!(verdict(&nothing), expect(true, "nothing-to-judge"), "{nothing}");

    let limit = wake(&daemon, wake_input(json!({ "event": "x", "skipped": 3, "max_skips": 3 }))).await;
    assert_eq!(verdict(&limit), expect(true, "skip-limit"), "{limit}");

    for (p, want) in [(0.1, expect(false, "judged")), (0.3, expect(true, "unsure")), (0.7, expect(true, "judged"))] {
        say(&script, "worth", 200, answer("worth", "noul", json!(p), 0.9));
        let reply = wake(&daemon, wake_input(event.clone())).await;
        assert_eq!(verdict(&reply), want, "p={p}: {reply}");
        assert_eq!(reply["decision"]["probability"], p);
    }

    say(&script, "worth", 500, String::new());
    let down = wake(&daemon, wake_input(event)).await;
    assert_eq!(verdict(&down), expect(true, "error"), "{down}");
}

#[tokio::test]
async fn tool_risk_ok_abstain_error() {
    let daemon = exec_daemon(&[], 5000).await;

    let ok = risk(&daemon, json!({ "mode": "allow" }), None).await;
    assert_eq!((ok["status"].as_str(), ok["decision"]["verdict"].as_str()), (Some("ok"), Some("allow")), "{ok}");
    assert_eq!(ok["provider"], json!({ "id": "toolgate", "version": "unconfigured" }));

    let none = risk(&daemon, json!({ "mode": "none" }), None).await;
    assert_eq!(none["status"], "abstain", "{none}");
    assert!(none.get("decision").is_none());

    let fail = risk(&daemon, json!({ "mode": "fail" }), None).await;
    assert_eq!((fail["status"].as_str(), code(&fail)), (Some("error"), Some("provider_failed")), "{fail}");
    assert!(fail.get("decision").is_none());
}

#[tokio::test]
async fn exec_is_disabled_by_default() {
    let daemon = start("http://127.0.0.1:1", false, &[("tool.risk", exec_yaml(&[], 5000))]).await;

    let reply = risk(&daemon, json!({ "mode": "allow" }), None).await;

    assert_eq!(code(&reply), Some("provider_disabled"), "{reply}");
    assert_eq!(reply["error"]["retryable"], false);
}

#[tokio::test]
async fn exec_deadline_kills_the_process() {
    let daemon = exec_daemon(&[], 5000).await;
    let started = Instant::now();

    let reply = risk(&daemon, json!({ "mode": "sleep" }), Some(300)).await;

    assert_eq!(code(&reply), Some("deadline_exceeded"), "{reply}");
    assert_eq!(reply["error"]["retryable"], true);
    assert!(started.elapsed() < Duration::from_secs(2), "tardó {:?}", started.elapsed());
}

#[tokio::test]
async fn exec_stdout_is_capped() {
    let daemon = exec_daemon(&[], 5000).await;

    let reply = risk(&daemon, json!({ "mode": "flood" }), None).await;

    assert_eq!(code(&reply), Some("provider_failed"), "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("1 MiB"));
    // El daemon sigue en pie.
    assert_eq!(risk(&daemon, json!({ "mode": "allow" }), None).await["status"], "ok");
}

#[tokio::test]
async fn exec_garbage_and_exit_code_fail() {
    let daemon = exec_daemon(&[], 5000).await;

    let garbage = risk(&daemon, json!({ "mode": "garbage" }), None).await;
    assert_eq!(code(&garbage), Some("provider_failed"), "{garbage}");

    let exit = risk(&daemon, json!({ "mode": "exit" }), None).await;
    assert_eq!(code(&exit), Some("provider_failed"), "{exit}");
    let message = exit["error"]["message"].as_str().unwrap();
    assert!(message.contains("fallo simulado") && message.contains('3'), "{message}");
}

#[tokio::test]
async fn exec_env_is_not_inherited() {
    // Los tests de este fichero solo leen estas variables desde el hijo, que las recibe por `Command::env`.
    unsafe {
        std::env::set_var("TYPESAFE_API_KEY", "clave-de-prueba");
        std::env::set_var("JEVALAN_TOKEN", "token-de-prueba");
    }
    let closed = exec_daemon(&[], 5000).await;
    let reply = risk(&closed, json!({ "mode": "env" }), None).await;
    assert_eq!(reply["decision"]["reason"], "none", "{reply}");

    let listed = exec_daemon(&["TYPESAFE_API_KEY"], 5000).await;
    let reply = risk(&listed, json!({ "mode": "env" }), None).await;
    assert_eq!(reply["decision"]["reason"], "TYPESAFE_API_KEY=clave-de-prueba", "{reply}");
}

#[tokio::test]
async fn dotenv_values_reach_exec_only_when_listed() {
    // La variable solo existe en el mapa del `.env`, no en el proceso.
    let dotenv = [("SOLO_EN_DOTENV", "valor-del-dotenv")];
    let caps = |env: &[&str]| [("tool.risk", exec_yaml(env, 5000))];
    let source = SPECIALIST_ENV_PROBE;

    let listed = start_with("http://127.0.0.1:1", true, &dotenv, &caps(&[source])).await;
    let reply = risk(&listed, json!({ "mode": "env" }), None).await;
    assert_eq!(reply["decision"]["reason"], "SOLO_EN_DOTENV=valor-del-dotenv", "{reply}");

    let unlisted = start_with("http://127.0.0.1:1", true, &dotenv, &caps(&[])).await;
    let reply = risk(&unlisted, json!({ "mode": "env" }), None).await;
    assert_eq!(reply["decision"]["reason"], "none", "{reply}");
}

#[tokio::test]
async fn envelope_errors() {
    let daemon = start("http://127.0.0.1:1", false, &[("agent.wakeup", wakeup_yaml())]).await;
    let client = reqwest::Client::new();

    let mut bad = envelope("agent.wakeup", "1", wake_input(json!({})), None);
    bad["protocol_version"] = json!("jev-hub/9");
    assert_eq!(post_hub(&daemon, &bad).await.0, 400);
    let resp = client.post(format!("{}/v1/hub", daemon.url)).body("no es json").send().await.unwrap();
    assert_eq!(resp.status(), 400);

    let cases = [
        (envelope("no.existe", "1", json!({}), None), "unknown_capability"),
        (envelope("agent.wakeup", "2", wake_input(json!({})), None), "version_mismatch"),
        (envelope("agent.wakeup", "1", json!({ "skipped": 0 }), None), "invalid_input"),
    ];
    for (body, want) in cases {
        let (status, reply) = post_hub(&daemon, &body).await;
        assert_eq!((status, reply["status"].as_str(), code(&reply)), (200, Some("error"), Some(want)), "{reply}");
    }
}

#[tokio::test]
async fn request_id_is_echoed() {
    let daemon = start("http://127.0.0.1:1", false, &[]).await;
    let mut body = envelope("no.existe", "1", json!({}), None);
    body["request_id"] = json!("mi-id-42");

    let (_, reply) = post_hub(&daemon, &body).await;

    assert_eq!(
        (reply["request_id"].as_str(), reply["protocol_version"].as_str()),
        (Some("mi-id-42"), Some("jev-hub/0.1"))
    );
}

#[tokio::test]
async fn invalid_decision_is_an_error() {
    let daemon = exec_daemon(&[], 5000).await;

    let reply = risk(&daemon, json!({ "mode": "maybe" }), None).await;

    assert_eq!(code(&reply), Some("invalid_decision"), "{reply}");
    assert!(reply.get("decision").is_none());
}

#[tokio::test]
async fn abstain_and_error_never_carry_a_decision() {
    let daemon = exec_daemon(&[], 300).await;
    let bodies = [
        envelope("tool.risk", "1", json!({ "mode": "none" }), None),
        envelope("tool.risk", "1", json!({ "mode": "fail" }), None),
        envelope("tool.risk", "1", json!({ "mode": "sleep" }), None),
        envelope("tool.risk", "1", json!({ "mode": "maybe" }), None),
        envelope("tool.risk", "1", json!(7), None),
        envelope("tool.risk", "9", json!({}), None),
        envelope("otra.cosa", "1", json!({}), None),
    ];
    for body in bodies {
        let (status, reply) = post_hub(&daemon, &body).await;
        assert_eq!(status, 200);
        assert_ne!(reply["status"], "ok", "{reply}");
        assert!(reply.get("decision").is_none(), "{reply}");
    }
}

#[tokio::test]
async fn deadline_never_grows() {
    let daemon = exec_daemon(&[], 300).await;

    let reply = risk(&daemon, json!({ "mode": "sleep" }), Some(10_000)).await;

    assert_eq!(code(&reply), Some("deadline_exceeded"), "{reply}");
    let elapsed = reply["trace"]["elapsed_ms"].as_u64().unwrap();
    assert!((250..4000).contains(&elapsed), "{elapsed} ms");
}

fn pid_alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out =
            std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).split_whitespace().any(|w| w == pid.to_string())
    } else {
        std::process::Command::new("kill").args(["-0", &pid.to_string()]).output().unwrap().status.success()
    }
}

#[tokio::test]
async fn deadline_kills_the_whole_tree() {
    let dir = TempDir::with(&[]);
    let pid_file = dir.0.join("grandchild.pid");
    let daemon = exec_daemon(&[], 5000).await;

    let input = json!({ "mode": "spawn-grandchild", "pid_file": pid_file });
    let reply = risk(&daemon, input, Some(2000)).await;

    assert_eq!(code(&reply), Some("deadline_exceeded"), "{reply}");
    let pid: u32 = std::fs::read_to_string(&pid_file).expect("el nieto no llegó a nacer").trim().parse().unwrap();
    let started = Instant::now();
    while pid_alive(pid) && started.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let alive = pid_alive(pid);
    if alive {
        // Que el test no deje huérfanos aunque falle.
        let pid = pid.to_string();
        let (tool, flags) = if cfg!(windows) { ("taskkill", "/F") } else { ("kill", "-KILL") };
        let args = if cfg!(windows) { vec![flags, "/PID", &pid] } else { vec![flags, &pid] };
        let _ = std::process::Command::new(tool).args(args).output();
    }
    assert!(!alive, "el nieto {pid} sigue vivo");
}

/// Especialista HTTP de prueba: `/ok` decide, `/bad` falla con 500.
async fn http_specialist() -> String {
    let allow = || async { axum::Json(json!({ "verdict": "allow", "reason": "ok", "source": "model" })) };
    let app = Router::new()
        .route("/ok", post(allow))
        .route("/bad", post(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
        .route("/moved", post(|| async { (StatusCode::FOUND, [("location", "/ok")]) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    url
}

#[tokio::test]
async fn provider_swap_is_transparent() {
    let base = http_specialist().await;
    let by_exec = exec_daemon(&[], 5000).await;
    let by_http = start("http://127.0.0.1:1", false, &[("tool.risk", http_yaml(&format!("{base}/ok")))]).await;

    let a = risk(&by_exec, json!({ "mode": "allow" }), None).await;
    let b = risk(&by_http, json!({ "mode": "allow" }), None).await;

    assert_eq!((a["status"].as_str(), b["status"].as_str()), (Some("ok"), Some("ok")), "{a} {b}");
    assert_eq!(a["decision"], b["decision"]);
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&a), keys(&b));
    assert_eq!((a["provider"]["id"].as_str(), b["provider"]["id"].as_str()), (Some("toolgate"), Some("remote")));
    assert_eq!(b["provider"]["version"], "9.9");
}

#[tokio::test]
async fn http_non_2xx_is_provider_failed() {
    let base = http_specialist().await;
    let daemon = start("http://127.0.0.1:1", false, &[("tool.risk", http_yaml(&format!("{base}/bad")))]).await;

    let reply = risk(&daemon, json!({}), None).await;

    assert_eq!(code(&reply), Some("provider_failed"), "{reply}");
    assert!(!reply.to_string().contains(&base), "la URL no debe salir: {reply}");
}

#[tokio::test]
async fn http_redirect_is_provider_failed() {
    let base = http_specialist().await;
    let daemon = start("http://127.0.0.1:1", false, &[("tool.risk", http_yaml(&format!("{base}/moved")))]).await;

    let reply = risk(&daemon, json!({}), None).await;

    assert_eq!(code(&reply), Some("provider_failed"), "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("302"), "{reply}");
}

#[tokio::test]
async fn capabilities_endpoint_hides_command_env_url() {
    let base = http_specialist().await;
    let daemon = start(
        "http://127.0.0.1:1",
        true,
        &[
            ("tool.risk", exec_yaml(&["TYPESAFE_API_KEY"], 5000)),
            ("agent.wakeup", wakeup_yaml()),
            ("other.remote", http_yaml(&format!("{base}/ok"))),
        ],
    )
    .await;

    let text = reqwest::get(format!("{}/v1/hub/capabilities", daemon.url)).await.unwrap().text().await.unwrap();

    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["capabilities"]["tool.risk"]["provider"], json!({ "id": "toolgate", "type": "exec" }));
    assert_eq!(body["capabilities"]["agent.wakeup"]["defaults"]["deadline_ms"], 3000);
    for secret in [SPECIALIST, "--marca-secreta", "TYPESAFE_API_KEY", base.as_str(), "command", "\"env\"", "\"url\""] {
        assert!(!text.contains(secret), "se filtró {secret}");
    }
}

#[tokio::test]
async fn hub_events_never_contain_input_or_decision() {
    let (backend, script) = scripted_backend().await;
    say(&script, "worth", 200, answer("worth", "noul", json!(0.7), 0.9));
    let daemon = start(&backend, true, &[("tool.risk", exec_yaml(&[], 5000)), ("agent.wakeup", wakeup_yaml())]).await;
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.url.trim_start_matches("http://"))).await.unwrap();
    let marker = "MARCADOR-SECRETO-9f3a";

    let risk_reply = risk(&daemon, json!({ "mode": "allow", "nota": marker }), None).await;
    assert_eq!(risk_reply["status"], "ok");
    wake(&daemon, wake_input(json!({ "event": marker }))).await;

    let mut lines = Vec::new();
    let mut responses = 0;
    while responses < 2 {
        let next = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("sin evento a tiempo");
        let Message::Text(text) = next.unwrap().unwrap() else { continue };
        responses += text.contains("HUB_RESPONSE") as u32;
        lines.push(text.to_string());
    }

    let events: Vec<Value> = lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    let kinds: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap_or_default()).collect();
    assert!(kinds.contains(&"HUB_REQUEST") && kinds.contains(&"HUB_RESPONSE"), "{kinds:?}");
    for line in lines.iter().filter(|l| !l.contains("FLOW_LOADED") && !l.contains("CAPABILITY_LOADED")) {
        for forbidden in
            [marker, "\"verdict\"", "\"wake\"", "--marca-secreta", SPECIALIST.replace('\\', "\\\\").as_str()]
        {
            assert!(!line.contains(forbidden), "{forbidden} en {line}");
        }
    }
    // Los eventos del flujo comparten traza con la petición al hub.
    let trace_of = |kind: &str| events.iter().rev().find(|e| e["type"] == kind).map(|e| e["trace_id"].clone());
    assert_eq!(trace_of("HUB_REQUEST"), trace_of("NODE_START"));
    assert!(kinds.contains(&"BACKEND_REQUEST"));
}

#[tokio::test]
async fn odd_capability_names_never_reach_events() {
    let daemon = start("http://127.0.0.1:1", false, &[]).await;
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.url.trim_start_matches("http://"))).await.unwrap();
    let long = "a.".repeat(200);

    let (_, reply) = post_hub(&daemon, &envelope(&long, "1", json!({}), None)).await;

    assert_eq!(reply["capability"], long.as_str());
    let mut seen = Vec::new();
    while seen.len() < 2 {
        let next = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("sin evento a tiempo");
        let Message::Text(text) = next.unwrap().unwrap() else { continue };
        assert!(!text.contains("a.a.a.a"), "{text}");
        let event: Value = serde_json::from_str(&text).unwrap();
        if event["type"] == "HUB_REQUEST" || event["type"] == "HUB_RESPONSE" {
            assert_eq!(event["payload"]["capability"], "?", "{event}");
            seen.push(event);
        }
    }
}
