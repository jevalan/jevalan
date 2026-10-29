//! Editor drafts: validate and run a flow that does not enter the registry.

use crate::AppState;
use crate::bus::{Tracer, new_trace_id};
use crate::flowrun::{Finish, run_flow};
use crate::hub::transport::{Io, Recordings, Transport};
use crate::proxy::error_response;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use jevalan_flow::{Flow, parse_with};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, PoisonError};

/// Cuerpo de `POST /v1/flows/validate`.
#[derive(Deserialize)]
struct Validate {
    /// Flow name, as given by the file name (`tool-risk-local`).
    flow: String,
    /// The YAML text, exactly as it is on disk.
    yaml: String,
}

/// The flow the body carries, judged with the real parser and the daemon's backend catalogue.
fn leer(state: &AppState, flow: &str, yaml: &str) -> Result<Flow, String> {
    let backends: Vec<&str> = state.cfg.backends.keys().map(String::as_str).collect();
    let (signals, _) = crate::signals::load_dir(&state.cfg.hub.signals_dir);
    parse_with(flow, yaml, &signals, &backends, &state.cfg.default_backend).map_err(|e| e.to_string())
}

/// `POST /v1/flows/validate`: the parser's verdict on the draft, without publishing it.
pub async fn validate(State(state): State<AppState>, body: Bytes) -> Response {
    let req: Validate = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("JSON inválido: {e}")),
    };
    match leer(&state, &req.flow, &req.yaml) {
        Ok(flow) => Json(json!({ "ok": true, "flow": flow })).into_response(),
        // An invalid flow is not a request error: the verdict travels in `ok`, with the
        // real `FlowError` message, which already comes in Spanish.
        Err(reason) => Json(json!({ "ok": false, "error": reason })).into_response(),
    }
}

/// Cuerpo de `POST /v1/flows/draft/run`.
#[derive(Deserialize)]
struct Draft {
    /// Flow name, which only signs the events; the registry does not see it.
    flow: String,
    yaml: String,
    /// The flow's input, shaped like the one in `POST /v1/hub` for `tool.risk`.
    input: Value,
    /// `live`, `replay` o `record`.
    transport: String,
    /// Recordings for `replay` (key: the node, or `"<node>.<question>"` in a `batch`).
    #[serde(default)]
    cassette: Option<Recordings>,
}

/// The name under which a draft signs its events: this way the TUI does not think the flow changed.
fn borrador(flow: &str) -> String {
    format!("{flow} (borrador)")
}

/// `POST /v1/flows/draft/run`: runs the draft once, without publishing it.
pub async fn draft_run(State(state): State<AppState>, body: Bytes) -> Response {
    let req: Draft = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("JSON inválido: {e}")),
    };
    let flow = match leer(&state, &req.flow, &req.yaml) {
        Ok(flow) => flow,
        Err(reason) => {
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "error": reason, "flow": req.flow })))
                .into_response();
        }
    };
    // The transport is chosen before anything is spent: an unknown name never reaches the engine.
    let grabado = Arc::new(Mutex::new(Recordings::new()));
    let transport = match req.transport.as_str() {
        "live" => Transport::Live,
        "replay" => Transport::Replay(Arc::new(req.cassette.clone().unwrap_or_default())),
        "record" => Transport::Record(grabado.clone()),
        otro => return error_response(StatusCode::BAD_REQUEST, &format!("transporte desconocido: {otro}")),
    };
    let tracer: Tracer = state.bus.tracer(new_trace_id());
    let io = Io::new(&transport);
    let ran = run_flow(&state, &borrador(&req.flow), &flow, &req.input, &tracer, &io).await;
    let (estado, mut respuesta) = match ran.finish {
        Finish::Done { outcome, data } => {
            let mut respuesta =
                json!({ "flow": req.flow, "outcome": outcome, "trace_id": tracer.trace_id(), "path": ran.path });
            if let Some(data) = data {
                respuesta["data"] = data;
            }
            (StatusCode::OK, respuesta)
        }
        Finish::Failed { message } => {
            let respuesta =
                json!({ "error": message, "flow": req.flow, "trace_id": tracer.trace_id(), "path": ran.path });
            (StatusCode::BAD_GATEWAY, respuesta)
        }
    };
    // The draft's confidence is that of the last node that answered; `null` if none did.
    respuesta["confidence"] = json!(io.measured().2);
    if req.transport == "record" {
        respuesta["cassette"] = json!(grabado.lock().unwrap_or_else(PoisonError::into_inner).clone());
    }
    (estado, Json(respuesta)).into_response()
}
