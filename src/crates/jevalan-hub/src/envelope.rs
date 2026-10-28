use crate::PROTOCOL;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HubRequest {
    pub protocol_version: String,
    pub request_id: String,
    pub capability: String,
    pub capability_version: String,
    pub input: Value,
    #[serde(default)]
    pub constraints: Constraints,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Constraints {
    pub deadline_ms: Option<u64>,
    pub max_cost_usd: Option<f64>,
}

/// Envoltura legible y en la versión del protocolo; si no, el motivo (será un 400).
pub fn parse_request(body: &[u8]) -> Result<HubRequest, String> {
    let req: HubRequest = serde_json::from_slice(body).map_err(|e| format!("envoltura inválida: {e}"))?;
    if req.protocol_version != PROTOCOL {
        return Err(format!("protocol_version debe ser {PROTOCOL}"));
    }
    if req.request_id.is_empty() || req.request_id.chars().count() > 128 {
        return Err("request_id debe tener entre 1 y 128 caracteres".into());
    }
    Ok(req)
}

/// El plazo pedido nunca amplía el de la capacidad.
pub fn effective_deadline(requested: Option<u64>, default: u64) -> u64 {
    requested.map_or(default, |r| r.min(default))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"{"protocol_version":"jev-hub/0.1","request_id":"r1","capability":"a.b","capability_version":"1","input":{}}"#;

    #[test]
    fn parses_a_minimal_request() {
        let r = parse_request(OK.as_bytes()).unwrap();
        assert_eq!((r.capability.as_str(), r.constraints.deadline_ms), ("a.b", None));
    }

    #[test]
    fn rejects_other_protocols_and_bad_ids() {
        assert!(parse_request(OK.replace("0.1", "9.9").as_bytes()).is_err());
        assert!(parse_request(OK.replace("\"r1\"", "\"\"").as_bytes()).is_err());
        assert!(parse_request(OK.replace("r1", &"x".repeat(129)).as_bytes()).is_err());
        assert!(parse_request(b"not json").is_err());
    }

    #[test]
    fn deadline_never_grows() {
        assert_eq!(effective_deadline(Some(800), 3000), 800);
        assert_eq!(effective_deadline(Some(9000), 3000), 3000);
        assert_eq!(effective_deadline(None, 3000), 3000);
    }
}
