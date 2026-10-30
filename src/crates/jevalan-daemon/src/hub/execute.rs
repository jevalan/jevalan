//! Core of `POST /v1/hub` without HTTP: shared by the route and the laboratory.

use crate::AppState;
use crate::bus::{Tracer, new_trace_id};
use crate::hub::provider::{Invoked, invoke};
use crate::hub::transport::{Io, Transport};
use jevalan_events::v2::EventKind;
use jevalan_flow::Flow;
use jevalan_hub::capability::{Capability, Provider, valid_capability_name};
use jevalan_hub::classify::{ErrorCode, Status, response_body};
use jevalan_hub::envelope::{HubRequest, effective_deadline};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Measure {
    pub elapsed_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Probability the model gave to the answer it gave in the last Jev node: `max(p, 1 − p)` for noul,
    /// `probabilities[choice]` for choice (or the `confidence` if it sent none) and the `confidence` for score.
    pub confidence: Option<f64>,
    /// Calls to backends and, of those, the ones that did not bring complete `usage`.
    pub backend_calls: u32,
    pub unmetered_calls: u32,
    /// Llamadas a proveedores `exec`/`http`.
    pub specialist_calls: u32,
    /// Sum of `usage.cost` (USD) if all the calls declared it.
    pub reported_cost_usd: Option<f64>,
}

pub struct Executed {
    pub response: Value,
    pub measure: Measure,
    /// Key of the first recording missing in `Replay`.
    pub missing: Option<String>,
}

pub async fn execute(
    state: &AppState,
    req: &HubRequest,
    cap: Option<&Capability>,
    flow_override: Option<Arc<Flow>>,
    transport: &Transport,
) -> Executed {
    execute_as(state, req, cap, flow_override, transport, "lab").await
}

/// Like `execute`, with the client that signs the `HUB_REQUEST` event.
pub async fn execute_as(
    state: &AppState,
    req: &HubRequest,
    cap: Option<&Capability>,
    flow_override: Option<Arc<Flow>>,
    transport: &Transport,
    client: &str,
) -> Executed {
    let started = Instant::now();
    let provider_id = cap.map_or("-", |c| c.provider_id.as_str()).to_string();
    let tracer = state.bus.tracer(new_trace_id());
    // Only capability-shaped names reach the bus: no arbitrary client strings.
    let label = if req.capability.len() <= 128 && valid_capability_name(&req.capability) {
        req.capability.as_str()
    } else {
        "?"
    };
    tracer.emit(EventKind::HubRequest {
        capability: label.into(),
        capability_version: req.capability_version.clone(),
        provider: provider_id.clone(),
        client: client.into(),
    });

    if let Some(recorder) = &state.recorder {
        recorder.input(tracer.trace_id(), client, &req.capability, &req.input);
    }
    let io = Io::new(transport).recording(state, tracer.trace_id());
    let Invoked { status, path } = match cap {
        Some(cap) => resolve(state, req, cap, &tracer, flow_override, &io).await,
        None => {
            let msg = format!("capacidad desconocida: {}", req.capability);
            Invoked { status: Status::error(ErrorCode::UnknownCapability, msg), path: None }
        }
    };

    // A missing recording cuts the run even if the flow would have absorbed it through its error port.
    let status = match io.missing() {
        Some(key) if !matches!(status, Status::Error { .. }) => {
            Status::error(ErrorCode::ProviderFailed, format!("missing_recording:{key}"))
        }
        _ => status,
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if let Some(recorder) = &state.recorder {
        let (decision, code) = match &status {
            Status::Ok(decision) => (Some(decision), None),
            Status::Error { code, .. } => (None, Some(code.as_str())),
            Status::Abstain(_) => (None, None),
        };
        recorder.decision(tracer.trace_id(), &req.capability, status.as_str(), decision, code);
    }
    let code = match &status {
        Status::Error { code, .. } => Some(code.as_str().to_string()),
        _ => None,
    };
    tracer.emit(EventKind::HubResponse {
        capability: label.into(),
        provider: provider_id.clone(),
        status: status.as_str().into(),
        code,
        elapsed_ms,
    });
    let mut trace = json!({ "trace_id": tracer.trace_id(), "elapsed_ms": elapsed_ms, "cost": "not_measured" });
    if let Some(path) = path {
        trace["path"] = json!(path);
    }
    let version = cap.map_or("-".to_string(), provider_version);
    let response = response_body(req, &status, &provider_id, &version, trace);
    let (input_tokens, output_tokens, confidence) = io.measured();
    let (backend_calls, unmetered_calls) = io.calls();
    let reported_cost_usd = io.reported_cost();
    let measure = Measure {
        elapsed_ms,
        input_tokens,
        output_tokens,
        confidence,
        backend_calls,
        unmetered_calls,
        specialist_calls: io.specialist_calls(),
        reported_cost_usd,
    };
    Executed { response, measure, missing: io.missing() }
}

/// Version, input, deadline, provider and decision, in that order.
async fn resolve(
    state: &AppState,
    req: &HubRequest,
    cap: &Capability,
    tracer: &Tracer,
    flow_override: Option<Arc<Flow>>,
    io: &Io<'_>,
) -> Invoked {
    let reject = |code, message: String| Invoked { status: Status::error(code, message), path: None };
    if req.capability_version != cap.version {
        return reject(ErrorCode::VersionMismatch, format!("la capacidad está en la versión {}", cap.version));
    }
    if let Err(e) = cap.check_input(&req.input) {
        return reject(ErrorCode::InvalidInput, e);
    }
    let deadline = Duration::from_millis(effective_deadline(req.constraints.deadline_ms, cap.deadline_ms));
    let mut invoked = invoke(state, cap, &req.input, tracer, deadline, flow_override, io).await;
    if let Status::Ok(decision) = &invoked.status
        && let Err(e) = cap.check_decision(decision)
    {
        invoked.status = Status::error(ErrorCode::InvalidDecision, e);
    }
    invoked
}

fn provider_version(cap: &Capability) -> String {
    match cap.provider {
        Provider::Flow { .. } => env!("CARGO_PKG_VERSION").to_string(),
        _ => cap.provider_version.clone().unwrap_or_else(|| "unconfigured".into()),
    }
}
