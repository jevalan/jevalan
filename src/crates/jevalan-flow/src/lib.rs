//! Flujos de decisión sobre System One: modelo, validación y ejecución. Lógica pura.

mod model;
mod program;
mod run;
mod signals;
mod validate;

pub use model::{Criteria, Defaults, FlowDef, NodeDef, NodeType, OutputDef, RouteDef};
pub use run::{
    Next, PathStep, Reply, Run, binary_confidence, question, questions, reply_from_response, request_body,
    request_body_many, resolve_refs,
};
pub use signals::{Signals, parse_signal};
pub use validate::{Flow, FlowError, Node, answer_ports, parse, parse_with, validate, validate_with};

/// Backend y modelo simbólicos de un nodo rule: no consulta a nadie.
pub const RULE_BACKEND: &str = "-";
pub const DEFAULT_THRESHOLD: f64 = 0.5;
pub const DEFAULT_MIN_CONFIDENCE: f64 = 0.5;
/// Puerto común: el backend falló o devolvió algo inservible.
pub const ERROR: &str = "error";
/// Puerto común: la confianza no alcanzó `min_confidence`.
pub const LOW_CONFIDENCE: &str = "low-confidence";
