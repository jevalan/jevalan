use crate::model::{Criteria, NodeType};
use crate::validate::{Flow, Node};
use crate::{ERROR, LOW_CONFIDENCE};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Respuesta de un backend a la pregunta de un nodo.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub answer: Value,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathStep {
    pub node: String,
    pub backend: String,
    pub model: String,
    /// `null` si el backend falló.
    pub answer: Value,
    pub confidence: Option<f64>,
    pub port: String,
    pub duration_ms: u64,
    /// Heurísticas evaluadas en este paso (`signals.<nombre>` → valor); vacío si no leyó ninguna.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub signals: Map<String, Value>,
    /// Solo `batch`: índice de la regla `route` que ganó; `None` si mandó `else` o hubo error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Next<'f> {
    Ask {
        node: &'f str,
        backend: &'f str,
        model: &'f str,
        question: Value,
    },
    /// Un nodo `batch`: todas sus preguntas viajan juntas en una sola llamada.
    BatchAsk {
        node: &'f str,
        backend: &'f str,
        model: &'f str,
        questions: Value,
    },
    /// Un nodo `rule` se evalúa sin preguntar a nadie: ya queda registrado en el camino.
    Ruled(PathStep),
    Outcome {
        outcome: &'f str,
        /// Con las referencias `$input`/`$answer` ya resueltas.
        data: Option<Value>,
    },
    Failed {
        node: String,
        error: String,
        /// Falló una regla (su error puede citar valores de la entrada), no un backend.
        rule: bool,
    },
}

enum Pos {
    At(String),
    Failed(String, String, bool),
}

/// Una ejecución de un flujo. No hace I/O: quien la conduce pregunta al backend.
pub struct Run<'f> {
    flow: &'f Flow,
    input: Value,
    pos: Pos,
    path: Vec<PathStep>,
}

impl<'f> Run<'f> {
    pub fn new(flow: &'f Flow, input: Value) -> Self {
        Self { flow, input, pos: Pos::At(flow.start.clone()), path: Vec::new() }
    }

    // El nombre `next` viene fijado por la interfaz de la tarea; no es un Iterator.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Next<'f> {
        let flow = self.flow;
        if let Pos::At(name) = &self.pos
            && let Some((k, n)) = flow.nodes.get_key_value(name.as_str())
            && n.ty == NodeType::Rule
        {
            let when = n.when.as_deref().unwrap_or("false");
            let (answer, port, signals) = match eval_rule(when, flow, &self.input, &self.path) {
                Ok((b, signals)) => (Value::Bool(b), if b { "true" } else { "false" }.to_string(), signals),
                Err(_) if n.out.contains_key(ERROR) => (Value::Null, ERROR.to_string(), Map::new()),
                Err(e) => {
                    self.pos = Pos::Failed(k.clone(), e, true);
                    let step = rule_step(k, Value::Null, ERROR);
                    self.path.push(step.clone());
                    return Next::Ruled(step);
                }
            };
            self.pos = Pos::At(n.out[&port].clone());
            let step = PathStep { signals, ..rule_step(k, answer, &port) };
            self.path.push(step.clone());
            return Next::Ruled(step);
        }
        match &self.pos {
            Pos::Failed(node, error, rule) => Next::Failed { node: node.clone(), error: error.clone(), rule: *rule },
            Pos::At(name) => match flow.nodes.get_key_value(name.as_str()) {
                Some((k, n)) if n.ty == NodeType::Batch => {
                    Next::BatchAsk { node: k, backend: &n.backend, model: &n.model, questions: questions(n) }
                }
                Some((k, n)) => Next::Ask { node: k, backend: &n.backend, model: &n.model, question: question(n) },
                None => {
                    let o = &flow.outputs[name.as_str()];
                    Next::Outcome {
                        outcome: &o.outcome,
                        data: o.data.as_ref().map(|d| resolve_refs(d, &self.input, &self.path)),
                    }
                }
            },
        }
    }

    /// Registra la respuesta del nodo actual y avanza. `None` si no había pregunta pendiente.
    pub fn answer(&mut self, result: Result<Reply, String>, duration_ms: u64) -> Option<&PathStep> {
        let Pos::At(current) = &self.pos else { return None };
        let flow = self.flow;
        let (name, node) = flow.nodes.get_key_value(current.as_str())?;
        // Un `batch` no se responde con una sola respuesta: eso es `answer_batch`.
        if matches!(node.ty, NodeType::Rule | NodeType::Batch) {
            return None;
        }
        let step = |answer: Value, confidence: Option<f64>, port: &str| PathStep {
            node: name.clone(),
            backend: node.backend.clone(),
            model: node.model.clone(),
            answer,
            confidence,
            port: port.into(),
            duration_ms,
            signals: Map::new(),
            rule: None,
        };
        match result.and_then(|r| port_for(node, &r).map(|port| (r, port))) {
            Ok((r, port)) => {
                let port = if r.confidence < node.min_confidence && node.out.contains_key(LOW_CONFIDENCE) {
                    LOW_CONFIDENCE.to_string()
                } else {
                    port
                };
                self.pos = Pos::At(node.out[&port].clone());
                self.path.push(step(r.answer, Some(r.confidence), &port));
            }
            Err(e) => {
                self.pos = match node.out.get(ERROR) {
                    Some(target) => Pos::At(target.clone()),
                    None => Pos::Failed(name.clone(), e, false),
                };
                self.path.push(step(Value::Null, None, ERROR));
            }
        }
        self.path.last()
    }

    pub fn path(&self) -> &[PathStep] {
        &self.path
    }
}

/// Respuestas crudas del backend a cada pregunta del nodo `batch` —tal como vienen en `answers`—, o el
/// fallo que impidió obtenerlas.
type Answers = Result<Map<String, Value>, String>;

impl Run<'_> {
    /// Registra de una vez la respuesta de un nodo `batch` y enruta según sus reglas.
    ///
    /// Cada entrada es el cuerpo de una pregunta (`{type, <tipo>, confidence?}`); su valor se extrae y se
    /// valida con las mismas reglas que el de un nodo suelto. El nodo deja **un solo** `PathStep`: la
    /// invariante «un nodo, un paso» sostiene las referencias, los eventos y la TUI (D2).
    pub fn answer_batch(&mut self, replies: Answers, duration_ms: u64) -> Option<&PathStep> {
        let (name, node) = self.batch_node()?;
        let step = |answer: Value, confidence: Option<f64>, port: &str| PathStep {
            node: name.clone(),
            backend: node.backend.clone(),
            model: node.model.clone(),
            answer,
            confidence,
            port: port.into(),
            duration_ms,
            signals: Map::new(),
            rule: None,
        };
        // `rule`: el fallo viene de una regla o heurística, cuyo error puede citar la entrada; el daemon
        // publica entonces una frase genérica. Una respuesta que falta o no vale es fallo del backend.
        let fail = |run: &mut Self, e: String, rule: bool| {
            run.pos = match node.out.get(ERROR) {
                Some(target) => Pos::At(target.clone()),
                None => Pos::Failed(name.clone(), e, rule),
            };
        };
        // Falta una respuesta o viene inservible: jamás se decide con lo que llegue (D6).
        let given = replies.and_then(|given| {
            let mut values = Map::new();
            let mut total = 0.0;
            for (question, body) in &node.questions {
                let entry = given.get(question).ok_or_else(|| format!("falta {question}"))?;
                let Reply { answer, confidence } =
                    reply_from_response(question, body, &json!({ "answers": { question: entry } }))
                        .map_err(|e| format!("{question}: {e}"))?;
                // El puerto que saldría no interesa —aquí enrutan las reglas—, pero sí su validación:
                // un valor fuera de [0, 1], de la escala o de los criterios es un fallo, no una respuesta.
                port_for(body, &Reply { answer: answer.clone(), confidence })?;
                values.insert(question.clone(), answer);
                total += confidence;
            }
            Ok((values, total / node.questions.len() as f64))
        });
        let (values, confidence) = match given {
            Ok(given) => given,
            Err(e) => {
                fail(self, e, false);
                self.path.push(step(Value::Null, None, ERROR));
                return self.path.last();
            }
        };
        // La primera `when` verdadera gana; sin ninguna, manda `else`.
        let (target, rule, signals) = match route_for(self.flow, &node, &name, &self.input, &self.path, &values) {
            Ok((Some((i, target)), signals)) => (Some(target), Some(i), signals),
            Ok((None, signals)) => (node.else_branch.clone(), None, signals),
            Err(e) => {
                fail(self, e, true);
                (None, None, Map::new())
            }
        };
        let Some(target) = target else {
            self.path.push(step(Value::Null, None, ERROR));
            return self.path.last();
        };
        self.pos = Pos::At(target.clone());
        self.path.push(PathStep { signals, rule, ..step(Value::Object(values), Some(confidence), &target) });
        self.path.last()
    }

    /// El nodo `batch` en curso, si el camino está parado en uno: se copia para no retener el préstamo.
    fn batch_node(&self) -> Option<(String, Node)> {
        let Pos::At(current) = &self.pos else { return None };
        let (name, node) = self.flow.nodes.get_key_value(current.as_str())?;
        (node.ty == NodeType::Batch).then(|| (name.clone(), node.clone()))
    }
}

type Route = (Option<(usize, String)>, Map<String, Value>);

/// Índice y destino de la primera regla de `route` que casa (o `None` si ninguna), con las heurísticas que se
/// evaluaron por el camino. Una regla que no se llega a mirar no evalúa las suyas (D4).
fn route_for(
    flow: &Flow,
    node: &Node,
    name: &str,
    input: &Value,
    path: &[PathStep],
    values: &Map<String, Value>,
) -> Result<Route, String> {
    // `answers` acumula los pasos ya recorridos y, bajo el nombre de este nodo, las respuestas recién dadas:
    // así `answers.<nodo>.<pregunta>` ve lo que el backend acaba de contestar.
    let mut answers: Map<String, Value> = path.iter().map(|s| (s.node.clone(), s.answer.clone())).collect();
    answers.insert(name.into(), Value::Object(values.clone()));
    let mut seen = Map::new();
    for (i, (when, to)) in node.route.iter().enumerate() {
        let signals = signal_values(when, flow, input)?;
        seen.extend(signals.clone());
        if eval_when(when, input, &answers, signals)? {
            return Ok((Some((i, to.clone())), seen));
        }
    }
    Ok((None, seen))
}

/// Evalúa un `when` con `input`, las respuestas (ya obtenidas y las de este mismo paso) y sus heurísticas.
fn eval_when(
    when: &str,
    input: &Value,
    answers: &Map<String, Value>,
    signals: Map<String, Value>,
) -> Result<bool, String> {
    let program = crate::program::compiled(when)?;
    let mut ctx = cel::Context::default();
    ctx.add_variable("input", input.clone()).map_err(|e| e.to_string())?;
    ctx.add_variable("answers", Value::Object(answers.clone())).map_err(|e| e.to_string())?;
    ctx.add_variable("signals", Value::Object(signals)).map_err(|e| e.to_string())?;
    match program.execute(&ctx) {
        Ok(cel::Value::Bool(b)) => Ok(b),
        Ok(other) => Err(format!("la ruta no devuelve un booleano: {other:?}")),
        Err(e) => Err(e.to_string()),
    }
}

/// Puerto de respuesta para `r`, o un error si la respuesta no encaja con el nodo.
fn port_for(node: &Node, r: &Reply) -> Result<String, String> {
    if !(0.0..=1.0).contains(&r.confidence) {
        return Err(format!("confianza fuera de [0, 1]: {}", r.confidence));
    }
    match node.ty {
        NodeType::Noul => {
            let p = r.answer.as_f64().filter(|p| (0.0..=1.0).contains(p)).ok_or("noul debe ser un número en [0, 1]")?;
            if !node.ranges.is_empty() {
                return pick_range(&node.ranges, p).ok_or_else(|| format!("probabilidad fuera de rango: {p}"));
            }
            Ok(if p >= node.threshold { "true" } else { "false" }.into())
        }
        NodeType::Rule => Err("rule no recibe respuestas".into()),
        NodeType::Choice => {
            let c = r.answer.as_str().ok_or("choice debe ser un texto")?;
            match &node.criteria {
                Some(Criteria::Options(m)) if m.contains_key(c) => Ok(c.into()),
                _ => Err(format!("opción desconocida: {c}")),
            }
        }
        NodeType::Score => {
            let s = r.answer.as_f64().ok_or("score debe ser un número")?;
            pick_range(&node.ranges, s).ok_or_else(|| format!("score fuera de escala: {s}"))
        }
        // Un `batch` no tiene puerto propio: enrutan sus reglas `route`.
        NodeType::Batch => Err("batch no recibe una sola respuesta".into()),
    }
}

/// Puerto del rango que contiene `v`: semiabierto, salvo el último, que también cierra.
fn pick_range(ranges: &[(f64, f64, String)], v: f64) -> Option<String> {
    let last = ranges.len().saturating_sub(1);
    ranges
        .iter()
        .enumerate()
        .find(|(i, (a, b, _))| v >= *a && (v < *b || (*i == last && v <= *b)))
        .map(|(_, (_, _, p))| p.clone())
}

/// Sustituye `"$input<pointer>"` y `"$answer/<nodo>"` (o `"$answer/<nodo>/<pregunta>"`) por lo apuntado;
/// `null` si no existe.
pub fn resolve_refs(data: &Value, input: &Value, path: &[PathStep]) -> Value {
    match data {
        Value::String(s) => {
            if let Some(rest) = s.strip_prefix("$answer/") {
                let (node, question) = rest.split_once('/').map_or((rest, None), |(n, q)| (n, Some(q)));
                let step = path.iter().rev().find(|p| p.node == node);
                match (step, question) {
                    (Some(p), Some(q)) => p.answer.get(q).cloned().unwrap_or(Value::Null),
                    (Some(p), None) => p.answer.clone(),
                    _ => Value::Null,
                }
            } else if let Some(ptr) = s.strip_prefix("$input") {
                input.pointer(ptr).cloned().unwrap_or(Value::Null)
            } else {
                data.clone()
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| resolve_refs(x, input, path)).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), resolve_refs(x, input, path))).collect()),
        _ => data.clone(),
    }
}

fn rule_step(node: &str, answer: Value, port: &str) -> PathStep {
    PathStep {
        node: node.into(),
        backend: crate::RULE_BACKEND.into(),
        model: crate::RULE_BACKEND.into(),
        confidence: Some(1.0),
        answer,
        port: port.into(),
        duration_ms: 0,
        signals: Map::new(),
        rule: None,
    }
}

/// Valores de las heurísticas que `expr` nombra como `signals.<nombre>`, evaluadas sobre `input`.
/// Solo las que nombra (D4); si una falla, falla la expresión entera (D5).
fn signal_values(expr: &str, flow: &Flow, input: &Value) -> Result<Map<String, Value>, String> {
    let mut values = Map::new();
    for name in crate::signals::referenced(expr) {
        let when = flow.signals.get(&name).ok_or_else(|| format!("señal desconocida {name}"))?;
        let program = crate::program::compiled(when)?;
        let mut ctx = cel::Context::default();
        ctx.add_variable("input", input.clone()).map_err(|e| e.to_string())?;
        match program.execute(&ctx) {
            Ok(cel::Value::Bool(b)) => {
                values.insert(name, Value::Bool(b));
            }
            Ok(other) => return Err(format!("la señal {name} no devuelve un booleano: {other:?}")),
            Err(e) => return Err(format!("señal {name}: {e}")),
        }
    }
    Ok(values)
}

/// Evalúa una regla con `input` (el estado), `answers` (nodo → respuesta ya obtenida) y las heurísticas que
/// nombra; devuelve también esas heurísticas, para el camino.
fn eval_rule(when: &str, flow: &Flow, input: &Value, path: &[PathStep]) -> Result<(bool, Map<String, Value>), String> {
    let program = crate::program::compiled(when)?;
    let answers: Map<String, Value> = path.iter().map(|s| (s.node.clone(), s.answer.clone())).collect();
    let signals = signal_values(when, flow, input)?;
    let mut ctx = cel::Context::default();
    ctx.add_variable("input", input.clone()).map_err(|e| e.to_string())?;
    ctx.add_variable("answers", Value::Object(answers)).map_err(|e| e.to_string())?;
    ctx.add_variable("signals", Value::Object(signals.clone())).map_err(|e| e.to_string())?;
    match program.execute(&ctx) {
        Ok(cel::Value::Bool(b)) => Ok((b, signals)),
        Ok(other) => Err(format!("la regla no devuelve un booleano: {other:?}")),
        Err(e) => Err(e.to_string()),
    }
}

/// Pregunta System One de un nodo: `{type, instructions, criteria?}`.
pub fn question(node: &Node) -> Value {
    let mut q = json!({ "type": node.ty.as_str(), "instructions": node.instructions });
    if let Some(c) = &node.criteria {
        q["criteria"] = serde_json::to_value(c).expect("criteria siempre serializa");
    }
    q
}

/// Preguntas de un nodo `batch`, con la forma `{ "<pregunta>": {type, instructions, criteria?} }`.
pub fn questions(node: &Node) -> Value {
    Value::Object(node.questions.iter().map(|(q, n)| (q.clone(), question(n))).collect())
}

/// Cuerpo de `POST /v1/systemone` con una sola pregunta, llamada como el nodo.
pub fn request_body(model: &str, state: &Value, node: &str, question: &Value) -> Value {
    let mut questions = Map::new();
    questions.insert(node.to_string(), question.clone());
    json!({ "model": model, "state": state, "questions": questions })
}

/// Cuerpo de `POST /v1/systemone` con varias preguntas ya nombradas; `request_body` no se toca.
pub fn request_body_many(model: &str, state: &Value, questions: Map<String, Value>) -> Value {
    json!({ "model": model, "state": state, "questions": questions })
}

/// Extrae la respuesta de `name` de una respuesta System One completa. En un `batch`, `name` es
/// `"<nodo>.<pregunta>"` y `node` el cuerpo de esa pregunta.
pub fn reply_from_response(name: &str, node: &Node, response: &Value) -> Result<Reply, String> {
    let ty = node.ty.as_str();
    let a = response
        .get("answers")
        .and_then(|a| a.get(name))
        .ok_or_else(|| format!("la respuesta no trae la pregunta {name}"))?;
    if let Some(t) = a.get("type").and_then(Value::as_str)
        && t != ty
    {
        return Err(format!("respuesta de tipo {t}, se esperaba {ty}"));
    }
    let answer = a.get(ty).cloned().ok_or_else(|| format!("falta el campo {ty}"))?;
    let confidence = match a.get("confidence").and_then(Value::as_f64) {
        Some(c) => c,
        None if node.ty == NodeType::Noul => answer.as_f64().map(binary_confidence).ok_or("noul debe ser un número")?,
        None => return Err("falta confidence".into()),
    };
    Ok(Reply { answer, confidence })
}

/// `1 - H(p) / ln 2`: la misma concentración que System One da en choice y score.
pub fn binary_confidence(p: f64) -> f64 {
    let h = |x: f64| if x <= 0.0 { 0.0 } else { -x * x.ln() };
    1.0 - (h(p) + h(1.0 - p)) / std::f64::consts::LN_2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use serde_json::json;

    const BACKENDS: [&str; 3] = ["typesafe", "ollama", "openrouter"];

    fn guard() -> Flow {
        parse("agent-guard", include_str!("../../../../config/flows/agent-guard.yaml"), &BACKENDS, "ollama").unwrap()
    }

    fn ok(answer: Value, confidence: f64) -> Result<Reply, String> {
        Ok(Reply { answer, confidence })
    }

    /// Alimenta el flujo con `replies` en orden y devuelve el final y los puertos recorridos.
    fn drive(flow: &Flow, replies: Vec<Result<Reply, String>>) -> (String, Vec<String>) {
        let mut run = Run::new(flow, json!({}));
        for r in replies {
            assert!(matches!(run.next(), Next::Ask { .. }), "sobran respuestas");
            run.answer(r, 1);
        }
        let end = match run.next() {
            Next::Outcome { outcome, .. } => outcome.to_string(),
            Next::Failed { node, .. } => format!("failed:{node}"),
            Next::Ask { node, .. } => format!("ask:{node}"),
            Next::BatchAsk { node, .. } => format!("batch:{node}"),
            Next::Ruled(step) => format!("ruled:{}", step.node),
        };
        (end, run.path().iter().map(|s| s.port.clone()).collect())
    }

    #[test]
    fn happy_path_follows_the_answers() {
        let (end, ports) = drive(&guard(), vec![ok(json!(0.1), 0.9), ok(json!("code-edit"), 0.9)]);
        assert_eq!((end.as_str(), ports), ("allow", vec!["false".to_string(), "code-edit".to_string()]));
    }

    #[test]
    fn noul_at_the_threshold_is_true() {
        let (end, _) = drive(&guard(), vec![ok(json!(0.8), 0.9)]);
        assert_eq!(end, "deny");
    }

    #[test]
    fn low_confidence_takes_its_port_when_wired() {
        let (end, ports) = drive(&guard(), vec![ok(json!(0.1), 0.9), ok(json!("shell"), 0.3)]);
        assert_eq!((end.as_str(), ports[1].as_str()), ("ask-user", "low-confidence"));
    }

    #[test]
    fn low_confidence_is_ignored_when_not_wired() {
        let (end, ports) = drive(&guard(), vec![ok(json!(0.1), 0.1), ok(json!("research"), 0.9)]);
        assert_eq!((end.as_str(), ports[0].as_str()), ("allow", "false"));
    }

    #[test]
    fn backend_error_takes_the_error_port_when_wired() {
        let (end, ports) = drive(&guard(), vec![Err("timeout".into())]);
        assert_eq!((end.as_str(), ports), ("abstain", vec!["error".to_string()]));
    }

    #[test]
    fn backend_error_without_error_port_fails() {
        let (end, ports) = drive(&guard(), vec![ok(json!(0.1), 0.9), Err("500".into())]);
        assert_eq!(end, "failed:kind");
        assert_eq!(ports.last().map(String::as_str), Some("error"));
    }

    #[test]
    fn score_ranges_are_half_open() {
        let path = |s: f64| drive(&guard(), vec![ok(json!(0.1), 0.9), ok(json!("shell"), 0.9), ok(json!(s), 0.9)]).0;
        assert_eq!(path(1.49), "allow");
        assert_eq!(path(1.5), "ask-user");
    }

    #[test]
    fn score_on_the_top_edge_takes_the_last_range() {
        let (end, ports) = drive(&guard(), vec![ok(json!(0.1), 0.9), ok(json!("shell"), 0.9), ok(json!(3.0), 0.9)]);
        assert_eq!((end.as_str(), ports[2].as_str()), ("ask-user", "1.5..3"));
    }

    #[test]
    fn choice_outside_the_criteria_is_an_error() {
        let (end, _) = drive(&guard(), vec![ok(json!(0.1), 0.9), ok(json!("nope"), 0.9)]);
        assert_eq!(end, "failed:kind");
    }

    #[test]
    fn wrong_answer_type_is_an_error() {
        let (end, ports) = drive(&guard(), vec![ok(json!("yes"), 0.9)]);
        assert_eq!((end.as_str(), ports[0].as_str()), ("abstain", "error"));
    }

    #[test]
    fn answer_after_the_end_is_ignored() {
        let flow = guard();
        let mut run = Run::new(&flow, json!({}));
        run.answer(ok(json!(0.9), 0.9), 1);
        assert!(run.answer(ok(json!(0.9), 0.9), 1).is_none());
        assert_eq!(run.path().len(), 1);
    }

    #[test]
    fn path_records_backend_model_and_duration() {
        let flow = guard();
        let mut run = Run::new(&flow, json!({}));
        let step = run.answer(ok(json!(0.1), 0.7), 42).unwrap().clone();
        assert_eq!(
            step,
            PathStep {
                node: "destructive".into(),
                backend: "ollama".into(),
                model: "nimble".into(),
                answer: json!(0.1),
                confidence: Some(0.7),
                port: "false".into(),
                duration_ms: 42,
                signals: Map::new(),
                rule: None,
            }
        );
    }

    #[test]
    fn question_carries_type_instructions_and_criteria() {
        let flow = guard();
        let q = question(&flow.nodes["risk"]);
        assert_eq!(q["type"], "score");
        assert_eq!(q["criteria"], json!(["ninguno", "bajo", "medio", "alto"]));
        assert!(question(&flow.nodes["destructive"]).get("criteria").is_none());
    }

    #[test]
    fn request_body_has_one_named_question() {
        let body = request_body("nimble", &json!("rm -rf /"), "destructive", &json!({"type": "noul"}));
        assert_eq!(
            body,
            json!({"model": "nimble", "state": "rm -rf /", "questions": {"destructive": {"type": "noul"}}})
        );
    }

    #[test]
    fn reply_reads_choice_and_confidence() {
        let flow = guard();
        let resp = json!({"answers": {"kind": {"type": "choice", "choice": "shell", "confidence": 0.9}}});
        assert_eq!(
            reply_from_response("kind", &flow.nodes["kind"], &resp),
            Ok(Reply { answer: json!("shell"), confidence: 0.9 })
        );
    }

    #[test]
    fn reply_without_the_question_is_an_error() {
        let flow = guard();
        assert!(reply_from_response("kind", &flow.nodes["kind"], &json!({"answers": {}})).is_err());
    }

    #[test]
    fn reply_with_another_type_is_an_error() {
        let flow = guard();
        let resp = json!({"answers": {"kind": {"type": "score", "score": 1.0, "confidence": 0.9}}});
        assert!(reply_from_response("kind", &flow.nodes["kind"], &resp).is_err());
    }

    #[test]
    fn noul_without_confidence_gets_it_computed() {
        let flow = guard();
        let resp = json!({"answers": {"destructive": {"type": "noul", "noul": 1.0}}});
        let r = reply_from_response("destructive", &flow.nodes["destructive"], &resp).unwrap();
        assert!((r.confidence - 1.0).abs() < 1e-9);
        assert!(binary_confidence(0.5).abs() < 1e-9);
    }

    #[test]
    fn example_flows_are_valid() {
        guard();
        parse("model-route", include_str!("../../../../config/flows/model-route.yaml"), &BACKENDS, "ollama").unwrap();
    }

    const RULES: &str = r#"
defaults: { backend: ollama, model: nimble }
start: user
nodes:
  user: { type: rule, when: "has(input.from_user) && input.from_user", out: { "true": yes, "false": ask } }
  ask:
    type: noul
    instructions: "¿Sí?"
    out: { "true": yes, "false": check }
  check: { type: rule, when: "answers.ask < 0.1", out: { "true": no, "false": yes, error: no } }
outputs:
  yes: { outcome: yes }
  no: { outcome: no }
"#;

    fn rules() -> Flow {
        parse("rules", RULES, &BACKENDS, "ollama").unwrap()
    }

    #[test]
    fn rule_true_skips_the_backend() {
        let flow = rules();
        let mut run = Run::new(&flow, json!({"from_user": true}));
        let Next::Ruled(step) = run.next() else { panic!("se esperaba una regla") };
        assert_eq!((step.node.as_str(), step.port.as_str(), step.backend.as_str()), ("user", "true", "-"));
        assert_eq!(step.answer, json!(true));
        assert!(matches!(run.next(), Next::Outcome { outcome: "yes", .. }));
    }

    #[test]
    fn rule_false_then_ask_then_rule_on_answers() {
        let flow = rules();
        let mut run = Run::new(&flow, json!({}));
        assert!(matches!(run.next(), Next::Ruled(_)));
        assert!(matches!(run.next(), Next::Ask { node: "ask", .. }));
        run.answer(ok(json!(0.05), 0.9), 1);
        let Next::Ruled(step) = run.next() else { panic!("se esperaba la segunda regla") };
        assert_eq!((step.node.as_str(), step.port.as_str()), ("check", "true"));
        assert!(matches!(run.next(), Next::Outcome { outcome: "no", .. }));
    }

    #[test]
    fn rule_error_takes_its_error_port() {
        let flow = parse("rules", &RULES.replace("answers.ask < 0.1", "answers.ask"), &BACKENDS, "ollama").unwrap();
        let mut run = Run::new(&flow, json!({}));
        run.next();
        run.next();
        run.answer(ok(json!(0.05), 0.9), 1);
        let Next::Ruled(step) = run.next() else { panic!() };
        assert_eq!(step.port, "error");
    }

    #[test]
    fn rule_error_without_error_port_fails() {
        let flow = rules();
        let mut run = Run::new(&flow, json!({"from_user": "sí"}));
        let Next::Ruled(step) = run.next() else { panic!() };
        assert_eq!(step.port, "error");
        assert!(matches!(run.next(), Next::Failed { .. }));
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
  unsure: { outcome: ok, data: { wake: true, why: "$input/reason", missing: "$input/nope" } }
  wake: { outcome: ok }
"#;

    fn outcome_for(p: f64, input: Value) -> (String, Option<Value>) {
        let flow = parse("ranges", RANGES, &BACKENDS, "ollama").unwrap();
        let mut run = Run::new(&flow, input);
        run.next();
        run.answer(ok(json!(p), 0.9), 1);
        match run.next() {
            Next::Outcome { outcome, data } => (outcome.to_string(), data),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn noul_ranges_route_by_probability() {
        assert_eq!(outcome_for(0.1, json!({})).1, Some(json!({"wake": false, "probability": 0.1})));
        assert_eq!(
            outcome_for(0.2, json!({"reason": "r"})).1,
            Some(json!({"wake": true, "why": "r", "missing": null}))
        );
        assert_eq!(outcome_for(1.0, json!({})).1, None);
    }

    #[test]
    fn references_resolve_at_any_depth() {
        let path = vec![PathStep {
            node: "n".into(),
            backend: "b".into(),
            model: "m".into(),
            answer: json!("x"),
            confidence: Some(1.0),
            port: "p".into(),
            duration_ms: 0,
            signals: Map::new(),
            rule: None,
        }];
        let data = json!({"a": ["$answer/n", {"b": "$input/k/0"}], "c": "literal"});
        assert_eq!(resolve_refs(&data, &json!({"k": [7]}), &path), json!({"a": ["x", {"b": 7}], "c": "literal"}));
    }

    #[test]
    fn agent_wakeup_flow_is_valid_and_rules_short_circuit() {
        let flow =
            parse("agent-wakeup", include_str!("../../../../config/flows/agent-wakeup.yaml"), &BACKENDS, "ollama")
                .unwrap();
        let mut run = Run::new(&flow, json!({"waiting_for": "x", "from_user": true, "skipped": 0}));
        assert!(matches!(run.next(), Next::Ruled(_)));
        match run.next() {
            Next::Outcome { data, .. } => assert_eq!(data, Some(json!({"wake": true, "reason": "user-message"}))),
            other => panic!("{other:?}"),
        }
    }

    /// Flujo de un solo nodo `batch` con cuatro preguntas, dos reglas y `else`: el caso real de `tool.risk`.
    const BATCH_RUN: &str = r#"
defaults: { backend: ollama, model: nimble }
start: risks
nodes:
  risks:
    type: batch
    questions:
      destructive: { type: noul, instructions: "¿Rompe algo?" }
      exfiltration: { type: noul, instructions: "¿Sale de la máquina?" }
      privilege: { type: noul, instructions: "¿Eleva privilegios?" }
      secret_exposure: { type: noul, instructions: "¿Toca secretos?" }
    route:
      - { when: "answers.risks.destructive >= 0.85 || answers.risks.exfiltration >= 0.85 || answers.risks.privilege >= 0.85 || answers.risks.secret_exposure >= 0.85", to: deny }
      - { when: "answers.risks.destructive >= 0.55 || answers.risks.exfiltration >= 0.55 || answers.risks.privilege >= 0.55 || answers.risks.secret_exposure >= 0.55", to: ask }
    else: allow
    out: { error: abstain }
outputs:
  allow: { outcome: allow, data: { probabilities: "$answer/risks" } }
  deny: { outcome: deny, data: { destructive: "$answer/risks/destructive" } }
  ask: { outcome: ask-user, data: { secret: "$answer/risks/secret_exposure" } }
  abstain: { outcome: abstain }
"#;

    fn batch_flow() -> Flow {
        parse("tool-risk", BATCH_RUN, &BACKENDS, "ollama").unwrap()
    }

    /// Respuestas del nodo `batch`: una probabilidad por pregunta, con la confianza dada, tal como las
    /// devuelve System One.
    fn replies(p: [f64; 4], confidence: f64) -> Answers {
        let keys = ["destructive", "exfiltration", "privilege", "secret_exposure"];
        Ok(keys
            .into_iter()
            .zip(p)
            .map(|(k, p)| (k.to_string(), json!({ "type": "noul", "noul": p, "confidence": confidence })))
            .collect())
    }

    #[test]
    fn batch_ask_carries_every_question() {
        let flow = batch_flow();
        let mut run = Run::new(&flow, json!({}));
        let Next::BatchAsk { node, backend, model, questions } = run.next() else { panic!("se esperaba un batch") };
        assert_eq!((node, backend, model), ("risks", "ollama", "nimble"));
        assert_eq!(questions.as_object().unwrap().len(), 4);
        assert_eq!(questions["secret_exposure"]["type"], "noul");
        assert!(questions["secret_exposure"]["instructions"].is_string());
        // Un `batch` no se responde con una respuesta suelta.
        assert!(run.answer(ok(json!(0.1), 0.9), 1).is_none());
        assert_eq!(run.path().len(), 0);
    }

    #[test]
    fn batch_routes_on_the_first_matching_when() {
        let flow = batch_flow();
        let mut run = Run::new(&flow, json!({}));
        assert!(matches!(run.next(), Next::BatchAsk { .. }));
        let step = run.answer_batch(replies([0.9, 0.1, 0.1, 0.1], 0.9), 7).unwrap().clone();
        assert_eq!(step.port, "deny");
        assert_eq!(
            step.answer,
            json!({"destructive": 0.9, "exfiltration": 0.1, "privilege": 0.1, "secret_exposure": 0.1})
        );
        assert_eq!(step.confidence, Some(0.9));
        assert_eq!(step.duration_ms, 7);
        // Un solo paso para todo el nodo (D2).
        assert_eq!(run.path().len(), 1);
        match run.next() {
            Next::Outcome { outcome, data } => {
                assert_eq!((outcome, data), ("deny", Some(json!({"destructive": 0.9}))))
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn batch_else_when_no_route_matches() {
        let flow = batch_flow();
        let mut run = Run::new(&flow, json!({}));
        run.next();
        let step = run.answer_batch(replies([0.1, 0.2, 0.3, 0.4], 0.8), 1).unwrap().clone();
        assert_eq!(step.port, "allow");
        match run.next() {
            Next::Outcome { outcome, data } => assert_eq!(
                (outcome, data),
                (
                    "allow",
                    Some(
                        json!({"probabilities": {"destructive": 0.1, "exfiltration": 0.2, "privilege": 0.3, "secret_exposure": 0.4}})
                    )
                )
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn batch_on_the_threshold_is_true() {
        // La frontera del `noul` que el flujo reemplaza: 0.55 ya pide permiso, 0.85 ya niega.
        let route = |p: f64| {
            let flow = batch_flow();
            let mut run = Run::new(&flow, json!({}));
            run.next();
            run.answer_batch(replies([p, 0.0, 0.0, 0.0], 0.9), 1).unwrap().port.clone()
        };
        assert_eq!(route(0.549), "allow");
        assert_eq!(route(0.55), "ask");
        assert_eq!(route(0.849), "ask");
        assert_eq!(route(0.85), "deny");
    }

    #[test]
    fn batch_backend_error_takes_the_error_port() {
        let flow = batch_flow();
        let mut run = Run::new(&flow, json!({}));
        run.next();
        let step = run.answer_batch(Err("timeout".into()), 3).unwrap().clone();
        assert_eq!((step.port.as_str(), step.answer.clone(), step.confidence), ("error", json!(null), None));
        match run.next() {
            Next::Outcome { outcome, .. } => assert_eq!(outcome, "abstain"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn batch_missing_an_answer_is_an_error() {
        let flow = batch_flow();
        let mut run = Run::new(&flow, json!({}));
        run.next();
        // Al backend le falta una de las cuatro: no se decide con lo que llegue.
        let partial: Answers = Ok([("destructive", json!({ "type": "noul", "noul": 0.1, "confidence": 0.9 }))]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect());
        assert_eq!(run.answer_batch(partial, 1).unwrap().port, "error");

        // Una probabilidad que no está en [0, 1] tampoco vale.
        let flow = batch_flow();
        let mut run = Run::new(&flow, json!({}));
        run.next();
        assert_eq!(run.answer_batch(replies([1.5, 0.0, 0.0, 0.0], 0.9), 1).unwrap().port, "error");
    }

    #[test]
    fn batch_refs_resolve_the_object_and_a_single_question() {
        let flow = batch_flow();
        let mut run = Run::new(&flow, json!({}));
        run.next();
        run.answer_batch(replies([0.9, 0.1, 0.1, 0.1], 0.9), 1);
        match run.next() {
            Next::Outcome { data, .. } => assert_eq!(data, Some(json!({"destructive": 0.9}))),
            other => panic!("{other:?}"),
        }

        // Y el objeto entero, como lo pide la salida de `tool.risk`.
        let flow = batch_flow();
        let mut run = Run::new(&flow, json!({}));
        run.next();
        run.answer_batch(replies([0.1, 0.2, 0.3, 0.4], 0.8), 1);
        match run.next() {
            Next::Outcome { data, .. } => assert_eq!(
                data,
                Some(
                    json!({"probabilities": {"destructive": 0.1, "exfiltration": 0.2, "privilege": 0.3, "secret_exposure": 0.4}})
                )
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn batch_confidence_is_the_mean_of_the_answers() {
        let flow = batch_flow();
        let mut run = Run::new(&flow, json!({}));
        run.next();
        let entry = |c: f64| json!({ "type": "noul", "noul": 0.1, "confidence": c });
        let mixed: Answers = Ok([
            ("destructive", entry(0.4)),
            ("exfiltration", entry(0.8)),
            ("privilege", entry(0.6)),
            ("secret_exposure", entry(0.6)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect());
        let confidence = run.answer_batch(mixed, 1).unwrap().confidence.unwrap_or_default();
        assert!((confidence - 0.6).abs() < 1e-9, "la confianza anotada es la media: {confidence}");
    }
}
