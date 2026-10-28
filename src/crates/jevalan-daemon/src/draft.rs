//! Borradores del editor: validar y ejecutar un flujo que no entra en el registro.

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
    /// Nombre del flujo, tal como lo da el nombre del fichero (`tool-risk-local`).
    flow: String,
    /// El texto del YAML, tal cual está en el disco.
    yaml: String,
}

/// El flujo que el cuerpo trae, juzgado con el parser real y el catálogo de backends del daemon.
fn leer(state: &AppState, flow: &str, yaml: &str) -> Result<Flow, String> {
    let backends: Vec<&str> = state.cfg.backends.keys().map(String::as_str).collect();
    let (signals, _) = crate::signals::load_dir(&state.cfg.hub.signals_dir);
    parse_with(flow, yaml, &signals, &backends, &state.cfg.default_backend).map_err(|e| e.to_string())
}

/// `POST /v1/flows/validate`: el veredicto del parser sobre el borrador, sin publicarlo.
pub async fn validate(State(state): State<AppState>, body: Bytes) -> Response {
    let req: Validate = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("JSON inválido: {e}")),
    };
    match leer(&state, &req.flow, &req.yaml) {
        Ok(flow) => Json(json!({ "ok": true, "flow": flow })).into_response(),
        // Un flujo inválido no es un error de la petición: el veredicto viaja en `ok`, con el
        // mensaje del `FlowError` real, que ya viene en español.
        Err(reason) => Json(json!({ "ok": false, "error": reason })).into_response(),
    }
}

/// Cuerpo de `POST /v1/flows/draft/run`.
#[derive(Deserialize)]
struct Draft {
    /// Nombre del flujo, que solo firma los eventos; el registro no lo ve.
    flow: String,
    yaml: String,
    /// La entrada del flujo, con la misma forma que la de `POST /v1/hub` para `tool.risk`.
    input: Value,
    /// `live`, `replay` o `record`.
    transport: String,
    /// Grabaciones para `replay` (clave: el nodo, o `"<nodo>.<pregunta>"` en un `batch`).
    #[serde(default)]
    cassette: Option<Recordings>,
}

/// El nombre con el que un borrador firma sus eventos: así la TUI no cree que el flujo cambió.
fn borrador(flow: &str) -> String {
    format!("{flow} (borrador)")
}

/// `POST /v1/flows/draft/run`: ejecuta el borrador una vez, sin publicarlo.
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
    // El transporte se elige antes de gastar nada: un nombre desconocido no llega al motor.
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
    // La confianza del borrador es la del último nodo que respondió; `null` si ninguno lo hizo.
    respuesta["confidence"] = json!(io.measured().2);
    if req.transport == "record" {
        respuesta["cassette"] = json!(grabado.lock().unwrap_or_else(PoisonError::into_inner).clone());
    }
    (estado, Json(respuesta)).into_response()
}
