//! jevalan daemon: transparent System One proxy, event bus and WebSocket.

pub mod auth;
pub mod bus;
pub mod config;
pub mod decide;
pub mod discovery;
pub mod draft;
pub mod flowrun;
pub mod flows;
pub mod hub;
pub mod mock;
pub mod probe;
pub mod proxy;
mod rng;
pub mod session;
pub mod signals;
pub mod simulate;
pub mod ws;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, post};
use bus::Bus;
use config::Config;
use flows::Registry;
use std::future::Future;
use std::sync::Arc;

const BODY_LIMIT: usize = 64 * 1024;
const BUS_CAPACITY: usize = 1024;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub bus: Bus,
    pub http: reqwest::Client,
    /// Client for the hub's `http` providers: it never follows redirects.
    pub hub_http: reqwest::Client,
    pub flows: Arc<Registry>,
    pub capabilities: Arc<hub::CapRegistry>,
    /// The session log, when the user opted in. It never feeds the public bus.
    pub recorder: Option<Arc<session::Recorder>>,
    /// Identifies this start of the process in the `trace_id` of `BACKENDS`.
    pub boot: Arc<str>,
}

impl AppState {
    pub fn new(cfg: Config) -> Self {
        let (cfg, bus) = (Arc::new(cfg), Bus::new(BUS_CAPACITY));
        let (flows, capabilities) = (Arc::new(Registry::new()), Arc::new(hub::CapRegistry::new()));
        let hook = (capabilities.clone(), cfg.clone(), bus.clone());
        flows.set_on_change(move |flows| hook.0.revalidate(&hook.1.hub.capabilities_dir, flows, &hook.1.hub, &hook.2));
        let hub_http =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default();
        let recorder = cfg.sessions.enabled.then(|| {
            let dir = cfg.sessions.dir.clone().or_else(session::default_dir).unwrap_or_else(|| "sessions".into());
            Arc::new(session::Recorder::open(dir, cfg.sessions.retention_days))
        });
        Self {
            cfg,
            bus,
            http: reqwest::Client::new(),
            hub_http,
            flows,
            capabilities,
            recorder,
            boot: bus::new_trace_id().into(),
        }
    }

    /// Starts storing the events of every trace. Needs a running runtime, so it is not part of `new`.
    pub fn start_recording(&self) {
        if let Some(recorder) = &self.recorder {
            recorder.follow(&self.bus);
        }
    }

    /// Loads `capabilities_dir` and watches for changes while the returned watcher lives.
    pub fn load_capabilities(&self) -> notify::Result<notify::RecommendedWatcher> {
        let dir = self.cfg.hub.capabilities_dir.clone();
        self.capabilities.load_dir(&dir, &self.flows, &self.cfg.hub, &self.bus);
        self.capabilities.clone().watch(dir, self.flows.clone(), self.cfg.hub.clone(), self.bus.clone())
    }

    /// Loads `flows_dir` and watches for changes while the returned watcher lives.
    pub fn load_flows(&self) -> notify::Result<notify::RecommendedWatcher> {
        self.flows.load_dir(&self.cfg.flows_dir, &self.cfg, &self.bus);
        self.flows.clone().watch(self.cfg.clone(), self.bus.clone())
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/{backend}/v1/systemone", post(proxy::named))
        .route("/v1/systemone", post(proxy::default))
        .route("/v1/decide/{flow}", post(decide::decide))
        .route("/v1/flows", get(decide::flows))
        .route("/v1/flows/validate", post(draft::validate))
        .route("/v1/flows/draft/run", post(draft::draft_run))
        .route("/v1/hub", post(hub::route::hub))
        .route("/v1/hub/capabilities", get(hub::route::capabilities))
        .route("/.well-known/jev-hub.json", get(hub::route::capabilities))
        .route("/events", get(ws::events))
        .route("/healthz", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_token))
        .with_state(state)
}

pub async fn serve(cfg: Config) -> std::io::Result<()> {
    let result = run(cfg, false, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;
    discovery::remove();
    result
}

/// Like `serve`, but without writing to stdout or stderr (it does not mess up a TUI) and with stop on demand.
pub async fn serve_quiet(cfg: Config, shutdown: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    run(cfg, true, shutdown).await
}

async fn run(cfg: Config, quiet: bool, shutdown: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(cfg.addr).await?;
    let addr = listener.local_addr()?;
    if !quiet {
        discovery::write(addr, cfg.token.as_deref());
    }
    let state = AppState::new(cfg);
    state.start_recording();
    let watcher = state.load_flows();
    let cap_watcher = state.load_capabilities();
    if !quiet {
        let _ = watcher.as_ref().map_err(|e| eprintln!("sin recarga en caliente: {e}"));
        report_flows(&state);
        report_capabilities(&state);
        println!("jevalan escuchando en http://{addr}");
    }
    let (_watcher, _cap_watcher) = (watcher.ok(), cap_watcher.ok());
    axum::serve(listener, router(state)).with_graceful_shutdown(shutdown).await
}

/// One line per loaded or invalid flow, and a warning if the directory is missing. Never keys or tokens.
fn report_flows(state: &AppState) {
    let dir = &state.cfg.flows_dir;
    if !dir.is_dir() {
        eprintln!("aviso: no existe el directorio de flujos {}", dir.display());
    }
    let list = state.flows.list();
    for (name, flow) in list["flows"].as_object().into_iter().flatten() {
        let nodes = flow["nodes"].as_object().map_or(0, |n| n.len());
        println!("flujo cargado: {name} ({nodes} nodos)");
    }
    for (name, reason) in list["invalid"].as_object().into_iter().flatten() {
        println!("flujo inválido: {name}: {}", reason.as_str().unwrap_or_default());
    }
}

/// One line per loaded or invalid capability, and warnings for a missing directory and for `exec` being off.
fn report_capabilities(state: &AppState) {
    let dir = &state.cfg.hub.capabilities_dir;
    if !dir.is_dir() {
        eprintln!("aviso: no existe el directorio de capacidades {}", dir.display());
    }
    let list = state.capabilities.list();
    for (name, cap) in list["capabilities"].as_object().into_iter().flatten() {
        let (kind, id) =
            (cap["provider"]["type"].as_str().unwrap_or("?"), cap["provider"]["id"].as_str().unwrap_or("?"));
        println!("capacidad cargada: {name} v{} ({kind}: {id})", cap["version"].as_str().unwrap_or("?"));
        if kind == "exec" && !state.cfg.hub.allow_exec {
            eprintln!("aviso: {name} usa exec y hub.allow_exec está desactivado");
        }
    }
    for (name, reason) in list["invalid"].as_object().into_iter().flatten() {
        println!("capacidad inválida: {name}: {}", reason.as_str().unwrap_or_default());
    }
}
