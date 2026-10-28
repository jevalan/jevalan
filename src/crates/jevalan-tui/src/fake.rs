//! Generador de trazas ficticias: una parte pura y determinista, y otra con hilo.

use crate::screen::Feed;
use jevalan_events::{Endpoint, Event, EventKind, SCHEMA_VERSION, new_event, rfc3339};
use jevalan_flow::{Criteria, Flow, Next, NodeType, Reply, Run, parse};
use serde_json::{Map, Value, json};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// Flujos de ejemplo del repositorio, embebidos.
pub const FLOWS: [(&str, &str); 2] = [
    ("agent-guard", include_str!("../../../../config/flows/agent-guard.yaml")),
    ("model-route", include_str!("../../../../config/flows/model-route.yaml")),
];
pub const BACKENDS: [&str; 2] = ["typesafe", "ollama"];
const DEFAULT_BACKEND: &str = "ollama";
/// Los de la topología más `openrouter`, que `agent-wakeup` pide y el tablero simulado no dibuja.
const FLOW_BACKENDS: [&str; 3] = ["typesafe", "ollama", "openrouter"];
/// El flujo que sirve a `agent.wakeup`; no se invoca por `/v1/decide`, así que no entra en `FLOWS`.
const WAKEUP_FLOW: (&str, &str) = ("agent-wakeup", include_str!("../../../../config/flows/agent-wakeup.yaml"));
/// (capacidad, versión, id del proveedor, tipo del proveedor) de las capacidades de ejemplo.
pub const CAPABILITIES: [(&str, &str, &str, &str); 2] =
    [("agent.wakeup", "1", "jevalan-flow", "flow"), ("tool.risk", "1", "toolgate", "exec")];
/// (flujo, descripción, campos de entrada, campos de decisión) de cada capacidad de `CAPABILITIES`, en el mismo orden.
type Detail = (Option<&'static str>, &'static str, &'static [&'static str], &'static [&'static str]);
const DETAILS: [Detail; 2] = [
    (
        Some("agent-wakeup"),
        "¿Merece la pena despertar a un agente dormido?",
        &["event", "from_user", "max_skips", "observation", "skipped", "waiting_for"],
        &["probability", "reason", "wake"],
    ),
    (
        None,
        "Riesgo de una llamada a herramienta",
        &["cwd", "session_id", "tool_input", "tool_name", "tool_use_id"],
        &["probabilities", "reason", "source", "verdict"],
    ),
];
/// Fracción de las trazas que pasan por el hub.
const HUB_SHARE: f64 = 0.3;

/// (cliente, peso, segundos activo, segundos callado); callado 0 = siempre activo.
const RHYTHM: [(&str, u64, u64, u64); 8] = [
    ("crush", 6, 1, 0),
    ("vscode", 5, 1, 0),
    ("cli", 3, 40, 40),
    ("ci-bot", 2, 25, 50),
    ("script-a", 2, 20, 45),
    ("script-b", 2, 30, 35),
    ("notebook", 1, 35, 60),
    ("cron", 1, 15, 45),
];

/// SplitMix64: determinista y sin dependencias.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniforme en [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Entero uniforme en [lo, hi].
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo + 1)
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

/// Flujo con un nodo `batch` de tres preguntas, para el generador de trazas y el grafo.
#[cfg(test)]
pub(crate) const BATCH_FLOW: &str = r#"
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
  deny: { outcome: deny, data: { verdict: deny } }
  ask: { outcome: ask-user }
  abstain: { outcome: abstain }
"#;

/// El flujo de `BATCH_FLOW`, ya validado; solo lo usan las pruebas y el grafo de la TUI.
#[cfg(test)]
pub(crate) fn batch_flow() -> Flow {
    parse("tool-risk", BATCH_FLOW, &FLOW_BACKENDS, DEFAULT_BACKEND).expect("flujo de pruebas válido")
}

/// Los flujos embebidos, ya validados. Son del repositorio: si no validan, es un defecto de compilación.
pub fn flows() -> Vec<Flow> {
    FLOWS
        .iter()
        .map(|(name, yaml)| parse(name, yaml, &FLOW_BACKENDS, DEFAULT_BACKEND).expect("flujo embebido"))
        .collect()
}

/// El flujo de `agent.wakeup`, ya validado.
pub fn wakeup_flow() -> Flow {
    parse(WAKEUP_FLOW.0, WAKEUP_FLOW.1, &FLOW_BACKENDS, DEFAULT_BACKEND).expect("flujo embebido")
}

/// Un `CAPABILITY_LOADED` por capacidad, como los que el daemon reenvía tras los flujos.
pub fn capability_events() -> Vec<Event> {
    CAPABILITIES
        .iter()
        .zip(DETAILS)
        .map(|((name, version, provider, provider_type), (flow, description, input, decision))| {
            let owned = |fields: &[&str]| fields.iter().map(|f| (*f).to_string()).collect();
            let kind = EventKind::CapabilityLoaded {
                capability: (*name).into(),
                version: (*version).into(),
                provider: (*provider).into(),
                provider_type: (*provider_type).into(),
                flow: flow.map(Into::into),
                description: description.into(),
                input_fields: owned(input),
                decision_fields: owned(decision),
            };
            new_event(&format!("capability:{name}:1"), 1, "fake", kind)
        })
        .collect()
}

/// Clientes activos en el segundo `elapsed` según su ritmo.
pub fn active_clients(elapsed: Duration) -> Vec<&'static str> {
    let s = elapsed.as_secs();
    RHYTHM
        .iter()
        .enumerate()
        .filter(|(i, (_, _, on, off))| *off == 0 || (s + *i as u64 * 7) % (on + off) < *on)
        .map(|(_, (name, ..))| *name)
        .collect()
}

/// Elige un cliente activo, ponderado por su peso.
pub fn pick_client(rng: &mut Rng, elapsed: Duration) -> &'static str {
    let active = active_clients(elapsed);
    let weight = |n: &str| RHYTHM.iter().find(|r| r.0 == n).map_or(1, |r| r.1);
    let total: u64 = active.iter().map(|n| weight(n)).sum();
    let mut roll = rng.range(0, total - 1);
    for name in &active {
        let w = weight(name);
        if roll < w {
            return name;
        }
        roll -= w;
    }
    active[0]
}

/// Backend caído en `elapsed`: typesafe de 90 a 100 s de cada 180 s; ollama de 40 a 48 s de cada 150 s;
/// toolgate de 120 a 128 s de cada 200 s.
pub fn down_backend(elapsed: Duration) -> Option<&'static str> {
    let s = elapsed.as_secs();
    if (90..100).contains(&(s % 180)) {
        Some("typesafe")
    } else if (40..48).contains(&(s % 150)) {
        Some("ollama")
    } else if (120..128).contains(&(s % 200)) {
        Some("toolgate")
    } else {
        None
    }
}

/// La definición que viaja en `FLOW_LOADED`: jamás lleva las `instructions`, tampoco las de las preguntas
/// de un nodo `batch`, que son el prompt.
fn definition(flow: &Flow) -> Value {
    let mut def = serde_json::to_value(flow).unwrap_or(Value::Null);
    if let Some(nodes) = def.get_mut("nodes").and_then(Value::as_object_mut) {
        for node in nodes.values_mut().filter_map(Value::as_object_mut) {
            node.remove("instructions");
            if let Some(questions) = node.get_mut("questions").and_then(Value::as_object_mut) {
                for question in questions.values_mut().filter_map(Value::as_object_mut) {
                    question.remove("instructions");
                }
            }
        }
    }
    def
}

/// El `BACKENDS` que el daemon envía primero al abrirse una conexión.
pub fn backends_event() -> Event {
    let backends = BACKENDS.iter().map(|b| b.to_string()).collect();
    new_event("backends:1", 1, "fake", EventKind::Backends { backends })
}

/// Un `FLOW_LOADED` por flujo, como el que el daemon envía al abrirse una conexión.
pub fn loaded_events(flows: &[Flow]) -> Vec<Event> {
    flows
        .iter()
        .map(|f| {
            let kind =
                EventKind::FlowLoaded { flow: f.name.clone(), nodes: f.nodes.len() as u32, definition: definition(f) };
            new_event(&format!("flow:{}:1", f.name), 1, "fake", kind)
        })
        .collect()
}

/// Latencia de un backend ficticio, en ms.
fn latency(rng: &mut Rng, backend: &str) -> u64 {
    if backend == "typesafe" { rng.range(80, 250) } else { rng.range(20, 120) }
}

/// Respuesta de una pregunta: la opción elegida, la puntuación o la probabilidad de «true», con su confianza.
fn question_reply(rng: &mut Rng, ty: NodeType, criteria: Option<&Criteria>) -> Reply {
    let answer = match (ty, criteria) {
        (NodeType::Choice, Some(Criteria::Options(options))) => {
            let i = rng.range(0, options.len() as u64 - 1) as usize;
            json!(options.keys().nth(i))
        }
        (NodeType::Score, Some(Criteria::Levels(levels))) => json!(rng.unit() * (levels.len() - 1) as f64),
        _ => json!(rng.unit()),
    };
    let answer = answer.as_f64().map_or(answer.clone(), |x| json!(round3(x)));
    Reply { answer, confidence: round3(0.4 + 0.6 * rng.unit()) }
}

/// Respuesta aleatoria pero determinista a la pregunta de un nodo. En un `batch`, una por pregunta.
fn random_reply(rng: &mut Rng, flow: &Flow, node: &str) -> Reply {
    let n = &flow.nodes[node];
    if n.ty == NodeType::Batch {
        // El flujo espera un objeto con todas las respuestas, no un valor suelto.
        let answers: Map<String, Value> = n
            .questions
            .iter()
            .map(|(question, q)| (question.clone(), question_reply(rng, q.ty, q.criteria.as_ref()).answer))
            .collect();
        return Reply { answer: Value::Object(answers), confidence: round3(0.4 + 0.6 * rng.unit()) };
    }
    question_reply(rng, n.ty, n.criteria.as_ref())
}

/// Tres decimales: bastan y viajan por JSON sin perder ni una cifra.
fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Respuestas de cada pregunta de un nodo `batch` con la forma que devuelve System One:
/// `{ "<pregunta>": {type, <tipo>, confidence} }`, a partir del objeto que da `random_reply`.
fn batch_entries(answer: &Value, flow: &Flow, node: &str) -> Map<String, Value> {
    let n = &flow.nodes[node];
    n.questions
        .iter()
        .filter_map(|(question, q)| {
            let value = answer.get(question)?.clone();
            let entry = match q.ty {
                NodeType::Choice => json!({ "type": "choice", "choice": value, "confidence": 0.9 }),
                NodeType::Score => json!({ "type": "score", "score": value, "confidence": 0.9 }),
                _ => json!({ "type": "noul", "noul": value, "confidence": 0.9 }),
            };
            Some((question.clone(), entry))
        })
        .collect()
}

/// Acumula los eventos de una traza con su desfase y su `seq`.
struct Out<'a> {
    trace_id: &'a str,
    events: Vec<(Duration, Event)>,
}

impl Out<'_> {
    fn push(&mut self, t: u64, kind: EventKind) {
        let seq = self.events.len() as u64 + 1;
        let event = Event {
            v: SCHEMA_VERSION,
            id: format!("{}-{seq}", self.trace_id),
            ts: String::new(),
            trace_id: self.trace_id.to_string(),
            seq,
            source: "fake".into(),
            kind,
        };
        self.events.push((Duration::from_millis(t), event));
    }
}

/// Genera una traza completa; cada evento va con su desfase desde el inicio de la traza.
/// El 85 % recorre un flujo con la misma secuencia de eventos que el daemon; el resto es passthrough.
pub fn trace(
    rng: &mut Rng,
    trace_id: &str,
    client: &str,
    flows: &[Flow],
    down: Option<&str>,
) -> Vec<(Duration, Event)> {
    let mut out = Out { trace_id, events: Vec::new() };
    if rng.chance(0.85) && !flows.is_empty() {
        let flow = &flows[rng.range(0, flows.len() as u64 - 1) as usize];
        decide(&mut out, rng, client, flow, down);
    } else {
        passthrough(&mut out, rng, client, down);
    }
    out.events
}

fn decide(out: &mut Out, rng: &mut Rng, client: &str, flow: &Flow, down: Option<&str>) {
    let received = EventKind::RequestReceived {
        client: client.into(),
        endpoint: Endpoint::Decide,
        flow: Some(flow.name.clone()),
        backend: None,
    };
    out.push(0, received);
    walk(out, rng, flow, Value::Null, down);
}

/// Recorre `flow` con la misma secuencia de eventos que el daemon. Devuelve el instante final y si hubo desenlace.
fn walk(out: &mut Out, rng: &mut Rng, flow: &Flow, input: Value, down: Option<&str>) -> (u64, bool) {
    let name = flow.name.as_str();
    let mut t = 0u64;
    let mut run = Run::new(flow, input);
    loop {
        match run.next() {
            Next::Ruled(step) => {
                t += 1;
                let start = EventKind::NodeStart {
                    flow: name.into(),
                    node: step.node.clone(),
                    node_type: "rule".into(),
                    backend: step.backend.clone(),
                    model: step.model.clone(),
                };
                out.push(t, start);
                let end = EventKind::NodeEnd {
                    flow: name.into(),
                    node: step.node,
                    answer: step.answer,
                    confidence: step.confidence.unwrap_or(1.0),
                    port: step.port,
                };
                out.push(t, end);
            }
            Next::Ask { node, backend, model, .. } => {
                t += rng.range(1, 5);
                out.push(
                    t,
                    EventKind::NodeStart {
                        flow: name.into(),
                        node: node.into(),
                        node_type: flow.nodes[node].ty.as_str().into(),
                        backend: backend.into(),
                        model: model.into(),
                    },
                );
                out.push(t, EventKind::BackendRequest { backend: backend.into(), model: model.into(), questions: 1 });
                let ok = down != Some(backend) && !rng.chance(0.03);
                let dur = if ok { latency(rng, backend) } else { 800 };
                t += dur;
                let status = if ok { 200 } else { 0 };
                out.push(
                    t,
                    EventKind::BackendResponse {
                        backend: backend.into(),
                        ok,
                        status,
                        duration_ms: dur,
                        input_tokens: None,
                    },
                );
                let reply = if ok { Ok(random_reply(rng, flow, node)) } else { Err("timeout".to_string()) };
                if let Some(step) = run.answer(reply, dur) {
                    let end = EventKind::NodeEnd {
                        flow: name.into(),
                        node: node.into(),
                        answer: step.answer.clone(),
                        confidence: step.confidence.unwrap_or(0.0),
                        port: step.port.clone(),
                    };
                    out.push(t, end);
                }
            }
            Next::BatchAsk { node, backend, model, questions } => {
                t += rng.range(1, 5);
                out.push(
                    t,
                    EventKind::NodeStart {
                        flow: name.into(),
                        node: node.into(),
                        node_type: flow.nodes[node].ty.as_str().into(),
                        backend: backend.into(),
                        model: model.into(),
                    },
                );
                // Una sola petición con todas las preguntas del nodo: el evento lleva su cuenta.
                let asked = questions.as_object().map_or(0, |q| q.len()) as u32;
                out.push(
                    t,
                    EventKind::BackendRequest { backend: backend.into(), model: model.into(), questions: asked },
                );
                let ok = down != Some(backend) && !rng.chance(0.03);
                let dur = if ok { latency(rng, backend) } else { 800 };
                t += dur;
                out.push(
                    t,
                    EventKind::BackendResponse {
                        backend: backend.into(),
                        ok,
                        status: if ok { 200 } else { 0 },
                        duration_ms: dur,
                        input_tokens: None,
                    },
                );
                // `random_reply` da el objeto de respuestas; `answer_batch` espera el cuerpo de cada pregunta.
                let reply = if ok { Ok(random_reply(rng, flow, node)) } else { Err("timeout".to_string()) };
                if let Some(step) = run.answer_batch(reply.map(|r| batch_entries(&r.answer, flow, node)), dur) {
                    let end = EventKind::NodeEnd {
                        flow: name.into(),
                        node: node.into(),
                        answer: step.answer.clone(),
                        confidence: step.confidence.unwrap_or(0.0),
                        port: step.port.clone(),
                    };
                    out.push(t, end);
                }
            }
            Next::Outcome { outcome, .. } => {
                t += 1;
                let made = EventKind::DecisionMade {
                    flow: name.into(),
                    outcome: outcome.into(),
                    path_len: run.path().len() as u32,
                    duration_ms: t,
                };
                out.push(t, made);
                return (t, true);
            }
            Next::Failed { node, error, .. } => {
                out.push(t, EventKind::Error { stage: "backend".into(), message: format!("{node}: {error}") });
                return (t, false);
            }
        }
    }
}

/// Una entrada plausible para `agent.wakeup`.
fn wakeup_input(rng: &mut Rng) -> Value {
    match rng.range(0, 3) {
        0 => json!({ "waiting_for": "respuesta del usuario", "from_user": true, "skipped": 0 }),
        1 => json!({ "waiting_for": "fin del build", "skipped": 2 }),
        2 => json!({ "waiting_for": "fin del build", "event": "build terminado", "skipped": 25, "max_skips": 10 }),
        _ => json!({ "waiting_for": "un commit nuevo", "event": "push a main", "skipped": 1 }),
    }
}

/// Traza de una petición al hub con la misma secuencia de eventos que el daemon: mitad `agent.wakeup`
/// (proveedor `flow`, con los eventos del flujo en la misma traza) y mitad `tool.risk` (especialista `exec`).
pub fn hub_trace(
    rng: &mut Rng,
    trace_id: &str,
    client: &str,
    wakeup: &Flow,
    down: Option<&str>,
) -> Vec<(Duration, Event)> {
    let mut out = Out { trace_id, events: Vec::new() };
    let (capability, version, provider, kind) = CAPABILITIES[usize::from(!rng.chance(0.5))];
    let request = EventKind::HubRequest {
        capability: capability.into(),
        capability_version: version.into(),
        provider: provider.into(),
        client: client.into(),
    };
    out.push(0, request);
    let (t, status, code) = if kind == "flow" {
        let input = wakeup_input(rng);
        match walk(&mut out, rng, wakeup, input, down) {
            (t, true) => (t, "ok", None),
            (t, false) => (t, "error", Some("provider_failed")),
        }
    } else {
        out.push(1, EventKind::BackendRequest { backend: provider.into(), model: "-".into(), questions: 1 });
        let ok = down != Some(provider) && !rng.chance(0.03);
        let dur = if ok { rng.range(30, 200) } else { 1500 };
        let t = 1 + dur;
        let answered = EventKind::BackendResponse {
            backend: provider.into(),
            ok,
            status: 0,
            duration_ms: dur,
            input_tokens: None,
        };
        out.push(t, answered);
        match (ok, rng.range(0, 9)) {
            (false, _) => (t, "error", Some("provider_failed")),
            (true, 0..=1) => (t, "abstain", None),
            (true, _) => (t, "ok", None),
        }
    };
    let response = EventKind::HubResponse {
        capability: capability.into(),
        provider: provider.into(),
        status: status.into(),
        code: code.map(Into::into),
        elapsed_ms: t,
    };
    out.push(t, response);
    out.events
}

fn passthrough(out: &mut Out, rng: &mut Rng, client: &str, down: Option<&str>) {
    let backend = BACKENDS[rng.range(0, BACKENDS.len() as u64 - 1) as usize];
    let model = if backend == "typesafe" { "jev-latest" } else { "nimble" };
    let mut t = 0u64;
    let received = EventKind::RequestReceived {
        client: client.into(),
        endpoint: Endpoint::Systemone,
        flow: None,
        backend: Some(backend.into()),
    };
    out.push(t, received);
    t += 1;
    out.push(t, EventKind::BackendRequest { backend: backend.into(), model: model.into(), questions: 1 });
    let ok = down != Some(backend) && !rng.chance(0.03);
    let dur = if ok { latency(rng, backend) } else { 800 };
    t += dur;
    let status = if ok { 200 } else { 0 };
    out.push(
        t,
        EventKind::BackendResponse { backend: backend.into(), ok, status, duration_ms: dur, input_tokens: None },
    );
    let done = EventKind::PassthroughDone {
        backend: backend.into(),
        model: model.into(),
        status: if ok { 200 } else { 502 },
        duration_ms: dur,
    };
    out.push(t, done);
}

/// Lanza un hilo que emite trazas ficticias a `rate` peticiones por segundo, cada
/// evento en su momento. El hilo termina cuando se suelta el `Receiver`.
pub fn spawn(rate: f64, seed: u64) -> Receiver<Feed> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || emit(tx, rate.max(0.1), seed));
    rx
}

fn emit(tx: Sender<Feed>, rate: f64, seed: u64) {
    let flows = flows();
    let wakeup = wakeup_flow();
    let announced = std::iter::once(backends_event())
        .chain(loaded_events(&flows))
        .chain(loaded_events(std::slice::from_ref(&wakeup)))
        .chain(capability_events());
    for e in announced {
        if tx.send(Feed::Event(e)).is_err() {
            return;
        }
    }
    let mut rng = Rng::new(seed);
    let start = Instant::now();
    let mut next = start;
    let mut n = 0u64;
    let mut pending: Vec<(Instant, Event)> = Vec::new();
    loop {
        let now = Instant::now();
        if now >= next {
            let elapsed = now - start;
            let client = pick_client(&mut rng, elapsed);
            n += 1;
            let id = format!("t{n:06}");
            let down = down_backend(elapsed);
            let events = if rng.chance(HUB_SHARE) {
                hub_trace(&mut rng, &id, client, &wakeup, down)
            } else {
                trace(&mut rng, &id, client, &flows, down)
            };
            for (offset, e) in events {
                pending.push((now + offset, e));
            }
            next += Duration::from_secs_f64((0.5 + rng.unit()) / rate);
        }
        let (mut due, rest): (Vec<_>, Vec<_>) = pending.into_iter().partition(|(at, _)| *at <= now);
        pending = rest;
        due.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.seq.cmp(&b.1.seq)));
        for (_, mut e) in due {
            e.ts = rfc3339(SystemTime::now());
            if tx.send(Feed::Event(e)).is_err() {
                return;
            }
        }
        let wake = pending.iter().map(|(at, _)| *at).min().map_or(next, |at| at.min(next));
        thread::sleep(wake.saturating_duration_since(Instant::now()).min(Duration::from_millis(50)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(tr: &[(Duration, Event)]) -> Vec<&EventKind> {
        tr.iter().map(|(_, e)| &e.kind).collect()
    }

    #[test]
    fn same_seed_same_events() {
        let flows = flows();
        let a = trace(&mut Rng::new(42), "t1", "crush", &flows, None);
        let b = trace(&mut Rng::new(42), "t1", "crush", &flows, None);
        assert_eq!(a, b);
        assert_eq!(loaded_events(&flows).len(), 2);
    }

    #[test]
    fn starts_with_backends_then_flows() {
        let rx = spawn(1.0, 1);
        let first: Vec<_> = (0..3).map(|_| rx.recv().unwrap()).collect();
        let kinds: Vec<_> = first.iter().map(|f| matches!(f, Feed::Event(e) if e.kind.name() == "BACKENDS")).collect();
        assert_eq!(kinds, [true, false, false]);
        assert!(matches!(&first[1], Feed::Event(e) if e.kind.name() == "FLOW_LOADED"));
        assert!(matches!(&first[2], Feed::Event(e) if e.kind.name() == "FLOW_LOADED"));
    }

    #[test]
    fn decide_traces_end_in_decision_or_error() {
        let flows = flows();
        let (mut decides, mut passthroughs) = (0, 0);
        for seed in 0..100 {
            let tr = trace(&mut Rng::new(seed), "t", "crush", &flows, None);
            let k = kinds(&tr);
            for (i, (_, e)) in tr.iter().enumerate() {
                assert_eq!(e.seq, i as u64 + 1, "seed {seed}");
                assert_eq!(e.trace_id, "t");
            }
            assert!(tr.windows(2).all(|w| w[0].0 <= w[1].0), "seed {seed}: offsets");
            match k[0] {
                EventKind::RequestReceived { endpoint: Endpoint::Decide, .. } => {
                    decides += 1;
                    assert!(
                        matches!(k[k.len() - 1], EventKind::DecisionMade { .. } | EventKind::Error { .. }),
                        "seed {seed}"
                    );
                    // Cada NODE_START va seguido de su NODE_END antes del siguiente NODE_START.
                    let mut open = false;
                    for e in &k {
                        match e {
                            EventKind::NodeStart { .. } => {
                                assert!(!open, "seed {seed}: NODE_START sin cerrar");
                                open = true;
                            }
                            EventKind::NodeEnd { .. } => {
                                assert!(open, "seed {seed}: NODE_END sin NODE_START");
                                open = false;
                            }
                            _ => {}
                        }
                    }
                    assert!(!open, "seed {seed}: NODE_START sin NODE_END");
                }
                EventKind::RequestReceived { endpoint: Endpoint::Systemone, backend: Some(_), .. } => {
                    passthroughs += 1;
                    assert!(matches!(k[k.len() - 1], EventKind::PassthroughDone { .. }), "seed {seed}");
                }
                other => panic!("seed {seed}: primer evento inesperado {other:?}"),
            }
        }
        assert!(decides > 60 && passthroughs > 3, "decides = {decides}, passthroughs = {passthroughs}");
    }

    #[test]
    fn down_backend_always_fails() {
        let flows = flows();
        let mut seen = 0;
        for seed in 0..200 {
            for (_, e) in trace(&mut Rng::new(seed), "t", "crush", &flows, Some("ollama")) {
                if let EventKind::BackendResponse { backend, ok, .. } = &e.kind
                    && backend == "ollama"
                {
                    seen += 1;
                    assert!(!ok, "seed {seed}");
                }
            }
        }
        assert!(seen > 0);
    }

    #[test]
    fn hub_traces_mirror_the_daemon_sequence() {
        let wakeup = wakeup_flow();
        let (mut flow_caps, mut exec_caps, mut statuses) = (0, 0, std::collections::BTreeSet::new());
        for seed in 0..200 {
            let tr = hub_trace(&mut Rng::new(seed), "h", "crush", &wakeup, None);
            let k = kinds(&tr);
            for (i, (_, e)) in tr.iter().enumerate() {
                assert_eq!(e.seq, i as u64 + 1, "seed {seed}");
            }
            assert!(tr.windows(2).all(|w| w[0].0 <= w[1].0), "seed {seed}: offsets");
            let EventKind::HubRequest { capability, provider, client, .. } = k[0] else {
                panic!("seed {seed}: la traza no empieza por HUB_REQUEST");
            };
            let EventKind::HubResponse { capability: answered, status, code, elapsed_ms, .. } = k[k.len() - 1] else {
                panic!("seed {seed}: la traza no acaba en HUB_RESPONSE");
            };
            assert_eq!((answered, client.as_str()), (capability, "crush"));
            assert_eq!(*elapsed_ms, tr[tr.len() - 1].0.as_millis() as u64);
            assert_eq!(code.is_some(), status == "error", "solo el error lleva code");
            statuses.insert(status.clone());
            if capability == "agent.wakeup" {
                flow_caps += 1;
                assert_eq!(provider, "jevalan-flow");
                assert!(k.iter().any(|e| matches!(e, EventKind::NodeStart { flow, .. } if flow == "agent-wakeup")));
                assert!(k.iter().any(|e| matches!(e, EventKind::DecisionMade { .. } | EventKind::Error { .. })));
            } else {
                exec_caps += 1;
                assert_eq!(provider, "toolgate");
                assert!(
                    matches!(k[1], EventKind::BackendRequest { backend, model, .. } if backend == "toolgate" && model == "-")
                );
                assert!(matches!(k[2], EventKind::BackendResponse { backend, .. } if backend == "toolgate"));
                assert_eq!(k.len(), 4);
            }
        }
        assert!(flow_caps > 50 && exec_caps > 50, "flow = {flow_caps}, exec = {exec_caps}");
        assert_eq!(statuses.len(), 3, "salen ok, abstain y error: {statuses:?}");
    }

    #[test]
    fn a_down_specialist_always_fails_with_an_error_status() {
        let wakeup = wakeup_flow();
        let mut seen = 0;
        for seed in 0..100 {
            let tr = hub_trace(&mut Rng::new(seed), "h", "crush", &wakeup, Some("toolgate"));
            let k = kinds(&tr);
            if matches!(k[0], EventKind::HubRequest { capability, .. } if capability == "tool.risk") {
                seen += 1;
                assert!(matches!(k[k.len() - 1], EventKind::HubResponse { status, .. } if status == "error"));
            }
        }
        assert!(seen > 0);
    }

    #[test]
    fn capabilities_are_announced_after_the_flows() {
        let caps = capability_events();
        let names: Vec<&str> = caps.iter().map(|e| e.kind.name()).collect();
        assert_eq!(names, ["CAPABILITY_LOADED", "CAPABILITY_LOADED"]);
        assert_eq!(caps[0].trace_id, "capability:agent.wakeup:1");
        assert_eq!(loaded_events(&[wakeup_flow()])[0].trace_id, "flow:agent-wakeup:1");
    }

    #[test]
    fn fake_events_round_trip_as_json() {
        let flows = flows();
        let mut events = loaded_events(&flows);
        events.extend(loaded_events(&[wakeup_flow()]));
        events.extend(capability_events());
        for seed in 0..50 {
            events.extend(trace(&mut Rng::new(seed), "t", "crush", &flows, None).into_iter().map(|(_, e)| e));
            events
                .extend(hub_trace(&mut Rng::new(seed), "h", "crush", &wakeup_flow(), None).into_iter().map(|(_, e)| e));
        }
        for e in events {
            let text = serde_json::to_string(&e).unwrap();
            assert!(!text.contains("instructions"), "los eventos no llevan instrucciones");
            assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), e);
            if let EventKind::FlowLoaded { definition, .. } = e.kind {
                serde_json::from_value::<Flow>(definition).expect("la definición se lee como flujo");
            }
        }
    }

    #[test]
    fn client_rhythm_shows_silence_and_crowds() {
        let active: Vec<Vec<&str>> = (0..600u64).map(|s| active_clients(Duration::from_secs(s))).collect();
        assert!(active.iter().all(|a| a.contains(&"crush")));
        assert!(active.iter().any(|a| a.len() > 5), "nunca hay más de 5 activos");
        for name in ["cli", "ci-bot", "script-a", "script-b", "notebook", "cron"] {
            let mut run = 0;
            let mut longest = 0;
            for a in &active {
                run = if a.contains(&name) { 0 } else { run + 1 };
                longest = longest.max(run);
            }
            assert!(longest > 30, "{name} nunca calla más de 30 s");
        }
    }

    #[test]
    fn pick_client_only_returns_active_clients() {
        let mut rng = Rng::new(7);
        for s in 0..300 {
            let at = Duration::from_secs(s);
            assert!(active_clients(at).contains(&pick_client(&mut rng, at)));
        }
    }

    #[test]
    fn backends_go_down_on_schedule() {
        assert_eq!(down_backend(Duration::from_secs(95)), Some("typesafe"));
        assert_eq!(down_backend(Duration::from_secs(44)), Some("ollama"));
        assert_eq!(down_backend(Duration::from_secs(124)), Some("toolgate"));
        assert_eq!(down_backend(Duration::from_secs(10)), None);
    }

    /// Flujo con un nodo `batch` de tres preguntas, compartido con la pantalla de flujo.
    fn batch_flow() -> Flow {
        super::batch_flow()
    }

    #[test]
    fn fake_answers_every_question_of_a_batch() {
        let flow = batch_flow();
        let answer = random_reply(&mut Rng::new(3), &flow, "risks").answer;
        let questions = answer.as_object().unwrap_or_else(|| panic!("un batch responde un objeto: {answer}"));
        assert_eq!(questions.keys().collect::<Vec<_>>(), ["destructive", "privilege", "risk"]);
        for (question, value) in questions {
            match question.as_str() {
                "risk" => assert!((0.0..=2.0).contains(&value.as_f64().unwrap()), "{value}"),
                _ => assert!((0.0..=1.0).contains(&value.as_f64().unwrap()), "{value}"),
            }
        }
    }

    #[test]
    fn a_batch_trace_asks_once_for_every_question() {
        let flow = batch_flow();
        let mut out = Out { trace_id: "t", events: Vec::new() };
        let (_, decided) = walk(&mut out, &mut Rng::new(11), &flow, Value::Null, None);
        let kinds = kinds(&out.events);
        assert!(decided);
        assert!(matches!(kinds[0], EventKind::NodeStart { node_type, .. } if node_type == "batch"));
        let requests: Vec<&EventKind> =
            kinds.iter().filter(|k| matches!(k, EventKind::BackendRequest { .. })).copied().collect();
        assert_eq!(requests.len(), 1, "una sola petición por nodo batch");
        assert!(matches!(requests[0], EventKind::BackendRequest { questions: 3, .. }));
        assert_eq!(kinds.iter().filter(|k| matches!(k, EventKind::NodeEnd { .. })).count(), 1);
        assert!(matches!(kinds.last(), Some(EventKind::DecisionMade { path_len: 1, .. })));
    }

    #[test]
    fn spawn_streams_ordered_traces() {
        let rx = spawn(50.0, 1);
        let mut last_seq = std::collections::HashMap::new();
        for _ in 0..40 {
            let Feed::Event(e) = rx.recv_timeout(Duration::from_secs(5)).expect("el generador emite") else {
                panic!("el generador solo emite eventos");
            };
            assert!(e.ts.ends_with('Z') || e.id.starts_with("flow:"));
            let prev = last_seq.insert(e.trace_id.clone(), e.seq).unwrap_or(0);
            assert_eq!(e.seq, prev + 1, "{} llega en orden", e.trace_id);
        }
        assert!(last_seq.keys().any(|k| k.starts_with("flow:agent-guard")));
    }
}
