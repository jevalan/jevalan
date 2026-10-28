use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Fichero de flujo tal como se escribe en YAML.
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
    /// Un puñado de preguntas en una sola llamada al backend; no pregunta nada por sí mismo.
    Batch,
}

impl NodeType {
    /// Nombre del tipo y, a la vez, del campo de la respuesta System One.
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

/// Una regla de enrutado de un nodo `batch`: `when` sobre la entrada y las respuestas, `to` su destino.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteDef {
    pub when: String,
    pub to: String,
}

/// `criteria` tal cual se envía a System One: lista de niveles (score) u opciones (choice, noul).
// ponytail: BTreeMap ordena las opciones; System One solo usa el orden para desempatar
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
    /// Solo `rule`: expresión CEL que devuelve un booleano.
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
    /// Solo `batch`: preguntas en una sola llamada, cada una con el cuerpo de un nodo suelto.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub questions: BTreeMap<String, NodeDef>,
    /// Solo `batch`: reglas de enrutado, en orden; gana la primera `when` verdadera.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route: Vec<RouteDef>,
    /// Solo `batch`: destino si ninguna `when` casa. En YAML se escribe `else`, que en Rust es palabra reservada.
    #[serde(default, rename = "else", skip_serializing_if = "Option::is_none")]
    pub else_branch: Option<String>,
    /// Solo `batch`: señales aprendidas del catálogo, que se preguntan como preguntas con su mismo nombre.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<String>,
    /// Puerto → destino (nodo o salida). Ausente vale vacío: una pregunta de `batch` no cablea puertos y un
    /// nodo suelto sin ellos lo denuncia `validate` con `MissingPort`, que es donde toca.
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
