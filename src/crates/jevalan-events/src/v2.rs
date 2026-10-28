//! Contrato de eventos v2: System One, flujos de decisión y passthrough.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub v: u32,
    pub id: String,
    pub ts: String,
    pub trace_id: String,
    pub seq: u64,
    pub source: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Endpoint {
    Decide,
    Systemone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventKind {
    RequestReceived {
        client: String,
        endpoint: Endpoint,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flow: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backend: Option<String>,
    },
    NodeStart {
        flow: String,
        node: String,
        #[serde(rename = "type")]
        node_type: String,
        backend: String,
        model: String,
    },
    BackendRequest {
        backend: String,
        model: String,
        questions: u32,
    },
    BackendResponse {
        backend: String,
        ok: bool,
        /// 0 si no hubo respuesta HTTP (timeout, conexión rechazada).
        status: u16,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_tokens: Option<u64>,
    },
    NodeEnd {
        flow: String,
        node: String,
        /// Número para noul y score, texto para choice; `null` si el backend falló.
        answer: Value,
        confidence: f64,
        port: String,
    },
    /// Lo que decidió un paso sin preguntar a nadie: las heurísticas que leyó y la regla `route` que ganó.
    /// Solo nombres y booleanos, nunca la entrada.
    NodeAudit {
        flow: String,
        node: String,
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        signals: serde_json::Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<u32>,
    },
    DecisionMade {
        flow: String,
        outcome: String,
        path_len: u32,
        duration_ms: u64,
    },
    PassthroughDone {
        backend: String,
        model: String,
        status: u16,
        duration_ms: u64,
    },
    /// Nombres de los backends configurados, nunca sus urls ni claves.
    Backends {
        backends: Vec<String>,
    },
    FlowLoaded {
        flow: String,
        nodes: u32,
        definition: Value,
    },
    FlowInvalid {
        flow: String,
        reason: String,
    },
    FlowRemoved {
        flow: String,
    },
    HubRequest {
        capability: String,
        capability_version: String,
        provider: String,
        client: String,
    },
    HubResponse {
        capability: String,
        provider: String,
        /// `ok`, `abstain` o `error`.
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        elapsed_ms: u64,
    },
    CapabilityLoaded {
        capability: String,
        version: String,
        provider: String,
        /// `flow`, `exec` o `http`.
        provider_type: String,
        /// Flujo que sirve la capacidad, si el proveedor es un flujo.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flow: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        description: String,
        /// Claves de primer nivel de `properties` del esquema de entrada, ordenadas.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        input_fields: Vec<String>,
        /// Ídem para el esquema de decisión.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decision_fields: Vec<String>,
    },
    CapabilityInvalid {
        capability: String,
        reason: String,
    },
    CapabilityRemoved {
        capability: String,
    },
    Error {
        stage: String,
        message: String,
    },
    Lagged {
        dropped: u64,
    },
}

impl EventKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::RequestReceived { .. } => "REQUEST_RECEIVED",
            Self::NodeStart { .. } => "NODE_START",
            Self::BackendRequest { .. } => "BACKEND_REQUEST",
            Self::BackendResponse { .. } => "BACKEND_RESPONSE",
            Self::NodeEnd { .. } => "NODE_END",
            Self::NodeAudit { .. } => "NODE_AUDIT",
            Self::DecisionMade { .. } => "DECISION_MADE",
            Self::PassthroughDone { .. } => "PASSTHROUGH_DONE",
            Self::Backends { .. } => "BACKENDS",
            Self::FlowLoaded { .. } => "FLOW_LOADED",
            Self::FlowInvalid { .. } => "FLOW_INVALID",
            Self::FlowRemoved { .. } => "FLOW_REMOVED",
            Self::HubRequest { .. } => "HUB_REQUEST",
            Self::HubResponse { .. } => "HUB_RESPONSE",
            Self::CapabilityLoaded { .. } => "CAPABILITY_LOADED",
            Self::CapabilityInvalid { .. } => "CAPABILITY_INVALID",
            Self::CapabilityRemoved { .. } => "CAPABILITY_REMOVED",
            Self::Error { .. } => "ERROR",
            Self::Lagged { .. } => "LAGGED",
        }
    }
}

/// Evento v2 con `id = "<trace_id>-<seq>"` y `ts` de ahora.
pub fn new_event(trace_id: &str, seq: u64, source: &str, kind: EventKind) -> Event {
    Event {
        v: SCHEMA_VERSION,
        id: format!("{trace_id}-{seq}"),
        ts: rfc3339(SystemTime::now()),
        trace_id: trace_id.to_string(),
        seq,
        source: source.to_string(),
        kind,
    }
}

/// Fecha UTC en RFC3339 con precisión de segundos.
pub fn rfc3339(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil_from_days, de Howard Hinnant.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Duration;

    const FIXTURES: [&str; 19] = [
        "request_received",
        "node_start",
        "backend_request",
        "backend_response",
        "node_end",
        "node_audit",
        "decision_made",
        "passthrough_done",
        "backends",
        "flow_loaded",
        "flow_invalid",
        "flow_removed",
        "hub_request",
        "hub_response",
        "capability_loaded",
        "capability_invalid",
        "capability_removed",
        "error",
        "lagged",
    ];

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../config/schema/fixtures/v2")
            .join(format!("{name}.json"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn every_fixture_round_trips() {
        for name in FIXTURES {
            let raw = fixture(name);
            let event: Event = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(serde_json::to_value(&event).unwrap(), serde_json::from_str::<Value>(&raw).unwrap(), "{name}");
        }
    }

    #[test]
    fn name_matches_the_json_type() {
        for name in FIXTURES {
            let raw = fixture(name);
            let event: Event = serde_json::from_str(&raw).unwrap();
            let json: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(event.kind.name(), json["type"]);
        }
    }

    #[test]
    fn node_start_serializes_its_type_field() {
        let e = new_event(
            "t",
            2,
            "daemon",
            EventKind::NodeStart {
                flow: "f".into(),
                node: "n".into(),
                node_type: "choice".into(),
                backend: "ollama".into(),
                model: "nimble".into(),
            },
        );
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["type"], "NODE_START");
        assert_eq!(json["payload"]["type"], "choice");
        assert_eq!(json["id"], "t-2");
        assert_eq!(json["v"], 2);
    }

    #[test]
    fn optional_fields_are_omitted() {
        let e = new_event(
            "t",
            1,
            "daemon",
            EventKind::RequestReceived {
                client: "c".into(),
                endpoint: Endpoint::Systemone,
                flow: None,
                backend: Some("ollama".into()),
            },
        );
        let json = serde_json::to_value(&e).unwrap();
        assert!(json["payload"].get("flow").is_none());
        assert_eq!(json["payload"]["endpoint"], "systemone");
    }

    #[test]
    fn unknown_type_fails_to_deserialize() {
        let raw = r#"{"v":2,"id":"a","ts":"x","trace_id":"t","seq":1,"source":"s","type":"HOOK_START","payload":{}}"#;
        assert!(serde_json::from_str::<Event>(raw).is_err());
    }

    #[test]
    fn rfc3339_formats_a_known_instant() {
        let t = UNIX_EPOCH + Duration::from_secs(1_791_374_400);
        assert_eq!(rfc3339(t), "2026-10-07T12:00:00Z");
    }
}

#[cfg(test)]
mod node_audit_tests {
    use super::*;
    use serde_json::{Map, json};

    #[test]
    fn node_audit_viaja_sin_campos_vacios() {
        let kind = EventKind::NodeAudit { flow: "f".into(), node: "n".into(), signals: Map::new(), rule: None };
        assert_eq!(kind.name(), "NODE_AUDIT");
        let v = serde_json::to_value(new_event("t", 1, "s", kind)).unwrap();
        assert_eq!(
            (v["type"].clone(), v["payload"].clone()),
            (json!("NODE_AUDIT"), json!({ "flow": "f", "node": "n" }))
        );
    }
}
