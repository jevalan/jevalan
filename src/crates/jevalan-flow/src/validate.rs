use crate::model::{Criteria, FlowDef, NodeDef, NodeType, OutputDef};
use crate::{DEFAULT_MIN_CONFIDENCE, DEFAULT_THRESHOLD, ERROR, LOW_CONFIDENCE};

/// Reserved name: the TUI uses it as the fixed port for traffic without a flow.
const PASSTHROUGH: &str = "passthrough";
use crate::signals::Signals;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Validated flow, with the defaults already applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flow {
    pub name: String,
    pub description: String,
    pub start: String,
    pub nodes: BTreeMap<String, Node>,
    pub outputs: BTreeMap<String, OutputDef>,
    /// Heuristics the flow reads as `signals.<name>`: name → `when`, already validated on load.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub signals: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    #[serde(rename = "type")]
    pub ty: NodeType,
    #[serde(default)]
    pub instructions: String,
    /// `rule` only: CEL expression already compiled as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    pub criteria: Option<Criteria>,
    pub threshold: f64,
    pub backend: String,
    pub model: String,
    pub min_confidence: f64,
    /// Puerto → destino (nodo o salida).
    pub out: BTreeMap<String, String>,
    /// `score` only: ordered ranges `(from, to, port)`.
    pub ranges: Vec<(f64, f64, String)>,
    /// `batch` only: questions validated with the same rules as a standalone node.
    pub questions: BTreeMap<String, Node>,
    /// `batch` only: routing rules `(when, destination)`; the `when` already compiled on validation.
    pub route: Vec<(String, String)>,
    /// `batch` only: destination if no rule matches.
    pub else_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FlowError {
    Yaml(String),
    BadName(String),
    NameClash(String),
    UnknownStart(String),
    UnknownTarget { node: String, port: String, target: String },
    MissingPort { node: String, port: String },
    UnknownPort { node: String, port: String },
    BadCriteria { node: String, reason: String },
    BadExpression { node: String, reason: String },
    BadRanges { node: String, reason: String },
    OutOfRange { node: String, field: &'static str },
    BadReference { node: String, reference: String },
    UnknownSignal { node: String, signal: String },
    BadSignal { signal: String, reason: String },
    UnknownBackend { node: String, backend: String },
    MissingModel(String),
    Cycle(String),
    Unreachable(String),
}

impl fmt::Display for FlowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Yaml(e) => write!(f, "YAML inválido: {e}"),
            Self::BadName(n) => write!(f, "nombre de flujo inválido: {n:?} (usa [a-z0-9-])"),
            Self::NameClash(n) => write!(f, "{n:?} es a la vez nodo y salida"),
            Self::UnknownStart(n) => write!(f, "start apunta a {n:?}, que no existe"),
            Self::UnknownTarget { node, port, target } => write!(f, "{node}.{port} apunta a {target:?}, que no existe"),
            Self::MissingPort { node, port } => write!(f, "{node} no cablea el puerto {port:?}"),
            Self::UnknownPort { node, port } => write!(f, "{node} cablea un puerto desconocido: {port:?}"),
            Self::BadCriteria { node, reason } => write!(f, "{node}: {reason}"),
            Self::BadExpression { node, reason } => write!(f, "{node}: expresión CEL inválida: {reason}"),
            Self::BadRanges { node, reason } => write!(f, "{node}: rangos inválidos: {reason}"),
            Self::OutOfRange { node, field } => write!(f, "{node}: {field} debe estar en [0, 1]"),
            Self::BadReference { node, reference } => write!(f, "{node}: referencia desconocida {reference:?}"),
            Self::UnknownSignal { node, signal } => write!(f, "{node}: señal desconocida {signal:?}"),
            Self::BadSignal { signal, reason } => write!(f, "señal {signal}: {reason}"),
            Self::UnknownBackend { node, backend } => write!(f, "{node}: backend desconocido {backend:?}"),
            Self::MissingModel(n) => write!(f, "{n}: sin modelo (decláralo en el nodo o en defaults)"),
            Self::Cycle(n) => write!(f, "ciclo en el nodo {n}"),
            Self::Unreachable(n) => write!(f, "el nodo {n} no es alcanzable desde start"),
        }
    }
}

impl std::error::Error for FlowError {}

pub fn parse(name: &str, yaml: &str, backends: &[&str], default_backend: &str) -> Result<Flow, FlowError> {
    parse_with(name, yaml, &Signals::new(), backends, default_backend)
}

/// `parse` with the signal catalogue: what the daemon and the editor actually use.
pub fn parse_with(
    name: &str,
    yaml: &str,
    signals: &Signals,
    backends: &[&str],
    default_backend: &str,
) -> Result<Flow, FlowError> {
    let def: FlowDef = serde_yaml::from_str(yaml).map_err(|e| FlowError::Yaml(e.to_string()))?;
    validate_with(name, def, signals, backends, default_backend)
}

pub fn validate(name: &str, def: FlowDef, backends: &[&str], default_backend: &str) -> Result<Flow, FlowError> {
    validate_with(name, def, &Signals::new(), backends, default_backend)
}

/// Expands the signals with the catalogue and validates the result with the usual rules.
pub fn validate_with(
    name: &str,
    def: FlowDef,
    signals: &Signals,
    backends: &[&str],
    default_backend: &str,
) -> Result<Flow, FlowError> {
    let (def, heuristics) = crate::signals::resolve(def, signals)?;
    Ok(Flow { signals: heuristics, ..validate_resolved(name, def, backends, default_backend)? })
}

fn validate_resolved(name: &str, def: FlowDef, backends: &[&str], default_backend: &str) -> Result<Flow, FlowError> {
    // `passthrough` is the TUI's fixed port; no flow may claim it.
    if !valid_name(name) || name == PASSTHROUGH {
        return Err(FlowError::BadName(name.into()));
    }
    if let Some(n) = def.nodes.keys().find(|n| def.outputs.contains_key(*n)) {
        return Err(FlowError::NameClash(n.clone()));
    }
    if !def.nodes.contains_key(&def.start) && !def.outputs.contains_key(&def.start) {
        return Err(FlowError::UnknownStart(def.start.clone()));
    }
    let mut nodes = BTreeMap::new();
    for (n, d) in &def.nodes {
        nodes.insert(n.clone(), node(n, d, &def, backends, default_backend)?);
    }
    for (out_name, o) in &def.outputs {
        if let Some(data) = &o.data {
            check_refs(out_name, data, &nodes)?;
        }
    }
    check_graph(&def.start, &nodes)?;
    Ok(Flow {
        name: name.into(),
        description: def.description,
        start: def.start,
        nodes,
        outputs: def.outputs,
        signals: BTreeMap::new(),
    })
}

fn valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn node(name: &str, d: &NodeDef, def: &FlowDef, backends: &[&str], default_backend: &str) -> Result<Node, FlowError> {
    let bad = |reason: &str| FlowError::BadCriteria { node: name.into(), reason: reason.into() };
    if d.when.is_some() && d.ty != NodeType::Rule {
        return Err(bad("when solo vale para rule"));
    }
    if d.ty == NodeType::Rule {
        let when = d.when.clone().ok_or_else(|| bad("rule necesita when"))?;
        if let Err(e) = crate::program::compiled(&when) {
            return Err(FlowError::BadExpression { node: name.into(), reason: e.to_string() });
        }
        if d.criteria.is_some() || d.threshold.is_some() || d.backend.is_some() || d.model.is_some() {
            return Err(bad("rule solo admite when y out"));
        }
        check_ports(name, d, def, &answer_ports(d), false)?;
        return Ok(Node {
            ty: NodeType::Rule,
            instructions: String::new(),
            when: Some(when),
            criteria: None,
            threshold: DEFAULT_THRESHOLD,
            backend: crate::RULE_BACKEND.into(),
            model: crate::RULE_BACKEND.into(),
            min_confidence: 0.0,
            out: d.out.clone(),
            ranges: Vec::new(),
            questions: BTreeMap::new(),
            route: Vec::new(),
            else_branch: None,
        });
    }
    if d.ty == NodeType::Batch {
        return batch(name, d, def, backends, default_backend);
    }
    let backend =
        d.backend.clone().or_else(|| def.defaults.backend.clone()).unwrap_or_else(|| default_backend.to_string());
    if !backends.contains(&backend.as_str()) {
        return Err(FlowError::UnknownBackend { node: name.into(), backend });
    }
    let model =
        d.model.clone().or_else(|| def.defaults.model.clone()).ok_or_else(|| FlowError::MissingModel(name.into()))?;
    if d.threshold.is_some() && d.ty != NodeType::Noul {
        return Err(bad("threshold solo vale para noul"));
    }
    let threshold = d.threshold.unwrap_or(DEFAULT_THRESHOLD);
    let min_confidence = d.min_confidence.or(def.defaults.min_confidence).unwrap_or(DEFAULT_MIN_CONFIDENCE);
    for (field, v) in [("threshold", threshold), ("min_confidence", min_confidence)] {
        if !(0.0..=1.0).contains(&v) {
            return Err(FlowError::OutOfRange { node: name.into(), field });
        }
    }

    let mut ranges = Vec::new();
    let answer_ports: Vec<String> = match (d.ty, &d.criteria) {
        // The `true`/`false` criteria coexist with range outputs: they only travel to System One.
        (NodeType::Noul, c)
            if c.as_ref()
                .is_none_or(|c| matches!(c, Criteria::Options(m) if m.keys().all(|k| k == "true" || k == "false")))
                && d.out.keys().any(|p| p.contains("..")) =>
        {
            if d.threshold.is_some() {
                return Err(bad("un noul con rangos no admite threshold"));
            }
            ranges = ranges_for(name, &d.out, 1.0)?;
            ranges.iter().map(|(_, _, p)| p.clone()).collect()
        }
        (NodeType::Noul, None) => vec!["true".into(), "false".into()],
        (NodeType::Noul, Some(Criteria::Options(m))) if m.keys().all(|k| k == "true" || k == "false") => {
            vec!["true".into(), "false".into()]
        }
        (NodeType::Noul, _) => return Err(bad("noul solo admite criteria con claves \"true\" y \"false\"")),
        (NodeType::Choice, Some(Criteria::Options(m))) if m.keys().any(|k| k == ERROR || k == LOW_CONFIDENCE) => {
            return Err(bad("las opciones no pueden llamarse error ni low-confidence"));
        }
        (NodeType::Choice, Some(Criteria::Options(m))) if (2..=255).contains(&m.len()) => m.keys().cloned().collect(),
        (NodeType::Choice, _) => return Err(bad("choice necesita entre 2 y 255 opciones")),
        (NodeType::Score, Some(Criteria::Levels(l))) if (2..=26).contains(&l.len()) => {
            ranges = ranges_for(name, &d.out, (l.len() - 1) as f64)?;
            ranges.iter().map(|(_, _, p)| p.clone()).collect()
        }
        (NodeType::Score, _) => return Err(bad("score necesita entre 2 y 26 niveles")),
        (NodeType::Rule, _) => unreachable!("rule se valida antes"),
        (NodeType::Batch, _) => unreachable!("batch se valida antes"),
    };

    check_ports(name, d, def, &answer_ports, true)?;

    Ok(Node {
        ty: d.ty,
        instructions: d.instructions.clone(),
        when: None,
        criteria: d.criteria.clone(),
        threshold,
        backend,
        model,
        min_confidence,
        out: d.out.clone(),
        ranges,
        questions: BTreeMap::new(),
        route: Vec::new(),
        else_branch: None,
    })
}

/// `batch` node: several questions in a single call, routed by CEL rules and with no thresholds of its own.
fn batch(name: &str, d: &NodeDef, def: &FlowDef, backends: &[&str], default_backend: &str) -> Result<Node, FlowError> {
    let bad = |reason: &str| FlowError::BadCriteria { node: name.into(), reason: reason.into() };
    // D4: routing is done by the `route` rules, which see the raw answers; no threshold applies here.
    if d.threshold.is_some() || d.min_confidence.is_some() || d.out.contains_key(LOW_CONFIDENCE) {
        return Err(bad("batch no admite threshold, min_confidence ni low-confidence"));
    }
    if d.questions.is_empty() {
        return Err(bad("batch necesita al menos una pregunta"));
    }
    let mut questions = BTreeMap::new();
    for (q, qd) in &d.questions {
        if !cel_identifier(q) {
            return Err(FlowError::BadCriteria {
                node: format!("{name}.{q}"),
                reason: "el nombre de la pregunta debe ser un identificador CEL: [a-z_][a-z0-9_]*".into(),
            });
        }
        if matches!(qd.ty, NodeType::Batch | NodeType::Rule) {
            return Err(FlowError::BadCriteria {
                node: format!("{name}.{q}"),
                reason: "una pregunta no puede ser de tipo batch ni rule".into(),
            });
        }
        if !qd.out.is_empty() {
            return Err(FlowError::BadCriteria {
                node: format!("{name}.{q}"),
                reason: "una pregunta no cablea puertos: el destino lo pone route".into(),
            });
        }
        // Each question goes through the same rules as a standalone node, with the compound name for the error.
        // A standalone node requires its answer ports to be wired, so the question is validated with its own
        // (those its type admits) pointing at a fake destination that always exists; afterwards it is returned
        // without ports, because the real destination is set by the `route` rules.
        let wired: BTreeMap<String, String> = answer_ports(qd).into_iter().map(|p| (p, name.to_string())).collect();
        let wired = NodeDef { out: wired, ..qd.clone() };
        let question = node(&format!("{name}.{q}"), &wired, def, backends, default_backend)?;
        questions.insert(q.clone(), Node { out: BTreeMap::new(), ..question });
    }
    if d.route.is_empty() {
        return Err(bad("batch necesita al menos una regla route"));
    }
    let mut route = Vec::new();
    for r in &d.route {
        // The `when` is compiled on load: a typo in the route does not replace the flow that already works.
        crate::program::compiled(&r.when)
            .map_err(|e| FlowError::BadExpression { node: format!("{name}.route"), reason: e.to_string() })?;
        if !exists(&r.to, def) {
            return Err(FlowError::UnknownTarget { node: name.into(), port: "route".into(), target: r.to.clone() });
        }
        route.push((r.when.clone(), r.to.clone()));
    }
    let else_branch = match &d.else_branch {
        Some(e) if !exists(e, def) => {
            return Err(FlowError::UnknownTarget { node: name.into(), port: "else".into(), target: e.clone() });
        }
        Some(e) => Some(e.clone()),
        // Without `else`, the only possible exit is the error port: without it, the flow would not know where to go.
        None if !d.out.contains_key(ERROR) => {
            return Err(FlowError::MissingPort { node: name.into(), port: "else".into() });
        }
        None => None,
    };
    if let Some(port) = d.out.keys().find(|p| p.as_str() != ERROR) {
        return Err(FlowError::UnknownPort { node: name.into(), port: port.clone() });
    }
    if let Some(target) = d.out.get(ERROR)
        && !exists(target, def)
    {
        return Err(FlowError::UnknownTarget { node: name.into(), port: ERROR.into(), target: target.clone() });
    }
    let backend =
        d.backend.clone().or_else(|| def.defaults.backend.clone()).unwrap_or_else(|| default_backend.to_string());
    if !backends.contains(&backend.as_str()) {
        return Err(FlowError::UnknownBackend { node: name.into(), backend });
    }
    let model =
        d.model.clone().or_else(|| def.defaults.model.clone()).ok_or_else(|| FlowError::MissingModel(name.into()))?;
    Ok(Node {
        ty: NodeType::Batch,
        instructions: d.instructions.clone(),
        when: None,
        criteria: None,
        threshold: DEFAULT_THRESHOLD,
        backend,
        model,
        min_confidence: DEFAULT_MIN_CONFIDENCE,
        out: d.out.clone(),
        ranges: Vec::new(),
        questions,
        route,
        else_branch,
    })
}

/// Is `name` a CEL identifier, as a `batch` question requires?
pub(crate) fn cel_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Does `target` name a node or an output of the flow?
fn exists(target: &str, def: &FlowDef) -> bool {
    def.nodes.contains_key(target) || def.outputs.contains_key(target)
}

/// Answer ports a node of this shape admits: those `validate` requires to be wired, not
/// counting `error` and `low-confidence`, which are common. A `batch` wires no answer ports:
/// its `route` rules do the routing. The editor offers them so they can be wired (D6: a single authority).
pub fn answer_ports(d: &NodeDef) -> Vec<String> {
    match d.ty {
        // The same as a standalone `noul`: `true` and `false`, with or without criteria.
        NodeType::Noul | NodeType::Rule => vec!["true".into(), "false".into()],
        NodeType::Choice => match &d.criteria {
            Some(Criteria::Options(m)) => m.keys().cloned().collect(),
            _ => Vec::new(),
        },
        // One level per span `i..i+1`, like the ranges a `score` derives from its levels.
        NodeType::Score => match &d.criteria {
            Some(Criteria::Levels(l)) => (0..l.len().saturating_sub(1)).map(|i| format!("{i}..{}", i + 1)).collect(),
            _ => Vec::new(),
        },
        NodeType::Batch => Vec::new(),
    }
}

/// Ports of `d`: only the answer ones, `error` and `low-confidence` (if `allow_low_confidence`), and each destination exists.
fn check_ports(
    name: &str,
    d: &NodeDef,
    def: &FlowDef,
    puertos: &[String],
    allow_low_confidence: bool,
) -> Result<(), FlowError> {
    for (port, target) in &d.out {
        let common = port == ERROR || (allow_low_confidence && port == LOW_CONFIDENCE);
        if !common && !puertos.contains(port) {
            return Err(FlowError::UnknownPort { node: name.into(), port: port.clone() });
        }
        if !def.nodes.contains_key(target) && !def.outputs.contains_key(target) {
            return Err(FlowError::UnknownTarget { node: name.into(), port: port.clone(), target: target.clone() });
        }
    }
    if let Some(port) = puertos.iter().find(|p| !d.out.contains_key(*p)) {
        return Err(FlowError::MissingPort { node: name.into(), port: port.clone() });
    }
    Ok(())
}

/// Ranges `a..b` of a score or a noul: half-open, ordered, contiguous and covering `[0, top]`.
fn ranges_for(name: &str, out: &BTreeMap<String, String>, top: f64) -> Result<Vec<(f64, f64, String)>, FlowError> {
    let bad = |reason: String| FlowError::BadRanges { node: name.into(), reason };
    let mut ranges = Vec::new();
    for port in out.keys().filter(|p| *p != ERROR && *p != LOW_CONFIDENCE) {
        let parsed = port
            .split_once("..")
            .and_then(|(a, b)| Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?)));
        let Some((a, b)) = parsed else {
            return Err(FlowError::UnknownPort { node: name.into(), port: port.clone() });
        };
        if a >= b {
            return Err(bad(format!("{port}: el inicio debe ser menor que el final")));
        }
        ranges.push((a, b, port.clone()));
    }
    ranges.sort_by(|x, y| x.0.total_cmp(&y.0));
    match (ranges.first(), ranges.last()) {
        (Some(first), Some(last)) if first.0 == 0.0 && last.1 == top => {}
        _ => return Err(bad(format!("deben cubrir 0..{top}"))),
    }
    for w in ranges.windows(2) {
        if w[0].1 != w[1].0 {
            return Err(bad(format!("hueco o solape entre {} y {}", w[0].2, w[1].2)));
        }
    }
    Ok(ranges)
}

/// The `$answer/<node>` references (and `$answer/<node>/<question>` in a `batch`) must exist.
fn check_refs(output: &str, v: &serde_json::Value, nodes: &BTreeMap<String, Node>) -> Result<(), FlowError> {
    match v {
        serde_json::Value::String(s) => match s.strip_prefix("$answer/") {
            // The reference names the whole node, or one of its questions if the node is a `batch`.
            Some(rest) => {
                let (node, question) = rest.split_once('/').map_or((rest, None), |(n, q)| (n, Some(q)));
                let known = nodes.get(node).is_some_and(|n| question.is_none_or(|q| n.questions.contains_key(q)));
                match known {
                    true => Ok(()),
                    false => Err(FlowError::BadReference { node: output.into(), reference: s.clone() }),
                }
            }
            None => Ok(()),
        },
        serde_json::Value::Array(a) => a.iter().try_for_each(|x| check_refs(output, x, nodes)),
        serde_json::Value::Object(m) => m.values().try_for_each(|x| check_refs(output, x, nodes)),
        _ => Ok(()),
    }
}

/// No cycles, and every node reachable from `start`.
fn check_graph(start: &str, nodes: &BTreeMap<String, Node>) -> Result<(), FlowError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Visiting,
        Done,
    }
    fn visit<'a>(
        n: &'a str,
        nodes: &'a BTreeMap<String, Node>,
        marks: &mut BTreeMap<&'a str, Mark>,
    ) -> Result<(), FlowError> {
        match marks.get(n) {
            Some(Mark::Done) => return Ok(()),
            Some(Mark::Visiting) => return Err(FlowError::Cycle(n.to_string())),
            None => {}
        }
        let Some(node) = nodes.get(n) else { return Ok(()) };
        marks.insert(n, Mark::Visiting);
        for t in node.out.values() {
            visit(t, nodes, marks)?;
        }
        marks.insert(n, Mark::Done);
        Ok(())
    }
    let mut marks = BTreeMap::new();
    visit(start, nodes, &mut marks)?;
    match nodes.keys().find(|n| !marks.contains_key(n.as_str())) {
        Some(n) => Err(FlowError::Unreachable(n.clone())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ports the editor offers to wire are the same ones validation requires.
    #[test]
    fn answer_ports_ofrece_los_puertos_que_la_validacion_exige() {
        let def = |yaml: &str| serde_yaml::from_str::<NodeDef>(yaml).unwrap();
        // A `rule` and a `noul` without criteria: `true` and `false`.
        assert_eq!(answer_ports(&def("type: rule\nwhen: \"true\"\nout: {}")), ["true", "false"]);
        assert_eq!(answer_ports(&def("type: noul\nout: {}")), ["true", "false"]);
        // A `choice`: its options, in map order.
        assert_eq!(answer_ports(&def("type: choice\ncriteria: { b: \"B\", a: \"A\" }\nout: {}")), ["a", "b"]);
        // A `score`: one span per level minus one.
        assert_eq!(
            answer_ports(&def("type: score\ncriteria: [\"bajo\", \"medio\", \"alto\"]\nout: {}")),
            ["0..1", "1..2"]
        );
        // A `batch` wires no answer ports: its `route` rules do the routing.
        assert!(answer_ports(&def("type: batch\nquestions: { q: { type: noul } }\nout: {}")).is_empty());
    }

    const BACKENDS: [&str; 2] = ["typesafe", "ollama"];

    const BASE: &str = r#"
defaults: { backend: ollama, model: nimble }
start: destructive
nodes:
  destructive:
    type: noul
    instructions: "¿Destructivo?"
    threshold: 0.8
    out: { "true": deny, "false": kind, error: abstain }
  kind:
    type: choice
    backend: typesafe
    model: jev-latest
    instructions: "¿Tipo?"
    criteria: { code-edit: "Editar", shell: "Comandos", research: ~ }
    out: { code-edit: allow, shell: risk, research: allow, low-confidence: ask }
  risk:
    type: score
    instructions: "¿Riesgo?"
    criteria: ["ninguno", "bajo", "medio", "alto"]
    out: { "0..1.5": allow, "1.5..3": ask }
outputs:
  allow: { outcome: allow }
  deny: { outcome: deny, data: { reason: destructivo } }
  ask: { outcome: ask-user }
  abstain: { outcome: abstain }
"#;

    fn ok(yaml: &str) -> Flow {
        parse("agent-guard", yaml, &BACKENDS, "ollama").unwrap_or_else(|e| panic!("{e}"))
    }

    fn err(yaml: &str) -> FlowError {
        parse("agent-guard", yaml, &BACKENDS, "ollama").expect_err("debía ser inválido")
    }

    fn with(from: &str, to: &str) -> String {
        assert!(BASE.contains(from), "el texto base no contiene {from:?}");
        BASE.replace(from, to)
    }

    #[test]
    fn base_flow_is_valid_and_applies_defaults() {
        let f = ok(BASE);
        assert_eq!(f.name, "agent-guard");
        assert_eq!(f.start, "destructive");
        let d = &f.nodes["destructive"];
        assert_eq!((d.backend.as_str(), d.model.as_str(), d.threshold), ("ollama", "nimble", 0.8));
        assert_eq!(d.min_confidence, DEFAULT_MIN_CONFIDENCE);
        let k = &f.nodes["kind"];
        assert_eq!((k.backend.as_str(), k.model.as_str()), ("typesafe", "jev-latest"));
        assert_eq!(k.threshold, DEFAULT_THRESHOLD);
        let r = &f.nodes["risk"];
        assert_eq!(r.ranges, vec![(0.0, 1.5, "0..1.5".to_string()), (1.5, 3.0, "1.5..3".to_string())]);
    }

    #[test]
    fn default_backend_applies_when_nothing_is_declared() {
        let f = ok(&with("defaults: { backend: ollama, model: nimble }", "defaults: { model: nimble }"));
        assert_eq!(f.nodes["destructive"].backend, "ollama");
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(matches!(err(&with("threshold: 0.8", "treshold: 0.8")), FlowError::Yaml(_)));
    }

    #[test]
    fn bad_flow_name() {
        assert!(matches!(parse("Agent Guard", BASE, &BACKENDS, "ollama").unwrap_err(), FlowError::BadName(_)));
    }

    #[test]
    fn passthrough_is_a_reserved_name() {
        assert!(matches!(parse("passthrough", BASE, &BACKENDS, "ollama").unwrap_err(), FlowError::BadName(_)));
    }

    #[test]
    fn choice_options_cannot_shadow_common_ports() {
        let e = err(&with("research: ~", "error: ~").replace("research: allow", "error: allow"));
        assert!(matches!(e, FlowError::BadCriteria { .. }));
    }

    #[test]
    fn unknown_start() {
        assert!(matches!(err(&with("start: destructive", "start: nope")), FlowError::UnknownStart(_)));
    }

    #[test]
    fn unknown_target() {
        assert!(matches!(err(&with("research: allow", "research: nowhere")), FlowError::UnknownTarget { .. }));
    }

    #[test]
    fn cycle() {
        assert!(matches!(err(&with(r#""1.5..3": ask"#, r#""1.5..3": destructive"#)), FlowError::Cycle(_)));
    }

    #[test]
    fn missing_answer_port() {
        let e = err(&with("research: allow, ", ""));
        assert_eq!(e, FlowError::MissingPort { node: "kind".into(), port: "research".into() });
    }

    #[test]
    fn unknown_port() {
        assert!(matches!(err(&with("error: abstain", "maybe: abstain")), FlowError::UnknownPort { .. }));
    }

    #[test]
    fn score_ranges_with_a_gap() {
        assert!(matches!(err(&with(r#""1.5..3": ask"#, r#""2..3": ask"#)), FlowError::BadRanges { .. }));
    }

    #[test]
    fn score_ranges_not_reaching_the_top() {
        assert!(matches!(err(&with(r#""1.5..3": ask"#, r#""1.5..2": ask"#)), FlowError::BadRanges { .. }));
    }

    #[test]
    fn choice_with_one_option() {
        let e = err(&with(
            r#"criteria: { code-edit: "Editar", shell: "Comandos", research: ~ }"#,
            r#"criteria: { code-edit: "Editar" }"#,
        ));
        assert!(matches!(e, FlowError::BadCriteria { .. }));
    }

    #[test]
    fn score_with_one_level() {
        assert!(matches!(
            err(&with(r#"criteria: ["ninguno", "bajo", "medio", "alto"]"#, r#"criteria: ["ninguno"]"#)),
            FlowError::BadCriteria { .. }
        ));
    }

    #[test]
    fn threshold_out_of_range() {
        assert!(matches!(err(&with("threshold: 0.8", "threshold: 1.8")), FlowError::OutOfRange { .. }));
    }

    #[test]
    fn threshold_on_a_choice_is_rejected() {
        assert!(matches!(
            err(&with("instructions: \"¿Tipo?\"", "instructions: \"¿Tipo?\"\n    threshold: 0.5")),
            FlowError::BadCriteria { .. }
        ));
    }

    #[test]
    fn unknown_backend() {
        assert!(matches!(err(&with("backend: typesafe", "backend: openai")), FlowError::UnknownBackend { .. }));
    }

    #[test]
    fn missing_model() {
        assert!(matches!(
            err(&with("defaults: { backend: ollama, model: nimble }", "defaults: { backend: ollama }")),
            FlowError::MissingModel(_)
        ));
    }

    #[test]
    fn unreachable_node() {
        let orphan =
            "  orphan:\n    type: noul\n    instructions: x\n    out: { \"true\": allow, \"false\": allow }\noutputs:";
        assert_eq!(err(&with("outputs:", orphan)), FlowError::Unreachable("orphan".into()));
    }

    #[test]
    fn node_and_output_names_clash() {
        let clash = "outputs:\n  risk: { outcome: x }";
        assert!(matches!(err(&with("outputs:", clash)), FlowError::NameClash(_)));
    }

    #[test]
    fn flow_round_trips_through_json() {
        let f = ok(BASE);
        let back: Flow = serde_json::from_value(serde_json::to_value(&f).unwrap()).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn definition_without_instructions_deserializes() {
        let f = ok(BASE);
        let mut json = serde_json::to_value(&f).unwrap();
        for node in json["nodes"].as_object_mut().unwrap().values_mut() {
            node.as_object_mut().unwrap().remove("instructions");
        }
        let back: Flow = serde_json::from_value(json).unwrap();
        assert!(back.nodes.values().all(|n| n.instructions.is_empty()));
        assert_eq!(back.start, f.start);
    }

    const RULES: &str = r#"
defaults: { backend: ollama, model: nimble }
start: user
nodes:
  user: { type: rule, when: "has(input.from_user) && input.from_user", out: { "true": yes, "false": ask } }
  ask:
    type: noul
    instructions: "¿Sí?"
    out: { "true": yes, "false": no }
outputs:
  yes: { outcome: ok }
  no: { outcome: ok }
"#;

    #[test]
    fn rule_node_is_valid_and_needs_no_backend() {
        let f = ok(RULES);
        let r = &f.nodes["user"];
        assert_eq!(r.ty, NodeType::Rule);
        assert_eq!(r.when.as_deref(), Some("has(input.from_user) && input.from_user"));
        assert_eq!((r.backend.as_str(), r.model.as_str()), (crate::RULE_BACKEND, crate::RULE_BACKEND));
    }

    #[test]
    fn rule_without_when_is_invalid() {
        let y = RULES.replace(r#"when: "has(input.from_user) && input.from_user", "#, "");
        assert!(matches!(err(&y), FlowError::BadCriteria { .. }));
    }

    #[test]
    fn broken_cel_is_invalid_not_a_panic() {
        let y = RULES.replace("has(input.from_user) && input.from_user", "input. &&");
        assert!(matches!(err(&y), FlowError::BadExpression { .. }));
    }

    #[test]
    fn when_on_a_noul_is_invalid() {
        let y = RULES.replace(r#"instructions: "¿Sí?""#, "instructions: \"¿Sí?\"\n    when: \"true\"");
        assert!(matches!(err(&y), FlowError::BadCriteria { .. }));
    }

    #[test]
    fn rule_must_wire_true_and_false() {
        let y = RULES.replace(r#", "false": ask"#, "");
        assert!(matches!(err(&y), FlowError::MissingPort { .. }));
    }

    const RANGES: &str = r#"
defaults: { backend: ollama, model: nimble }
start: worth
nodes:
  worth:
    type: noul
    instructions: "¿Vale la pena?"
    out: { "0..0.2": sleep, "0.2..0.5": unsure, "0.5..1": wake }
outputs:
  sleep: { outcome: ok, data: { wake: false, probability: "$answer/worth" } }
  unsure: { outcome: ok, data: { wake: true, why: "$input/reason" } }
  wake: { outcome: ok }
"#;

    #[test]
    fn noul_accepts_ranges_over_zero_one() {
        let f = ok(RANGES);
        assert_eq!(f.nodes["worth"].ranges.len(), 3);
    }

    #[test]
    fn noul_ranges_accept_true_false_criteria_but_nothing_else() {
        let with = |c: &str| {
            RANGES.replace(r#"instructions: "¿Vale la pena?""#, &format!("instructions: \"x\"\n    criteria: {c}"))
        };
        assert_eq!(ok(&with(r#"{ "true": "Sí", "false": "No" }"#)).nodes["worth"].ranges.len(), 3);
        assert!(matches!(err(&with(r#"{ quizá: "Tal vez" }"#)), FlowError::BadCriteria { .. }));
    }

    #[test]
    fn noul_ranges_must_cover_zero_one() {
        let y = RANGES.replace(r#""0.5..1": wake"#, r#""0.5..0.9": wake"#);
        assert!(matches!(err(&y), FlowError::BadRanges { .. }));
    }

    #[test]
    fn noul_ranges_and_threshold_do_not_mix() {
        let y = RANGES.replace(r#"instructions: "¿Vale la pena?""#, "instructions: \"x\"\n    threshold: 0.3");
        assert!(matches!(err(&y), FlowError::BadCriteria { .. }));
    }

    #[test]
    fn answer_reference_must_name_a_node() {
        let y = RANGES.replace("$answer/worth", "$answer/nobody");
        assert!(matches!(err(&y), FlowError::BadReference { .. }));
    }

    /// Flow with a `batch` node of three questions, routing rules and `else`.
    const BATCH: &str = r#"
defaults: { backend: ollama, model: nimble }
start: risks
nodes:
  risks:
    type: batch
    questions:
      destructive: { type: noul, instructions: "¿Rompe algo?" }
      privilege: { type: noul, instructions: "¿Eleva privilegios?" }
      risk: { type: score, instructions: "¿Cuánto riesgo?", criteria: ["ninguno", "bajo", "alto"] }
    route:
      - { when: "answers.risks.destructive >= 0.85", to: deny }
      - { when: "answers.risks.privilege >= 0.55 || answers.risks.risk >= 2", to: ask }
    else: allow
    out: { error: abstain }
outputs:
  allow: { outcome: allow }
  deny: { outcome: deny }
  ask: { outcome: ask-user }
  abstain: { outcome: abstain }
"#;

    fn batch_ok() -> Flow {
        ok(BATCH)
    }

    fn batch_ok_with(yaml: &str) -> Flow {
        parse("agent-guard", yaml, &BACKENDS, "ollama").unwrap_or_else(|e| panic!("{e}"))
    }

    /// The error reason, to check that the message points at the culprit question.
    fn reason(e: &FlowError) -> String {
        match e {
            FlowError::BadCriteria { node, reason } => format!("{node}: {reason}"),
            other => panic!("se esperaba un BadCriteria, llegó {other:?}"),
        }
    }

    #[test]
    fn batch_node_parses() {
        let f = batch_ok();
        let risks = &f.nodes["risks"];
        assert_eq!(risks.ty, NodeType::Batch);
        assert_eq!(risks.ty.as_str(), "batch");
        assert_eq!(risks.questions.keys().collect::<Vec<_>>(), ["destructive", "privilege", "risk"]);
        assert_eq!(risks.questions["destructive"].ty, NodeType::Noul);
        assert_eq!(risks.questions["risk"].ty, NodeType::Score);
        assert_eq!(risks.questions["risk"].ranges.len(), 2);
        assert_eq!(risks.route.len(), 2);
        assert_eq!(risks.else_branch.as_deref(), Some("allow"));
        assert_eq!(risks.out["error"], "abstain");
        // No thresholds of its own: the flow's, and not one more (D4).
        assert_eq!((risks.backend.as_str(), risks.model.as_str(), risks.threshold), ("ollama", "nimble", 0.5));
        assert_eq!(f.nodes["risks"].min_confidence, DEFAULT_MIN_CONFIDENCE);
    }

    #[test]
    fn batch_rejects_unknown_fields() {
        let y = BATCH.replace("else: allow", "else: allow\n    routes: []");
        assert!(matches!(err(&y), FlowError::Yaml(_)));
        let y = BATCH.replace("- { when:", "- { whence:");
        assert!(matches!(err(&y), FlowError::Yaml(_)));
        let y = BATCH.replace("destructive: { type: noul", "destructive: { type: noul, treshold: 0.5");
        assert!(matches!(err(&y), FlowError::Yaml(_)), "un campo de más en una pregunta es YAML inválido");
    }

    #[test]
    fn batch_needs_a_question() {
        let y = r#"
defaults: { backend: ollama, model: nimble }
start: risks
nodes:
  risks:
    type: batch
    route:
      - { when: "answers.risks.x >= 0.5", to: allow }
    else: allow
    out: { error: abstain }
outputs:
  allow: { outcome: allow }
  abstain: { outcome: abstain }
"#;
        assert!(reason(&err(y)).contains("al menos una pregunta"), "{}", reason(&err(y)));
    }

    #[test]
    fn batch_question_name_must_be_a_cel_identifier() {
        for bad in ["destructive-risk", "destructive.dot", "Destructive"] {
            let y = BATCH.replace("      destructive:", &format!("      {bad}:"));
            assert!(reason(&err(&y)).contains("identificador CEL"), "{bad} debía rechazarse");
        }
    }

    #[test]
    fn batch_route_to_unknown_target() {
        let y = BATCH.replace("to: deny", "to: nowhere");
        assert!(
            matches!(err(&y), FlowError::UnknownTarget { port, target, .. } if port == "route" && target == "nowhere")
        );
    }

    #[test]
    fn batch_only_wires_the_error_port() {
        // D4: `low-confidence` is half a threshold, and in a `batch` it plays no part.
        let y = BATCH.replace("out: { error: abstain }", "out: { error: abstain, low-confidence: ask }");
        let e = err(&y);
        assert!(reason(&e).contains("low-confidence"), "{e:?}");

        let y = BATCH.replace("    out: { error: abstain }", "    out: { whatever: abstain }");
        let e = err(&y);
        assert!(matches!(e, FlowError::UnknownPort { .. }), "{e:?}");

        // A question's `out` is allowed to be absent, but if it is there it cannot wire anything.
        let y = BATCH.replace(
            "      destructive: { type: noul, instructions: \"¿Rompe algo?\" }",
            "      destructive: { type: noul, instructions: \"¿Rompe algo?\", out: { \"true\": deny } }",
        );
        let e = err(&y);
        assert!(
            matches!(e, FlowError::UnknownPort { .. }) || reason(&e).contains("no cablea puertos"),
            "un puerto en una pregunta debía rechazarse: {e:?}"
        );
    }

    #[test]
    fn batch_cannot_contain_a_rule() {
        let y = BATCH.replace(
            "destructive: { type: noul, instructions: \"¿Rompe algo?\" }",
            "destructive: { type: rule, when: \"true\" }",
        );
        assert!(reason(&err(&y)).contains("batch ni rule"));
    }

    #[test]
    fn batch_else_must_exist() {
        let y = BATCH.replace("else: allow", "else: nowhere");
        assert!(
            matches!(err(&y), FlowError::UnknownTarget { port, target, .. } if port == "else" && target == "nowhere")
        );

        // Without `else` and with an error port, the flow is still valid: the error is an exit.
        let y = BATCH.replace("    else: allow\n", "");
        assert_eq!(batch_ok_with(&y).nodes["risks"].else_branch, None);

        // Without `else` or `out.error`, the node would not know where to go when no rule matches.
        let y = BATCH.replace("    else: allow\n", "").replace("error: abstain", "");
        assert!(matches!(err(&y), FlowError::MissingPort { port, .. } if port == "else"));
    }

    #[test]
    fn broken_cel_in_a_batch_route_is_invalid_not_a_panic() {
        let y = BATCH.replace("answers.risks.destructive >= 0.85", "answers.risks.destructive >");
        assert!(matches!(err(&y), FlowError::BadExpression { .. }));
    }

    #[test]
    fn batch_reference_may_name_a_question() {
        let y = BATCH
            .replace("deny: { outcome: deny }", "deny: { outcome: deny, data: { p: \"$answer/risks/destructive\" } }");
        assert!(batch_ok_with(&y).outputs["deny"].data.is_some());

        let y =
            BATCH.replace("deny: { outcome: deny }", "deny: { outcome: deny, data: { p: \"$answer/risks/nobody\" } }");
        assert!(matches!(err(&y), FlowError::BadReference { .. }));

        let y = BATCH.replace("deny: { outcome: deny }", "deny: { outcome: deny, data: { p: \"$answer/nobody/x\" } }");
        assert!(matches!(err(&y), FlowError::BadReference { .. }));
    }
}
