//! `POST /v1/hub` y `GET /v1/hub/capabilities`.

use crate::AppState;
use crate::hub::execute::execute_as;
use crate::hub::transport::Transport;
use crate::proxy::error_response;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use jevalan_hub::envelope::parse_request;
use serde_json::Value;

pub async fn capabilities(State(state): State<AppState>) -> Json<Value> {
    Json(state.capabilities.list())
}

pub async fn hub(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let req = match parse_request(&body) {
        Ok(req) => req,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
    };
    let client = headers.get("x-jevalan-client").and_then(|v| v.to_str().ok()).unwrap_or("unknown");
    let cap = state.capabilities.get(&req.capability);
    let executed = execute_as(&state, &req, cap.as_deref(), None, &Transport::Live, client).await;
    Json(executed.response).into_response()
}
