#![allow(dead_code)]

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::post;
use jevalan_daemon::config::{Backend, Config};
use jevalan_daemon::{AppState, router};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

pub struct Captured {
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub type Log = Arc<Mutex<Vec<Captured>>>;

#[derive(Clone)]
struct Fake {
    status: u16,
    body: &'static str,
    log: Log,
}

async fn record(State(fake): State<Fake>, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    fake.log.lock().unwrap().push(Captured { headers, body });
    let status = StatusCode::from_u16(fake.status).unwrap();
    (status, [(header::CONTENT_TYPE, "application/json")], fake.body)
}

/// Fake backend: stores what it receives on `/v1/systemone` and answers what it is told.
pub async fn fake_backend(reply_status: u16, reply_body: &'static str) -> (String, Log) {
    fake_backend_at("/v1/systemone", reply_status, reply_body).await
}

/// Like `fake_backend`, but listening on `path`.
pub async fn fake_backend_at(path: &str, reply_status: u16, reply_body: &'static str) -> (String, Log) {
    let log = Log::default();
    let fake = Fake { status: reply_status, body: reply_body, log: log.clone() };
    let app = Router::new().route(path, post(record)).with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (url, log)
}

pub async fn start_daemon(cfg: Config) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(AppState::new(cfg));
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}")
}

/// The first backend in the list is the default.
pub fn cfg_with(backends: &[(&str, &str, Option<&str>)], token: Option<&str>) -> Config {
    let map: BTreeMap<String, Backend> = backends
        .iter()
        .map(|(name, url, key)| {
            let backend = Backend {
                url: url.to_string(),
                path: "/v1/systemone".into(),
                api_key: key.map(str::to_string),
                timeout: Duration::from_millis(2000),
                cloud: false,
            };
            (name.to_string(), backend)
        })
        .collect();
    Config {
        addr: "127.0.0.1:0".parse().unwrap(),
        token: token.map(str::to_string),
        backends: map,
        routes: BTreeMap::new(),
        default_backend: backends[0].0.to_string(),
        flows_dir: "./config/flows".into(),
        hub: Default::default(),
    }
}

pub const GUARD: &str = include_str!("../../../../../config/flows/agent-guard.yaml");
pub const ROUTE: &str = include_str!("../../../../../config/flows/model-route.yaml");

/// Flow with a single `batch` node of four questions: the `tool.risk` case, without its fixed rules.
pub const BATCH: &str = r#"
defaults: { backend: ollama, model: nimble }
start: risks
nodes:
  risks:
    type: batch
    questions:
      destructive: { type: noul, instructions: "¿Rompe algo?" }
      exfiltration: { type: noul, instructions: "¿Saca datos?" }
      privilege: { type: noul, instructions: "¿Eleva privilegios?" }
      secret_exposure: { type: noul, instructions: "¿Toca secretos?" }
    route:
      - { when: "answers.risks.destructive >= 0.85 || answers.risks.exfiltration >= 0.85 || answers.risks.privilege >= 0.85 || answers.risks.secret_exposure >= 0.85", to: deny }
      - { when: "answers.risks.destructive >= 0.55 || answers.risks.exfiltration >= 0.55 || answers.risks.privilege >= 0.55 || answers.risks.secret_exposure >= 0.55", to: ask }
    else: allow
    out: { error: abstain }
outputs:
  allow: { outcome: allow, data: { probabilities: "$answer/risks" } }
  deny: { outcome: deny, data: { probabilities: "$answer/risks" } }
  ask: { outcome: ask-user, data: { probabilities: "$answer/risks" } }
  abstain: { outcome: abstain }
"#;

/// System One response with several question bodies, named `"<node>.<question>"` (D3).
pub fn answers(named: &[(&str, &str, serde_json::Value, f64)]) -> String {
    let mut map = serde_json::Map::new();
    for (key, ty, value, confidence) in named {
        let field = (*ty).to_string();
        map.insert((*key).into(), serde_json::json!({ "type": *ty, field: value, "confidence": confidence }));
    }
    serde_json::json!({ "answers": map }).to_string()
}

/// Temporary directory that is deleted when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn with(files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("jevalan-test-{}", jevalan_daemon::bus::new_trace_id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Backend script: node (name of the single question) -> (status, body). Anything not scripted gives 500.
pub type Script = Arc<Mutex<HashMap<String, (u16, String)>>>;

/// The script and, in addition, what the backend received: needed to count requests and see their keys.
#[derive(Clone)]
pub struct Scripted {
    pub script: Script,
    pub log: Log,
}

/// The node a question key belongs to: `"risks.destructive"` → `"risks"`, `"worth"` → `"worth"`.
fn node_of(key: &str) -> &str {
    key.split_once('.').map_or(key, |(node, _)| node)
}

async fn scripted(State(scripted): State<Scripted>, body: Bytes) -> impl IntoResponse {
    scripted.log.lock().unwrap().push(Captured { headers: HeaderMap::new(), body: body.clone() });
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    // A `batch` node sends several questions: all hang from the same node and share a script (D3).
    let nodes: BTreeMap<String, usize> =
        json["questions"].as_object().into_iter().flatten().fold(BTreeMap::new(), |mut acc, (key, _)| {
            *acc.entry(node_of(key).to_string()).or_insert(0) += 1;
            acc
        });
    let (status, text) = match nodes.iter().max_by_key(|(_, n)| **n) {
        Some((node, _)) => scripted.script.lock().unwrap().get(node).cloned().unwrap_or((500, String::new())),
        None => (500, String::new()),
    };
    (StatusCode::from_u16(status).unwrap(), [(header::CONTENT_TYPE, "application/json")], text)
}

pub async fn scripted_backend() -> (String, Scripted) {
    let scripted_state = Scripted { script: Script::default(), log: Log::default() };
    let app = Router::new().route("/v1/systemone", post(scripted)).with_state(scripted_state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (url, scripted_state)
}

pub fn say(scripted: &Scripted, node: &str, status: u16, body: String) {
    scripted.script.lock().unwrap().insert(node.into(), (status, body));
}

/// Valid System One response to the question `node`.
pub fn answer(node: &str, ty: &str, value: serde_json::Value, confidence: f64) -> String {
    serde_json::json!({ "answers": { node: { "type": ty, ty: value, "confidence": confidence } } }).to_string()
}

/// Daemon with the flows in `dir` loaded and watched, and the backend catalogue it is given;
/// all point to `backend`. The first of `names` is the default backend.
pub async fn start_with_flows_and(backend: &str, names: &[&str], dir: &TempDir) -> String {
    let backends: Vec<(&str, &str, Option<&str>)> = names.iter().map(|n| (*n, backend, None)).collect();
    let mut cfg = cfg_with(&backends, None);
    cfg.flows_dir = dir.0.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(cfg);
    let watcher = state.load_flows().unwrap();
    tokio::spawn(async move {
        let _watcher = watcher;
        axum::serve(listener, router(state)).await
    });
    format!("http://{addr}")
}

/// Daemon with the flows in `dir` loaded and watched; the three backends of the examples point to `backend`.
pub async fn start_with_flows(backend: &str, dir: &TempDir) -> String {
    start_with_flows_and(backend, &["ollama", "typesafe", "openrouter"], dir).await
}

/// Repeats `check` every 100 ms for up to 3 s; no fixed sleeps.
pub async fn eventually<F, Fut>(mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..30 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Connection to the test daemon's event bus.
pub async fn connect(
    daemon: &str,
) -> impl futures_util::StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin {
    connect_async(format!("ws://{}/events", daemon.trim_start_matches("http://"))).await.unwrap().0
}

/// The next bus event, already read as JSON; fails if it does not arrive within three seconds.
pub async fn next_json<S>(ws: &mut S) -> serde_json::Value
where
    S: futures_util::StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let siguiente = tokio::time::timeout(Duration::from_secs(3), futures_util::StreamExt::next(ws))
        .await
        .expect("sin evento a tiempo");
    let Message::Text(texto) = siguiente.unwrap().unwrap() else { panic!("se esperaba texto") };
    serde_json::from_str(&texto).unwrap()
}
