//! Decision flows over System One: model, validation and execution. Pure logic.

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
pub use validate::{
    Flow, FlowError, Node, answer_ports, parse, parse_routed, parse_with, validate, validate_routed, validate_with,
};

/// Symbolic backend and model of a rule node: it consults nobody.
pub const RULE_BACKEND: &str = "-";
pub const DEFAULT_THRESHOLD: f64 = 0.5;
pub const DEFAULT_MIN_CONFIDENCE: f64 = 0.5;
/// Common port: the backend failed or returned something unusable.
pub const ERROR: &str = "error";
/// Common port: the confidence did not reach `min_confidence`.
pub const LOW_CONFIDENCE: &str = "low-confidence";
