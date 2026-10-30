//! `POST /v1/decide/{flow}` and `GET /v1/flows`: drives a flow node by node against its backends.

use crate::AppState;
use crate::bus::{Tracer, new_trace_id};
use crate::flowrun::{Finish, run_flow};
use crate::hub::transport::{Io, Transport};
use crate::proxy::error_response;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use jevalan_events::v2::{Endpoint, EventKind};
use jevalan_flow::Flow;
use serde_json::{Value, json};

const MAX_CLIENT_TRACE_ID: usize = 128;

pub async fn flows(State(state): State<AppState>) -> Json<Value> {
    Json(state.flows.list())
}

pub async fn decide(State(state): State<AppState>, Path(name): Path<String>, body: Bytes) -> Response {
    // The `Arc` is cloned: a reload halfway through does not alter this run.
    let Some(flow) = state.flows.get(&name) else {
        return error_response(StatusCode::NOT_FOUND, &format!("flujo desconocido: {name}"));
    };
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("JSON inválido: {e}")),
    };
    let Some(input) = req.get("state") else {
        return error_response(StatusCode::BAD_REQUEST, "falta «state»");
    };
    let client = req["client"].as_str().unwrap_or("unknown").to_string();
    let who = client.clone();
    // The trace is always our own; the client's identifier is only returned, never enters the events.
    let client_trace_id = match req.get("client_trace_id") {
        None => None,
        Some(Value::String(t)) if t.chars().count() <= MAX_CLIENT_TRACE_ID => Some(t.clone()),
        Some(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "«client_trace_id» debe ser un string de hasta 128 caracteres",
            );
        }
    };

    let tracer = state.bus.tracer(new_trace_id());
    tracer.emit(EventKind::RequestReceived {
        client,
        endpoint: Endpoint::Decide,
        flow: Some(name.clone()),
        backend: None,
    });
    if let Some(recorder) = &state.recorder {
        recorder.input(tracer.trace_id(), &who, &format!("flow:{name}"), input);
    }
    drive(&state, &name, &flow, input, &tracer, client_trace_id).await
}

async fn drive(
    state: &AppState,
    name: &str,
    flow: &Flow,
    input: &Value,
    tracer: &Tracer,
    echo: Option<String>,
) -> Response {
    let io = Io::new(&Transport::Live).recording(state, tracer.trace_id());
    let ran = run_flow(state, name, flow, input, tracer, &io).await;
    if let Some(recorder) = &state.recorder {
        let capability = format!("flow:{name}");
        match &ran.finish {
            Finish::Done { outcome, data } => {
                let decision = json!({ "outcome": outcome, "data": data });
                recorder.decision(tracer.trace_id(), &capability, "ok", Some(&decision), None);
            }
            Finish::Failed { .. } => {
                recorder.decision(tracer.trace_id(), &capability, "error", None, Some("flow_failed"))
            }
        }
    }
    let (status, mut reply) = match ran.finish {
        Finish::Done { outcome, data } => {
            let mut reply =
                json!({ "flow": name, "outcome": outcome, "trace_id": tracer.trace_id(), "path": ran.path });
            if let Some(data) = data {
                reply["data"] = data;
            }
            (StatusCode::OK, reply)
        }
        Finish::Failed { message } => {
            let reply = json!({ "error": message, "flow": name, "trace_id": tracer.trace_id(), "path": ran.path });
            (StatusCode::BAD_GATEWAY, reply)
        }
    };
    if let Some(id) = echo {
        reply["client_trace_id"] = id.into();
    }
    (status, Json(reply)).into_response()
}
