//! Bucle de ejecución de un flujo, compartido por `/v1/decide` y por el proveedor `flow` del hub.

use crate::AppState;
use crate::bus::Tracer;
use crate::hub::transport::Io;
use axum::body::Bytes;
use jevalan_events::v2::EventKind;
use jevalan_flow::{Flow, Next, PathStep, Reply, Run, reply_from_response, request_body, request_body_many};
use serde_json::{Map, Value, json};
use std::time::Instant;

pub enum Finish {
    Done { outcome: String, data: Option<Value> },
    Failed { message: String },
}

pub struct Ran {
    pub finish: Finish,
    pub path: Vec<PathStep>,
}

/// Conduce `flow` nodo a nodo contra sus backends, emitiendo los eventos de la traza.
pub async fn run_flow(state: &AppState, name: &str, flow: &Flow, input: &Value, tracer: &Tracer, io: &Io<'_>) -> Ran {
    let started = Instant::now();
    let mut run = Run::new(flow, input.clone());
    loop {
        match run.next() {
            Next::Ruled(step) => {
                let audited = audit(name, &step);
                tracer.emit(EventKind::NodeStart {
                    flow: name.into(),
                    node: step.node.clone(),
                    node_type: "rule".into(),
                    backend: step.backend.clone(),
                    model: step.model.clone(),
                });
                tracer.emit(EventKind::NodeEnd {
                    flow: name.into(),
                    node: step.node,
                    answer: step.answer,
                    confidence: step.confidence.unwrap_or(1.0),
                    port: step.port,
                });
                if let Some(event) = audited {
                    tracer.emit(event);
                }
            }
            Next::Ask { node, backend, model, question } => {
                tracer.emit(EventKind::NodeStart {
                    flow: name.into(),
                    node: node.into(),
                    node_type: flow.nodes[node].ty.as_str().into(),
                    backend: backend.into(),
                    model: model.into(),
                });
                let body = request_body(model, input, node, &question).to_string();
                tracer.emit(EventKind::BackendRequest { backend: backend.into(), model: model.into(), questions: 1 });
                let asked = Instant::now();
                let result = io.backend(state, backend, node, Bytes::from(body)).await;
                let duration_ms = asked.elapsed().as_millis() as u64;
                let (reply, given) = judge(tracer, backend, flow, node, result, duration_ms);
                if let Some(step) = run.answer(reply, duration_ms) {
                    // Un fallo de backend no aporta confianza: jamás se inventa una.
                    if let (Some(_), Some(p)) = (step.confidence, given) {
                        io.note_confidence(p);
                    }
                    tracer.emit(EventKind::NodeEnd {
                        flow: name.into(),
                        node: node.into(),
                        answer: step.answer.clone(),
                        confidence: step.confidence.unwrap_or(0.0),
                        port: step.port.clone(),
                    });
                }
            }
            Next::BatchAsk { node, backend, model, questions } => {
                tracer.emit(EventKind::NodeStart {
                    flow: name.into(),
                    node: node.into(),
                    node_type: flow.nodes[node].ty.as_str().into(),
                    backend: backend.into(),
                    model: model.into(),
                });
                // D3: cada pregunta viaja con el nombre del nodo por delante, para que dos nodos `batch`
                // del mismo flujo no puedan pisarse las claves en una misma petición.
                let named: Map<String, Value> = questions
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(question, body)| (format!("{node}.{question}"), body.clone()))
                    .collect();
                let body = request_body_many(model, input, named.clone()).to_string();
                // Una sola petición al backend para todas las preguntas del nodo: solo viaja su cuenta.
                tracer.emit(EventKind::BackendRequest {
                    backend: backend.into(),
                    model: model.into(),
                    questions: named.len() as u32,
                });
                let asked = Instant::now();
                let names: Vec<String> = named.keys().cloned().collect();
                let result = io.backend_many(state, backend, node, &names, Bytes::from(body)).await;
                let duration_ms = asked.elapsed().as_millis() as u64;
                let (replies, given) = judge_many(tracer, backend, flow, node, result, duration_ms);
                if let Some(step) = run.answer_batch(replies, duration_ms) {
                    // La confianza que se anota es la media de las dadas a cada pregunta (ver `answer_batch`).
                    if let (Some(_), Some(p)) = (step.confidence, given) {
                        io.note_confidence(p);
                    }
                    tracer.emit(EventKind::NodeEnd {
                        flow: name.into(),
                        node: node.into(),
                        answer: step.answer.clone(),
                        confidence: step.confidence.unwrap_or(0.0),
                        port: step.port.clone(),
                    });
                    if let Some(event) = audit(name, step) {
                        tracer.emit(event);
                    }
                }
            }
            Next::Outcome { outcome, data } => {
                let path = run.path().to_vec();
                tracer.emit(EventKind::DecisionMade {
                    flow: name.into(),
                    outcome: outcome.into(),
                    path_len: path.len() as u32,
                    duration_ms: started.elapsed().as_millis() as u64,
                });
                return Ran { finish: Finish::Done { outcome: outcome.into(), data }, path };
            }
            Next::Failed { node, error, rule } => {
                let message = format!("{node}: {error}");
                // El error de una regla puede citar la entrada: al bus solo llega una frase genérica.
                let (stage, public) = if rule {
                    ("rule", format!("la regla {node} no pudo evaluarse"))
                } else {
                    ("backend", message.clone())
                };
                tracer.emit(EventKind::Error { stage: stage.into(), message: public });
                return Ran { finish: Finish::Failed { message }, path: run.path().to_vec() };
            }
        }
    }
}

/// Probabilidad que el modelo dio a la respuesta que dio (no la concentración de su distribución):
/// `max(p, 1 − p)` en noul; `probabilities[choice]` en choice si vinieron, y si no la `confidence`; la `confidence` en score.
fn given_probability(ty: &str, entry: &Value, reply: &Reply) -> f64 {
    match ty {
        "noul" => reply.answer.as_f64().map_or(reply.confidence, |p| p.max(1.0 - p)),
        "choice" => reply.answer.as_str().and_then(|c| entry["probabilities"][c].as_f64()).unwrap_or(reply.confidence),
        _ => reply.confidence,
    }
}

/// Emite `BACKEND_RESPONSE` y traduce lo recibido en la respuesta del nodo, o en un fallo; la segunda pieza
/// es la probabilidad de la respuesta dada (ver `given_probability`).
fn judge(
    tracer: &Tracer,
    backend: &str,
    flow: &Flow,
    node: &str,
    result: Result<(u16, Bytes), String>,
    duration_ms: u64,
) -> (Result<Reply, String>, Option<f64>) {
    let value = match judge_value(tracer, backend, result, duration_ms) {
        Ok(value) => value,
        Err(e) => return (Err(e), None),
    };
    let n = &flow.nodes[node];
    match reply_from_response(node, n, &value) {
        Ok(reply) => {
            let p = given_probability(n.ty.as_str(), &value["answers"][node], &reply);
            (Ok(reply), Some(p))
        }
        Err(e) => (Err(e), None),
    }
}

/// Traduce una respuesta con **todas** las preguntas de un nodo `batch` en los cuerpos de cada una; la
/// segunda pieza es la media de las probabilidades dadas, que es la confianza que se anota.
fn judge_many(
    tracer: &Tracer,
    backend: &str,
    flow: &Flow,
    node: &str,
    result: Result<(u16, Bytes), String>,
    duration_ms: u64,
) -> (Result<Map<String, Value>, String>, Option<f64>) {
    let value = match judge_value(tracer, backend, result, duration_ms) {
        Ok(value) => value,
        Err(e) => return (Err(e), None),
    };
    let n = &flow.nodes[node];
    let mut bodies = Map::new();
    let mut total = 0.0;
    for question in n.questions.keys() {
        let key = format!("{node}.{question}");
        let Some(entry) = value["answers"].get(&key).cloned() else {
            return (Err(format!("la respuesta no trae {key}")), None);
        };
        // La probabilidad se mide por pregunta, con el mismo criterio que un nodo suelto.
        let single = json!({ "answers": { question: entry.clone() } });
        match reply_from_response(question, &n.questions[question], &single) {
            Ok(reply) => total += given_probability(n.questions[question].ty.as_str(), &entry, &reply),
            Err(e) => return (Err(format!("{question}: {e}")), None),
        }
        bodies.insert(question.clone(), entry);
    }
    let mean = total / bodies.len().max(1) as f64;
    (Ok(bodies), Some(mean))
}

/// Emite `BACKEND_RESPONSE` con lo que haga falta saber de la respuesta y devuelve el JSON ya leído.
fn judge_value(
    tracer: &Tracer,
    backend: &str,
    result: Result<(u16, Bytes), String>,
    duration_ms: u64,
) -> Result<Value, String> {
    let response = |ok, status, input_tokens| EventKind::BackendResponse {
        backend: backend.into(),
        ok,
        status,
        duration_ms,
        input_tokens,
    };
    let (status, bytes) = match result {
        Err(message) => {
            tracer.emit(response(false, 0, None));
            return Err(message);
        }
        Ok(done) => done,
    };
    let ok = (200..300).contains(&status);
    let parsed = serde_json::from_slice::<Value>(&bytes);
    let tokens = parsed.as_ref().ok().and_then(|v| v["usage"]["input_tokens"].as_u64());
    tracer.emit(response(ok, status, tokens));
    if !ok {
        return Err(format!("HTTP {status}"));
    }
    parsed.map_err(|e| format!("respuesta no JSON: {e}"))
}

/// `NODE_AUDIT` de un paso que leyó heurísticas o ganó por una regla `route`; nada si no hay qué contar.
fn audit(flow: &str, step: &PathStep) -> Option<EventKind> {
    (!step.signals.is_empty() || step.rule.is_some()).then(|| EventKind::NodeAudit {
        flow: flow.into(),
        node: step.node.clone(),
        signals: step.signals.clone(),
        rule: step.rule.map(|r| r as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(answer: Value, confidence: f64) -> Reply {
        Reply { answer, confidence }
    }

    #[test]
    fn noul_confidence_is_probability_of_given_answer() {
        let p = |x: f64| given_probability("noul", &json!({}), &reply(json!(x), 0.99));
        assert!((p(0.2) - 0.8).abs() < 1e-12);
        assert!((p(0.9) - 0.9).abs() < 1e-12);
    }

    #[test]
    fn choice_confidence_uses_probability_of_choice() {
        let with = json!({ "probabilities": { "a": 0.3, "b": 0.7 } });
        assert_eq!(given_probability("choice", &with, &reply(json!("b"), 0.4)), 0.7);
        assert_eq!(given_probability("choice", &json!({}), &reply(json!("b"), 0.4)), 0.4);
        assert_eq!(given_probability("score", &with, &reply(json!(0.5), 0.6)), 0.6);
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    fn step(signals: Map<String, Value>, rule: Option<usize>) -> PathStep {
        PathStep {
            node: "risks".into(),
            backend: "kev".into(),
            model: "m".into(),
            answer: Value::Null,
            confidence: None,
            port: "ask".into(),
            duration_ms: 0,
            signals,
            rule,
        }
    }

    #[test]
    fn solo_se_audita_lo_que_hay_que_contar() {
        assert_eq!(audit("f", &step(Map::new(), None)), None);
        let signals = Map::from_iter([("rm_root".to_string(), Value::Bool(false))]);
        assert_eq!(
            audit("f", &step(signals.clone(), Some(1))),
            Some(EventKind::NodeAudit { flow: "f".into(), node: "risks".into(), signals, rule: Some(1) })
        );
    }
}
