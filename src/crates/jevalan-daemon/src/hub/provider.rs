//! Invocation of the three provider kinds under the effective deadline.

use crate::AppState;
use crate::bus::Tracer;
use crate::flowrun::{Finish, run_flow};
use crate::hub::exec;
use crate::hub::transport::Io;
use jevalan_events::v2::EventKind;
use jevalan_flow::{Flow, PathStep};
use jevalan_hub::capability::{Capability, Provider};
use jevalan_hub::classify::{ErrorCode, Status, classify, from_flow};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const MAX_HTTP_BODY: usize = 1024 * 1024;

pub struct Invoked {
    pub status: Status,
    /// Only `flow` providers leave a path.
    pub path: Option<Vec<PathStep>>,
}

fn plain(status: Status) -> Invoked {
    Invoked { status, path: None }
}

fn deadline_exceeded() -> Status {
    Status::error(ErrorCode::DeadlineExceeded, "el proveedor no respondió a tiempo")
}

pub async fn invoke(
    state: &AppState,
    cap: &Capability,
    input: &Value,
    tracer: &Tracer,
    deadline: Duration,
    flow_override: Option<Arc<Flow>>,
    io: &Io<'_>,
) -> Invoked {
    match &cap.provider {
        Provider::Flow { flow } => run_native(state, flow, input, tracer, deadline, flow_override, io).await,
        Provider::Exec { command, env } => {
            if !state.cfg.hub.allow_exec && !io.is_replay() {
                let msg = "los proveedores exec están desactivados (hub.allow_exec)";
                return plain(Status::error(ErrorCode::ProviderDisabled, msg));
            }
            let started = Instant::now();
            emit_request(tracer, &cap.provider_id);
            let run = exec::run(command, env, &state.cfg.hub.dotenv, input, deadline);
            let result = io.specialist(&cap.provider_id, run).await;
            emit_response(tracer, &cap.provider_id, result.is_ok(), 0, started);
            plain(finish(result, cap))
        }
        Provider::Http { url } => {
            let started = Instant::now();
            emit_request(tracer, &cap.provider_id);
            let http_status = std::sync::atomic::AtomicU16::new(0);
            let live = async {
                let (status, result) = post(state, url, input).await;
                http_status.store(status, Ordering::Relaxed);
                result
            };
            let result = match tokio::time::timeout(deadline, io.specialist(&cap.provider_id, live)).await {
                Ok(done) => done,
                Err(_) => Err(deadline_exceeded()),
            };
            let status = http_status.load(Ordering::Relaxed);
            emit_response(tracer, &cap.provider_id, result.is_ok(), status, started);
            plain(finish(result, cap))
        }
    }
}

async fn run_native(
    state: &AppState,
    name: &str,
    input: &Value,
    tracer: &Tracer,
    deadline: Duration,
    flow_override: Option<Arc<Flow>>,
    io: &Io<'_>,
) -> Invoked {
    let Some(flow) = flow_override.or_else(|| state.flows.get(name)) else {
        return plain(Status::error(ErrorCode::ProviderFailed, "el flujo del proveedor no está cargado"));
    };
    match tokio::time::timeout(deadline, run_flow(state, name, &flow, input, tracer, io)).await {
        Err(_) => plain(deadline_exceeded()),
        Ok(ran) => {
            let status = match ran.finish {
                Finish::Done { outcome, data } => from_flow(&outcome, data),
                Finish::Failed { message } => Status::error(ErrorCode::ProviderFailed, message),
            };
            Invoked { status, path: Some(ran.path) }
        }
    }
}

fn finish(result: Result<Value, Status>, cap: &Capability) -> Status {
    match result {
        Ok(response) => classify(response, cap.abstain_when.as_deref(), cap.error_when.as_deref()),
        Err(status) => status,
    }
}

/// `POST` of the `input` to the configured URL; only `Content-Type` travels. Returns the HTTP status (0 if there was none).
async fn post(state: &AppState, url: &str, input: &Value) -> (u16, Result<Value, Status>) {
    let fail = |m: String| Status::error(ErrorCode::ProviderFailed, m);
    let mut resp = match state.hub_http.post(url).json(input).send().await {
        Ok(r) => r,
        Err(e) => return (0, Err(fail(format!("petición fallida: {}", e.without_url())))),
    };
    let status = resp.status().as_u16();
    let mut body = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() <= MAX_HTTP_BODY => body.extend_from_slice(&chunk),
            Ok(Some(_)) => return (status, Err(fail("la respuesta del especialista supera 1 MiB".into()))),
            Ok(None) => break,
            Err(e) => return (status, Err(fail(format!("lectura fallida: {}", e.without_url())))),
        }
    }
    if !(200..300).contains(&status) {
        return (status, Err(fail(format!("el especialista respondió HTTP {status}"))));
    }
    let parsed = serde_json::from_slice(&body).map_err(|_| fail("la respuesta del especialista no es JSON".into()));
    (status, parsed)
}

fn emit_request(tracer: &Tracer, provider: &str) {
    tracer.emit(EventKind::BackendRequest { backend: provider.into(), model: "-".into(), questions: 1 });
}

fn emit_response(tracer: &Tracer, provider: &str, ok: bool, status: u16, started: Instant) {
    let duration_ms = started.elapsed().as_millis() as u64;
    tracer.emit(EventKind::BackendResponse { backend: provider.into(), ok, status, duration_ms, input_tokens: None });
}
