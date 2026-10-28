//! Passthrough transparente de System One hacia el backend elegido.

use crate::AppState;
use crate::bus::new_trace_id;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jevalan_events::v2::{Endpoint, EventKind};
use serde_json::{Value, json};
use std::time::Instant;

pub async fn named(
    State(state): State<AppState>,
    Path(backend): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    passthrough(state, backend, headers, body).await
}

pub async fn default(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let backend = state.cfg.default_backend.clone();
    passthrough(state, backend, headers, body).await
}

async fn passthrough(state: AppState, backend: String, headers: HeaderMap, body: Bytes) -> Response {
    // El nombre solo sirve de clave en la tabla configurada; jamás se interpreta como URL.
    if !state.cfg.backends.contains_key(&backend) {
        return error_response(StatusCode::NOT_FOUND, &format!("backend desconocido: {backend}"));
    }
    let client = headers.get("x-jevalan-client").and_then(|v| v.to_str().ok()).unwrap_or("unknown").to_string();
    let sent: Option<Value> = serde_json::from_slice(&body).ok();
    let model = sent.as_ref().and_then(|v| v["model"].as_str()).unwrap_or("?").to_string();
    let questions = sent.as_ref().and_then(|v| v["questions"].as_object()).map_or(0, |q| q.len() as u32);

    let tracer = state.bus.tracer(new_trace_id());
    tracer.emit(EventKind::RequestReceived {
        client,
        endpoint: Endpoint::Systemone,
        flow: None,
        backend: Some(backend.clone()),
    });
    tracer.emit(EventKind::BackendRequest { backend: backend.clone(), model: model.clone(), questions });

    let started = Instant::now();
    let result = call_backend(&state, &backend, body).await;
    let duration_ms = started.elapsed().as_millis() as u64;

    let (status, reply) = match result {
        Ok((status, bytes)) => {
            let input_tokens =
                serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v["usage"]["input_tokens"].as_u64());
            let ok = (200..300).contains(&status);
            tracer.emit(EventKind::BackendResponse { backend: backend.clone(), ok, status, duration_ms, input_tokens });
            (status, json_response(status, bytes))
        }
        Err(message) => {
            let failed = EventKind::BackendResponse {
                backend: backend.clone(),
                ok: false,
                status: 0,
                duration_ms,
                input_tokens: None,
            };
            tracer.emit(failed);
            (502, error_response(StatusCode::BAD_GATEWAY, &message))
        }
    };
    tracer.emit(EventKind::PassthroughDone { backend, model, status, duration_ms });
    reply
}

/// Envía `body` sin tocarlo a `<url><path>` (por defecto `/v1/systemone`). Solo viajan `Content-Type` y, si la hay, la clave configurada.
pub async fn call_backend(state: &AppState, backend: &str, body: Bytes) -> Result<(u16, Bytes), String> {
    let cfg = state.cfg.backends.get(backend).ok_or_else(|| format!("backend desconocido: {backend}"))?;
    let mut req = state
        .http
        .post(format!("{}{}", cfg.url, cfg.path))
        .timeout(cfg.timeout)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body);
    if let Some(key) = &cfg.api_key {
        req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
    }
    let resp = req.send().await.map_err(|e| e.without_url().to_string())?;
    let status = resp.status().as_u16();
    let bytes = resp.bytes().await.map_err(|e| e.without_url().to_string())?;
    Ok((status, bytes))
}

fn json_response(status: u16, body: Bytes) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

pub fn error_response(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(json!({ "error": message }))).into_response()
}
