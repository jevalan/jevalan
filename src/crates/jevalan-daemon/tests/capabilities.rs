mod common;

use common::{TempDir, cfg_with, eventually};
use futures_util::StreamExt;
use jevalan_daemon::{AppState, router};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const WAKE_FLOW: &str = include_str!("../../../../config/flows/agent-wakeup.yaml");

fn repo_caps() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../config/capabilities")
}

fn repo_yaml(name: &str) -> String {
    std::fs::read_to_string(repo_caps().join(name)).unwrap()
}

struct Daemon {
    url: String,
    flows: TempDir,
    caps: TempDir,
}

impl Daemon {
    fn cap_file(&self, name: &str) -> PathBuf {
        self.caps.0.join(name)
    }

    async fn listing(&self) -> Value {
        reqwest::get(format!("{}/v1/hub/capabilities", self.url)).await.unwrap().json().await.unwrap()
    }

    async fn code_of(&self, capability: &str) -> Option<String> {
        let body = json!({
            "protocol_version": "jev-hub/0.1", "request_id": "r", "capability": capability,
            "capability_version": "1", "input": { "skipped": 0 },
        });
        let resp = reqwest::Client::new().post(format!("{}/v1/hub", self.url)).json(&body).send().await.unwrap();
        let reply: Value = resp.json().await.unwrap();
        reply["error"]["code"].as_str().map(str::to_string)
    }
}

fn copy_schemas(to: &Path) {
    let dir = to.join("schemas");
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(repo_caps().join("schemas")).unwrap().flatten() {
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
}

/// Daemon con el flujo `agent-wakeup` y las capacidades dadas en un directorio propio (con sus esquemas).
async fn start(caps: &[(&str, String)]) -> Daemon {
    start_with(caps, false, &[]).await
}

/// Como `start`, con `allow_exec` y las variables del `.env` indicados.
async fn start_with(caps: &[(&str, String)], allow_exec: bool, dotenv: &[(&str, &str)]) -> Daemon {
    let flows = TempDir::with(&[("agent-wakeup.yaml", WAKE_FLOW)]);
    let caps_dir = TempDir::with(&[]);
    copy_schemas(&caps_dir.0);
    for (name, yaml) in caps {
        std::fs::write(caps_dir.0.join(name), yaml).unwrap();
    }
    let mut cfg = cfg_with(
        &[
            ("ollama", "http://127.0.0.1:1", None),
            ("typesafe", "http://127.0.0.1:1", None),
            ("openrouter", "http://127.0.0.1:1", None),
        ],
        None,
    );
    cfg.flows_dir = flows.0.clone();
    cfg.hub.capabilities_dir = caps_dir.0.clone();
    cfg.hub.allow_exec = allow_exec;
    cfg.hub.dotenv = dotenv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let state = AppState::new(cfg);
    let (flow_watcher, cap_watcher) = (state.load_flows().unwrap(), state.load_capabilities().unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _watchers = (flow_watcher, cap_watcher);
        axum::serve(listener, router(state)).await
    });
    Daemon { url, flows, caps: caps_dir }
}

async fn replay(daemon: &Daemon) -> Vec<Value> {
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.url.trim_start_matches("http://"))).await.unwrap();
    let mut events = Vec::new();
    // BACKENDS, un FLOW_LOADED y las capacidades; el resto no llega hasta que algo ocurra.
    while let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
        events.push(serde_json::from_str(&text).unwrap());
    }
    events
}

#[tokio::test]
async fn capability_files_load_and_replay_on_connect() {
    let daemon = start(&[
        ("agent.wakeup.yaml", repo_yaml("agent.wakeup.yaml")),
        ("tool.risk.yaml", repo_yaml("tool.risk.yaml")),
    ])
    .await;

    let events = replay(&daemon).await;

    let kinds: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["BACKENDS", "FLOW_LOADED", "CAPABILITY_LOADED", "CAPABILITY_LOADED"]);
    let (wake, risk) = (&events[2], &events[3]);
    assert_eq!(wake["trace_id"], "capability:agent.wakeup:1");
    assert_eq!(wake["payload"]["provider_type"], "flow");
    assert_eq!(
        (risk["payload"]["capability"].as_str(), risk["payload"]["provider_type"].as_str()),
        (Some("tool.risk"), Some("exec"))
    );
    let list =
        |v: &Value| -> Vec<String> { v.as_array().unwrap().iter().map(|f| f.as_str().unwrap().into()).collect() };
    assert_eq!(wake["payload"]["flow"], "agent-wakeup");
    assert!(risk["payload"].get("flow").is_none());
    assert!(wake["payload"]["description"].as_str().unwrap().contains("despertar"));
    assert_eq!(
        list(&wake["payload"]["input_fields"]),
        ["event", "from_user", "max_skips", "observation", "skipped", "waiting_for"]
    );
    assert_eq!(list(&wake["payload"]["decision_fields"]), ["probability", "reason", "wake"]);
    assert_eq!(list(&risk["payload"]["decision_fields"]), ["probabilities", "reason", "source", "verdict"]);
    let text = serde_json::to_string(&events).unwrap();
    assert!(!text.contains("command") && !text.contains("http://"), "{text}");
    assert!(!text.contains("toolgate\",\"decide") && !text.contains("TYPESAFE_API_KEY"), "{text}");
    assert_eq!(daemon.listing().await["capabilities"].as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn new_capability_file_is_loaded_while_running() {
    let daemon = start(&[]).await;
    assert!(daemon.listing().await["capabilities"].as_object().unwrap().is_empty());

    std::fs::write(daemon.cap_file("agent.wakeup.yaml"), repo_yaml("agent.wakeup.yaml")).unwrap();

    assert!(eventually(|| async { daemon.listing().await["capabilities"].get("agent.wakeup").is_some() }).await);
}

#[tokio::test]
async fn edited_capability_gets_a_new_version_and_identical_rewrite_does_not() {
    let yaml = repo_yaml("agent.wakeup.yaml");
    let daemon = start(&[("agent.wakeup.yaml", yaml.clone())]).await;

    std::fs::write(daemon.cap_file("agent.wakeup.yaml"), &yaml).unwrap();
    std::fs::write(daemon.cap_file("agent.wakeup.yaml"), yaml.replace("Wakegate como flujo", "otra descripción"))
        .unwrap();

    let reloaded = || async { replay(&daemon).await.iter().any(|e| e["trace_id"] == "capability:agent.wakeup:2") };
    assert!(eventually(reloaded).await);
    assert!(!replay(&daemon).await.iter().any(|e| e["trace_id"] == "capability:agent.wakeup:3"));
}

#[tokio::test]
async fn invalid_reload_keeps_the_previous_capability() {
    let daemon = start(&[("agent.wakeup.yaml", repo_yaml("agent.wakeup.yaml"))]).await;

    std::fs::write(daemon.cap_file("agent.wakeup.yaml"), "version: [esto no vale").unwrap();

    assert!(eventually(|| async { daemon.listing().await["invalid"]["agent.wakeup"].is_string() }).await);
    assert!(daemon.listing().await["capabilities"].get("agent.wakeup").is_some());
    // Sigue sirviendo: la entrada vacía es inválida, pero la capacidad existe.
    assert_eq!(daemon.code_of("agent.wakeup").await.as_deref(), Some("invalid_input"));
}

#[tokio::test]
async fn deleted_capability_stops_serving() {
    let daemon = start(&[("agent.wakeup.yaml", repo_yaml("agent.wakeup.yaml"))]).await;
    assert_eq!(daemon.code_of("agent.wakeup").await.as_deref(), Some("invalid_input"));

    std::fs::remove_file(daemon.cap_file("agent.wakeup.yaml")).unwrap();

    assert!(
        eventually(|| async { daemon.code_of("agent.wakeup").await.as_deref() == Some("unknown_capability") }).await
    );
}

#[tokio::test]
async fn removing_its_flow_invalidates_a_capability() {
    let daemon = start(&[("agent.wakeup.yaml", repo_yaml("agent.wakeup.yaml"))]).await;
    let (mut ws, _) = connect_async(format!("ws://{}/events", daemon.url.trim_start_matches("http://"))).await.unwrap();

    std::fs::remove_file(daemon.flows.0.join("agent-wakeup.yaml")).unwrap();

    assert!(
        eventually(|| async { daemon.code_of("agent.wakeup").await.as_deref() == Some("unknown_capability") }).await
    );
    let listing = daemon.listing().await;
    assert!(listing["invalid"]["agent.wakeup"].as_str().unwrap().contains("agent-wakeup"), "{listing}");
    // El tablero debe enterarse de que ya no se sirve.
    let mut seen = Vec::new();
    while !seen.iter().any(|t| t == "CAPABILITY_REMOVED") {
        let next = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("sin CAPABILITY_REMOVED");
        if let Message::Text(text) = next.unwrap().unwrap() {
            seen.push(serde_json::from_str::<Value>(&text).unwrap()["type"].as_str().unwrap_or_default().to_string());
        }
    }

    // Al volver el flujo, la capacidad vuelve sin tocar su fichero.
    std::fs::write(daemon.flows.0.join("agent-wakeup.yaml"), WAKE_FLOW).unwrap();

    assert!(eventually(|| async { daemon.code_of("agent.wakeup").await.as_deref() == Some("invalid_input") }).await);
    assert!(daemon.listing().await["invalid"].as_object().unwrap().is_empty());
}

#[tokio::test]
async fn editing_a_schema_file_reloads_its_capability() {
    let daemon = start(&[("agent.wakeup.yaml", repo_yaml("agent.wakeup.yaml"))]).await;
    let schema = daemon.caps.0.join("schemas").join("agent.wakeup.input.json");
    let mut json: Value = serde_json::from_str(&std::fs::read_to_string(&schema).unwrap()).unwrap();
    json["description"] = json!("editado a mano");

    std::fs::write(&schema, json.to_string()).unwrap();

    let reloaded = || async {
        daemon.listing().await["capabilities"]["agent.wakeup"]["input_schema"]["description"] == "editado a mano"
    };
    assert!(eventually(reloaded).await);
    assert!(replay(&daemon).await.iter().any(|e| e["trace_id"] == "capability:agent.wakeup:2"));
}

const SPECIALIST: &str = env!("CARGO_BIN_EXE_fake-specialist");

fn exec_cap(program: &str) -> String {
    let def = json!({
        "manifest_version": "2",
        "namespace": "io.github.prueba",
        "version": "1",
        "input_schema": { "type": "object" },
        "decision_schema": { "$file": "schemas/tool.risk.decision.json" },
        "provider": { "id": "toolgate", "exec": { "command": [program, "decide"], "env": ["TYPESAFE_API_KEY"] } },
        "defaults": { "deadline_ms": 5000 },
    });
    serde_yaml::to_string(&def).unwrap()
}

async fn invalid_reason(daemon: &Daemon) -> Option<String> {
    daemon.listing().await["invalid"]["tool.risk"].as_str().map(str::to_string)
}

#[tokio::test]
async fn exec_command_expands_vars_from_dotenv_and_runs() {
    let daemon =
        start_with(&[("tool.risk.yaml", exec_cap("${FAKE_SPECIALIST}"))], true, &[("FAKE_SPECIALIST", SPECIALIST)])
            .await;
    assert_eq!(invalid_reason(&daemon).await, None);
    let body = json!({
        "protocol_version": "jev-hub/0.1", "request_id": "r", "capability": "tool.risk",
        "capability_version": "1", "input": { "mode": "allow" },
    });
    let resp = reqwest::Client::new().post(format!("{}/v1/hub", daemon.url)).json(&body).send().await.unwrap();
    let reply: Value = resp.json().await.unwrap();
    assert_eq!(
        (reply["status"].as_str(), reply["decision"]["verdict"].as_str()),
        (Some("ok"), Some("allow")),
        "{reply}"
    );

    // El comando expandido no se publica ni por HTTP ni por el bus.
    let published = format!("{}{}", daemon.listing().await, serde_json::to_string(&replay(&daemon).await).unwrap());
    assert!(!published.contains("fake-specialist") && !published.contains("FAKE_SPECIALIST"), "{published}");
}

#[tokio::test]
async fn process_env_wins_over_dotenv_and_dotenv_fills_the_gaps() {
    // CARGO_MANIFEST_DIR existe en el entorno del proceso: si ganara el `.env`, el `.cmd` invalidaría la capacidad.
    let cap = [("tool.risk.yaml", exec_cap("${CARGO_MANIFEST_DIR}/x"))];
    let daemon = start_with(&cap, true, &[("CARGO_MANIFEST_DIR", "C:/x/toolgate.cmd")]).await;
    assert_eq!(invalid_reason(&daemon).await, None);
    assert!(daemon.listing().await["capabilities"].get("tool.risk").is_some());

    let cap = [("tool.risk.yaml", exec_cap("${SOLO_EN_DOTENV}/x"))];
    let daemon = start_with(&cap, true, &[("SOLO_EN_DOTENV", "/opt")]).await;
    assert_eq!(invalid_reason(&daemon).await, None);
}

#[tokio::test]
async fn partial_expansion_and_batch_via_expansion() {
    let cap = [("tool.risk.yaml", exec_cap("${DIR_X}/bin/${NAME_X}.cmd"))];
    let daemon = start_with(&cap, true, &[("DIR_X", "C:/npm"), ("NAME_X", "toolgate")]).await;
    assert!(invalid_reason(&daemon).await.unwrap().contains("cmd.exe"));
}

#[tokio::test]
async fn undefined_var_invalidates_only_when_exec_is_allowed() {
    let cap = [("tool.risk.yaml", exec_cap("${NO_DEFINIDA_JAMAS}"))];
    let strict = start_with(&cap, true, &[]).await;
    let reason = invalid_reason(&strict).await.unwrap();
    assert!(reason.contains("exec.command usa ${NO_DEFINIDA_JAMAS}, que no está definida"), "{reason}");

    let lenient = start_with(&cap, false, &[]).await;
    assert_eq!(invalid_reason(&lenient).await, None);
    assert_eq!(lenient.code_of("tool.risk").await.as_deref(), Some("provider_disabled"));
}

/// La forma que hoy es válida y que v2 deja de aceptar. Se fija aquí para que el
/// rechazo del v1 no dependa de cómo esté escrito `exec_cap`.
const V1_MANIFEST: &str = r#"
version: "1"
description: "manifiesto v1"
input_schema: { $file: "schemas/tool.risk.input.json" }
decision_schema: { $file: "schemas/tool.risk.decision.json" }
provider: { id: toolgate, exec: { command: ["node", "-e", "0"] } }
defaults: { deadline_ms: 1000 }
"#;

/// `extra` es donde va lo que la prueba quiera poner o quitar: `namespace`,
/// `publisher`, `requires`.
fn v2_manifest(extra: &str) -> String {
    format!(
        r#"
manifest_version: "2"
version: "1"
description: "manifiesto v2"
input_schema: {{ $file: "schemas/tool.risk.input.json" }}
decision_schema: {{ $file: "schemas/tool.risk.decision.json" }}
{extra}
provider: {{ id: toolgate, exec: {{ command: ["node", "-e", "0"] }} }}
defaults: {{ deadline_ms: 1000 }}
"#
    )
}

#[tokio::test]
async fn a_v1_manifest_is_rejected_and_the_reason_names_the_missing_field() {
    let daemon = start(&[("tool.risk.yaml", V1_MANIFEST.to_string())]).await;
    let reason = invalid_reason(&daemon).await.unwrap_or_default();
    assert!(reason.contains("manifest_version"), "el motivo no nombra el campo: {reason}");
}

#[tokio::test]
async fn a_v2_manifest_without_namespace_is_rejected() {
    let daemon = start(&[("tool.risk.yaml", v2_manifest(""))]).await;
    let reason = invalid_reason(&daemon).await.unwrap_or_default();
    assert!(reason.contains("namespace"), "el motivo no nombra el campo: {reason}");
}

#[tokio::test]
async fn an_empty_namespace_is_rejected() {
    let daemon = start(&[("tool.risk.yaml", v2_manifest(r#"namespace: """#))]).await;
    let reason = invalid_reason(&daemon).await.unwrap_or_default();
    assert!(reason.contains("namespace"), "el motivo no nombra el campo: {reason}");
}

#[tokio::test]
async fn a_v2_manifest_loads() {
    let daemon = start(&[("tool.risk.yaml", v2_manifest("namespace: io.github.prueba"))]).await;
    assert_eq!(invalid_reason(&daemon).await, None);
    assert!(daemon.listing().await["capabilities"].get("tool.risk").is_some());
}
