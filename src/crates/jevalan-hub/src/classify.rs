use crate::PROTOCOL;
use crate::envelope::HubRequest;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    UnknownCapability,
    VersionMismatch,
    InvalidInput,
    DeadlineExceeded,
    ProviderFailed,
    ProviderDisabled,
    InvalidDecision,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnknownCapability => "unknown_capability",
            Self::VersionMismatch => "version_mismatch",
            Self::InvalidInput => "invalid_input",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ProviderFailed => "provider_failed",
            Self::ProviderDisabled => "provider_disabled",
            Self::InvalidDecision => "invalid_decision",
        }
    }

    /// The inverse of `as_str`.
    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::UnknownCapability,
            Self::VersionMismatch,
            Self::InvalidInput,
            Self::DeadlineExceeded,
            Self::ProviderFailed,
            Self::ProviderDisabled,
            Self::InvalidDecision,
        ]
        .into_iter()
        .find(|c| c.as_str() == code)
    }

    pub fn retryable(self) -> bool {
        matches!(self, Self::DeadlineExceeded | Self::ProviderFailed)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Ok(Value),
    Abstain(Option<String>),
    Error { code: ErrorCode, message: String },
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok(_) => "ok",
            Self::Abstain(_) => "abstain",
            Self::Error { .. } => "error",
        }
    }

    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Error { code, message: message.into() }
    }
}

/// Response of an `exec`/`http` provider. `error_when` takes precedence over `abstain_when`; an `error_when` that
/// cannot be evaluated fails closed (error), whereas an unevaluable `abstain_when` counts as false.
pub fn classify(response: Value, abstain_when: Option<&str>, error_when: Option<&str>) -> Status {
    match error_when.map(|e| eval(e, &response)) {
        Some(Some(true)) => return Status::error(ErrorCode::ProviderFailed, "el especialista informó de un fallo"),
        Some(None) => return Status::error(ErrorCode::ProviderFailed, "no se pudo evaluar error_when"),
        _ => {}
    }
    let reason = || response.get("reason").and_then(Value::as_str).map(String::from);
    match abstain_when {
        Some(e) if holds(e, &response) => Status::Abstain(reason()),
        Some(_) => Status::Ok(response),
        None if response.get("status").and_then(Value::as_str) == Some("abstain") => Status::Abstain(reason()),
        None => Status::Ok(response),
    }
}

/// `flow` provider: `outcome: abstain` abstains; anything else is a decision with the output's `data`.
pub fn from_flow(outcome: &str, data: Option<Value>) -> Status {
    if outcome == "abstain" {
        Status::Abstain(data.as_ref().and_then(|d| d.get("reason")).and_then(Value::as_str).map(String::from))
    } else {
        Status::Ok(data.unwrap_or_else(|| json!({})))
    }
}

fn holds(expr: &str, response: &Value) -> bool {
    eval(expr, response) == Some(true)
}

/// `None` if the expression does not compile, cannot be evaluated or does not yield a boolean.
fn eval(expr: &str, response: &Value) -> Option<bool> {
    let program = crate::program::compiled(expr).ok()?;
    let mut ctx = cel::Context::default();
    ctx.add_variable("response", response.clone()).ok()?;
    match program.execute(&ctx) {
        Ok(cel::Value::Bool(b)) => Some(b),
        _ => None,
    }
}

/// Body of the `jev-hub/0.1` response. `abstain` and `error` never carry `decision`.
pub fn response_body(
    req: &HubRequest,
    status: &Status,
    provider_id: &str,
    provider_version: &str,
    trace: Value,
) -> Value {
    let mut body = json!({
        "protocol_version": PROTOCOL,
        "request_id": req.request_id,
        "status": status.as_str(),
        "capability": req.capability,
        "capability_version": req.capability_version,
        "provider": { "id": provider_id, "version": provider_version },
        "trace": trace,
    });
    match status {
        Status::Ok(decision) => body["decision"] = decision.clone(),
        Status::Abstain(reason) => body["abstain"] = json!({ "reason": reason }),
        Status::Error { code, message } => {
            body["error"] = json!({ "code": code.as_str(), "retryable": code.retryable(), "message": message })
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::parse_request;

    const TG_ABSTAIN: &str = "response.source == 'no-opinion'";
    const TG_ERROR: &str = "response.source == 'fail-mode'";

    #[test]
    fn toolgate_shapes_map_to_statuses() {
        let allow = json!({"verdict": "allow", "reason": "ok", "source": "model"});
        assert_eq!(classify(allow.clone(), Some(TG_ABSTAIN), Some(TG_ERROR)), Status::Ok(allow));
        let none = json!({"verdict": "passthrough", "reason": "n/a", "source": "no-opinion"});
        assert_eq!(classify(none, Some(TG_ABSTAIN), Some(TG_ERROR)), Status::Abstain(Some("n/a".into())));
        let fail = json!({"verdict": "ask", "reason": "down", "source": "fail-mode"});
        assert!(matches!(
            classify(fail, Some(TG_ABSTAIN), Some(TG_ERROR)),
            Status::Error { code: ErrorCode::ProviderFailed, .. }
        ));
    }

    #[test]
    fn missing_abstain_field_counts_as_false() {
        let r = json!({"verdict": "allow"});
        assert_eq!(classify(r.clone(), Some(TG_ABSTAIN), None), Status::Ok(r));
    }

    #[test]
    fn error_when_that_cannot_be_evaluated_fails_closed() {
        let r = json!({"verdict": "allow"});
        assert!(matches!(
            classify(r, Some(TG_ABSTAIN), Some(TG_ERROR)),
            Status::Error { code: ErrorCode::ProviderFailed, .. }
        ));
    }

    #[test]
    fn default_abstain_convention() {
        assert_eq!(
            classify(json!({"status": "abstain", "reason": "r"}), None, None),
            Status::Abstain(Some("r".into()))
        );
    }

    #[test]
    fn flow_outcomes() {
        assert_eq!(from_flow("ok", Some(json!({"wake": true}))), Status::Ok(json!({"wake": true})));
        assert_eq!(from_flow("abstain", None), Status::Abstain(None));
    }

    #[test]
    fn abstain_and_error_never_carry_a_decision() {
        let req = parse_request(br#"{"protocol_version":"jev-hub/0.1","request_id":"r","capability":"a.b","capability_version":"1","input":{}}"#).unwrap();
        for s in [Status::Abstain(None), Status::error(ErrorCode::DeadlineExceeded, "x")] {
            let body = response_body(&req, &s, "p", "1", json!({}));
            assert!(body.get("decision").is_none(), "{body}");
        }
        let body = response_body(&req, &Status::error(ErrorCode::DeadlineExceeded, "x"), "p", "1", json!({}));
        assert_eq!(
            (body["error"]["code"].as_str(), body["error"]["retryable"].as_bool()),
            (Some("deadline_exceeded"), Some(true))
        );
    }
}
