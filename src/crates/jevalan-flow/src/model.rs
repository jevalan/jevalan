use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Flow file as written in YAML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowDef {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub defaults: Defaults,
    pub start: String,
    pub nodes: BTreeMap<String, NodeDef>,
    pub outputs: BTreeMap<String, OutputDef>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    Choice,
    Score,
    Noul,
    Rule,
    /// A handful of questions in a single call to the backend; it asks nothing by itself.
    Batch,
}

impl NodeType {
    /// Name of the type and, at the same time, of the System One response field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Choice => "choice",
            Self::Score => "score",
            Self::Noul => "noul",
            Self::Rule => "rule",
            Self::Batch => "batch",
        }
    }
}

/// A routing rule of a `batch` node: `when` over the input and the answers, `to` its destination.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteDef {
    pub when: String,
    pub to: String,
}

/// `criteria` exactly as sent to System One: list of levels (score) or options (choice, noul).
// ponytail: BTreeMap sorts the options; System One only uses the order to break ties
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Criteria {
    Levels(Vec<String>),
    Options(BTreeMap<String, Option<String>>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDef {
    #[serde(rename = "type")]
    pub ty: NodeType,
    #[serde(default)]
    pub instructions: String,
    /// `rule` only: a CEL expression that returns a boolean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<Criteria>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f64>,
    /// `batch` only: questions in a single call, each with the body of a standalone node.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub questions: BTreeMap<String, NodeDef>,
    /// `batch` only: routing rules, in order; the first true `when` wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route: Vec<RouteDef>,
    /// `batch` only: destination if no `when` matches. In YAML it is written `else`, which in Rust is a reserved word.
    #[serde(default, rename = "else", skip_serializing_if = "Option::is_none")]
    pub else_branch: Option<String>,
    /// `batch` only: signals learned from the catalogue, which are asked as questions with the same name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<String>,
    /// Port → destination (node or output). Absent means empty: a `batch` question wires no ports and a
    /// standalone node without them is reported by `validate` with `MissingPort`, which is where it belongs.
    #[serde(default)]
    pub out: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputDef {
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
