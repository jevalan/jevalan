//! Replay and import of recorded sessions (`jevalan sessions replay|import`).
//!
//! A recorded trace holds the input of a capability, the backend and specialist answers it got and the decision it
//! took. Replaying runs the same `execute` path as `POST /v1/hub`, in process, with `Transport::Replay` fed from
//! those answers, and compares the decision. Importing writes the trace as a laboratory fixture.

use crate::write::fnv1a64;
use jevalan_daemon::AppState;
use jevalan_daemon::config::Config;
use jevalan_daemon::hub::execute::execute;
use jevalan_daemon::hub::transport::{FAILURE, Recordings, Transport};
use jevalan_daemon::session::log::Trace;
use jevalan_flow::NodeType;
use jevalan_hub::capability::{Capability, Provider};
use jevalan_hub::envelope::parse_request;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// Flows and capabilities loaded like `serve` does, without a server, a watcher or a session log of its own: a replay
/// must never write into the log it reads.
pub fn load_state(cfg: &Config) -> AppState {
    let mut cfg = cfg.clone();
    cfg.sessions.enabled = false;
    let state = AppState::new(cfg);
    state.flows.load_dir(&state.cfg.flows_dir, &state.cfg, &state.bus);
    state.capabilities.load_dir(&state.cfg.hub.capabilities_dir, &state.flows, &state.cfg.hub, &state.bus);
    state
}

/// How a replayed trace compares with what was recorded.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    /// The same status and the same decision.
    Same,
    /// The replay decided otherwise.
    Different { expected: Value, got: Value },
    /// An answer the run asked for was not recorded (`missing_recording:<key>`).
    Missing { key: String },
    /// Not replayable, and why.
    Skipped { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Replayed {
    pub trace_id: String,
    pub capability: String,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// The answers of a trace as `Transport::Replay` wants them: per node (or `"<node>.<question>"` for a `batch`) the
/// backend's `{status, body}`, and per provider id the specialist's JSON.
fn recordings(trace: &Trace) -> Recordings {
    let mut rec = Recordings::new();
    for call in trace.all("BACKEND_CALL") {
        let node = call["node"].as_str().unwrap_or_default();
        let (response, request) = (&call["response"], &call["request"]);
        // A call that failed in transport replays as status 0, which the flow judges as a failed backend.
        let status = response["status"].as_u64().unwrap_or(0);
        let body = response.get("body").cloned().unwrap_or(Value::Null);
        let keys: Vec<&String> = request["questions"]
            .as_object()
            .into_iter()
            .flat_map(|q| q.keys())
            .filter(|k| k.starts_with(&format!("{node}.")))
            .collect();
        if keys.is_empty() {
            rec.insert(node.into(), json!({ "status": status, "body": body }));
        } else {
            // A `batch` is replayed question by question.
            for key in keys {
                let kept = match body["answers"].get(key) {
                    Some(answer) => json!({ "answers": { key: answer }, "model": body["model"] }),
                    None => body.clone(),
                };
                rec.insert(key.clone(), json!({ "status": status, "body": kept }));
            }
        }
    }
    for call in trace.all("SPECIALIST_CALL") {
        if let Some(key) = call["key"].as_str() {
            let kept = match call.get("failure") {
                Some(failure) => json!({ FAILURE: failure }),
                None => call["response"].clone(),
            };
            rec.insert(key.into(), kept);
        }
    }
    rec
}

/// The capability, input and recorded decision of a trace, or why it cannot be replayed.
fn parts<'a>(state: &AppState, trace: &'a Trace) -> Result<(Arc<Capability>, &'a Value, &'a Value), String> {
    let input = trace.first("INPUT").ok_or("la traza no tiene entrada registrada")?;
    let name = input["capability"].as_str().unwrap_or_default();
    if name.starts_with("flow:") {
        return Err("traza de /v1/decide: solo se reproducen las capacidades del hub".into());
    }
    let cap = state.capabilities.get(name).ok_or_else(|| format!("la capacidad {name} no está cargada"))?;
    let decision = trace.first("DECISION").ok_or("la traza no tiene decisión registrada")?;
    Ok((cap, &input["input"], decision))
}

fn request(cap: &Capability, id: &str, input: &Value) -> Result<jevalan_hub::envelope::HubRequest, String> {
    let body = json!({
        "protocol_version": "jev-hub/0.1", "request_id": id,
        "capability": cap.name, "capability_version": cap.version, "input": input,
    });
    parse_request(body.to_string().as_bytes())
}

/// Replays every trace and compares the decision with the recorded one. No network, no processes.
pub async fn replay(state: &AppState, traces: &[Trace]) -> Vec<Replayed> {
    let mut out = Vec::new();
    for trace in traces {
        let capability = trace.first("INPUT").and_then(|i| i["capability"].as_str()).unwrap_or("?").to_string();
        let outcome = match parts(state, trace) {
            Err(reason) => Outcome::Skipped { reason },
            Ok((cap, input, recorded)) => match request(&cap, &trace.trace_id, input) {
                Err(reason) => Outcome::Skipped { reason },
                Ok(req) => {
                    let transport = Transport::Replay(Arc::new(recordings(trace)));
                    let done = execute(state, &req, Some(&cap), None, &transport).await;
                    match done.missing {
                        Some(key) => Outcome::Missing { key },
                        None => compare(recorded, &done.response),
                    }
                }
            },
        };
        out.push(Replayed { trace_id: trace.trace_id.clone(), capability, outcome });
    }
    out
}

/// Status, and the decision when there is one, must match what the log recorded.
fn compare(recorded: &Value, response: &Value) -> Outcome {
    let expected = json!({ "status": recorded["status"], "decision": recorded.get("decision").cloned() });
    let got = json!({ "status": response["status"], "decision": response.get("decision").cloned() });
    if expected == got { Outcome::Same } else { Outcome::Different { expected, got } }
}

/// One line per trace and a verdict for the whole session.
pub fn render(results: &[Replayed]) -> String {
    let mut out = String::new();
    for r in results {
        let detail = match &r.outcome {
            Outcome::Same => "igual".to_string(),
            Outcome::Different { expected, got } => format!("DISTINTA: grabado {expected} · ahora {got}"),
            Outcome::Missing { key } => format!("falta la grabación de {key}"),
            Outcome::Skipped { reason } => format!("omitida: {reason}"),
        };
        out.push_str(&format!("{} ({}): {detail}\n", r.trace_id, r.capability));
    }
    let count = |f: fn(&Outcome) -> bool| results.iter().filter(|r| f(&r.outcome)).count();
    out.push_str(&format!(
        "{} trazas: {} iguales, {} distintas, {} sin grabación, {} omitidas — sesión {}",
        results.len(),
        count(|o| matches!(o, Outcome::Same)),
        count(|o| matches!(o, Outcome::Different { .. })),
        count(|o| matches!(o, Outcome::Missing { .. })),
        count(|o| matches!(o, Outcome::Skipped { .. })),
        if passed(results) { "reproducida" } else { "NO reproducida" },
    ));
    out
}

/// A session passes when something was replayed and nothing differed or lacked a recording.
pub fn passed(results: &[Replayed]) -> bool {
    results.iter().any(|r| r.outcome == Outcome::Same)
        && results.iter().all(|r| matches!(r.outcome, Outcome::Same | Outcome::Skipped { .. }))
}

/// What an import did.
#[derive(Debug, Default, PartialEq)]
pub struct Imported {
    pub written: usize,
    pub identical: usize,
    pub skipped: Vec<String>,
    pub lab_yaml_written: bool,
}

const LAB_YAML: &str = "\
# Banco de fixtures importados de sesiones grabadas (`jevalan sessions import`).
# Sus fixtures son de modo `offline`: reproducen decisiones ya grabadas, no miden a ningún modelo.
# Los targets son opcionales: `offline` no necesita ninguno.
targets: {}
";

/// Decision fields that say what was decided; the free text (`reason`, messages) is left out on purpose.
const VERDICT_KEYS: [&str; 5] = ["verdict", "decision", "outcome", "stop", "source"];

/// Writes each replayable trace of `traces` as a fixture of `lab_dir`, with its answers as `replay`, and creates
/// `lab.yaml` if the bench has none. Call `Lab::load(..).check()` afterwards to prove the bench still loads.
pub fn import(state: &AppState, traces: &[Trace], lab_dir: &Path) -> Result<Imported, String> {
    std::fs::create_dir_all(lab_dir).map_err(|e| format!("{}: {e}", lab_dir.display()))?;
    let mut done = Imported::default();
    let lab_yaml = lab_dir.join("lab.yaml");
    if !lab_yaml.is_file() {
        std::fs::write(&lab_yaml, LAB_YAML).map_err(|e| format!("{}: {e}", lab_yaml.display()))?;
        done.lab_yaml_written = true;
    }
    for trace in traces {
        let (cap, input, recorded) = match parts(state, trace) {
            Ok(p) => p,
            Err(reason) => {
                done.skipped.push(format!("{}: {reason}", trace.trace_id));
                continue;
            }
        };
        let replay = match replay_rules(state, &cap, trace) {
            Ok(r) => r,
            Err(reason) => {
                done.skipped.push(format!("{}: {reason}", trace.trace_id));
                continue;
            }
        };
        let id = format!("session-{}", &fnv1a64(&format!("{}{}", trace.trace_id, input)));
        let yaml = fixture_yaml(&id, trace, recorded, input, &replay)?;
        let dir = lab_dir.join("fixtures").join(&cap.name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("{id}.yaml"));
        if std::fs::read_to_string(&path).is_ok_and(|t| t == yaml) {
            done.identical += 1;
        } else {
            std::fs::write(&path, yaml).map_err(|e| format!("{}: {e}", path.display()))?;
            done.written += 1;
        }
    }
    Ok(done)
}

/// The fixture's `replay` map: the specialist's JSON under its provider id, or per question of the flow its answer
/// and confidence, the way `lab check` requires them.
fn replay_rules(state: &AppState, cap: &Capability, trace: &Trace) -> Result<BTreeMap<String, Value>, String> {
    let rec = recordings(trace);
    let mut out = BTreeMap::new();
    match &cap.provider {
        Provider::Flow { flow } => {
            let flow = state.flows.get(flow).ok_or("el flujo de la capacidad no está cargado")?;
            for (name, node) in flow.nodes.iter().filter(|(_, n)| n.ty != NodeType::Rule) {
                let questions: Vec<(String, NodeType)> = match node.ty {
                    NodeType::Batch => node.questions.iter().map(|(q, n)| (format!("{name}.{q}"), n.ty)).collect(),
                    ty => vec![(name.clone(), ty)],
                };
                for (key, ty) in questions {
                    // A node the run never reached (another branch of the flow) has no recording, and needs none.
                    let Some(reply) = rec.get(&key) else { continue };
                    let entry = &reply["body"]["answers"][&key];
                    let (answer, confidence) = (&entry[ty.as_str()], entry["confidence"].as_f64());
                    let Some(confidence) = confidence.filter(|_| !answer.is_null()) else {
                        return Err(format!("la respuesta de {key} no se puede reducir a answer y confidence"));
                    };
                    out.insert(key, json!({ "answer": answer, "confidence": confidence }));
                }
            }
        }
        _ => match rec.get(&cap.provider_id) {
            Some(reply) if reply.get(FAILURE).is_some() => {
                return Err("el especialista falló: se reproduce con `sessions replay`, no es una fixture".into());
            }
            Some(reply) => {
                out.insert(cap.provider_id.clone(), reply.clone());
            }
            None => return Err("no hay respuesta del especialista registrada".into()),
        },
    }
    Ok(out)
}

fn fixture_yaml(
    id: &str,
    trace: &Trace,
    recorded: &Value,
    input: &Value,
    replay: &BTreeMap<String, Value>,
) -> Result<String, String> {
    let status = recorded["status"].as_str().unwrap_or("error");
    let mut expect = vec![json!({
        "cel": format!("status == '{status}'"),
        "why": format!("La sesión grabada terminó con estado {status}"),
    })];
    if let Some(decision) = recorded["decision"].as_object() {
        for key in VERDICT_KEYS {
            let cel = match &decision.get(key) {
                Some(Value::String(s)) if !s.contains('\'') => format!("decision.{key} == '{s}'"),
                Some(Value::Bool(b)) => format!("decision.{key} == {b}"),
                _ => continue,
            };
            expect.push(json!({ "cel": cel, "why": format!("«{key}» grabado en la sesión debe reproducirse") }));
        }
    }
    let client = trace.lines.iter().find_map(|l| l["client"].as_str()).unwrap_or("unknown");
    let fixture = json!({
        "id": id,
        "version": 1,
        "description": format!("Importado de la sesión {client}, traza {} ({})", trace.trace_id, trace.start()),
        "modes": ["offline"],
        "input": input,
        "replay": replay,
        "expect": expect,
    });
    serde_yaml::to_string(&fixture).map_err(|e| e.to_string())
}
