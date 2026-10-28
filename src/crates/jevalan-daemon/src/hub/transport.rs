//! Por dónde salen las llamadas al exterior: en vivo, reproducidas de una grabación o grabadas al vuelo.

use crate::AppState;
use crate::proxy::call_backend;
use axum::body::Bytes;
use jevalan_hub::classify::{ErrorCode, Status};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

/// Clave: nombre de nodo (`{status, body}` del backend) o `provider.id` (JSON del especialista).
pub type Recordings = BTreeMap<String, Value>;

#[derive(Clone)]
pub enum Transport {
    Live,
    /// Jamás lanza procesos ni abre conexiones.
    Replay(Arc<Recordings>),
    /// Llama en vivo y guarda solo la respuesta: nunca cabeceras ni la petición.
    Record(Arc<Mutex<Recordings>>),
}

/// Respuesta System One de `node` tal como la grabaría un backend; sirve para repetir a mano.
pub fn system_one_reply(node: &str, node_type: &str, answer: &Value, confidence: f64) -> Value {
    json!({ "status": 200, "body": { "answers": { node: { "type": node_type, node_type: answer, "confidence": confidence } } } })
}

/// Respuesta uniendo las grabaciones de cada pregunta de un nodo `batch`: una petición, muchas respuestas.
/// `questions` son las claves completas, `"<nodo>.<pregunta>"`. El estado HTTP lo pone la primera;
/// ninguna grabación, ninguna respuesta.
pub fn replay_many(rec: &Recordings, questions: &[String]) -> Option<(u16, Bytes)> {
    let mut answers = serde_json::Map::new();
    let mut status = 200;
    for (i, key) in questions.iter().enumerate() {
        let hit = rec.get(key)?;
        if i == 0 {
            status = hit["status"].as_u64().unwrap_or(0) as u16;
        }
        for (name, body) in hit["body"]["answers"].as_object().into_iter().flatten() {
            answers.insert(name.clone(), body.clone());
        }
    }
    Some((status, Bytes::from(json!({ "answers": answers }).to_string())))
}

#[derive(Default)]
struct Tally {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    confidence: Option<f64>,
    missing: Option<String>,
    /// Llamadas a backends y, de ellas, las que no devolvieron los dos contadores de `usage`.
    calls: u32,
    unmetered: u32,
    specialist_calls: u32,
    /// Suma de `usage.cost` y llamadas que no lo traían.
    cost: f64,
    uncosted: u32,
}

/// Transporte más lo que se mide durante una ejecución.
pub struct Io<'a> {
    transport: &'a Transport,
    tally: Mutex<Tally>,
}

impl<'a> Io<'a> {
    pub fn new(transport: &'a Transport) -> Self {
        Self { transport, tally: Mutex::default() }
    }

    fn tally(&self) -> std::sync::MutexGuard<'_, Tally> {
        self.tally.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn is_replay(&self) -> bool {
        matches!(self.transport, Transport::Replay(_))
    }

    pub fn specialist_calls(&self) -> u32 {
        self.tally().specialist_calls
    }

    /// Probabilidad de la respuesta dada por el último nodo no `rule`.
    pub fn note_confidence(&self, confidence: f64) {
        self.tally().confidence = Some(confidence);
    }

    pub fn missing(&self) -> Option<String> {
        self.tally().missing.clone()
    }

    pub fn measured(&self) -> (Option<u64>, Option<u64>, Option<f64>) {
        let t = self.tally();
        (t.input_tokens, t.output_tokens, t.confidence)
    }

    /// (llamadas a backends, llamadas sin `usage` completo).
    pub fn calls(&self) -> (u32, u32) {
        let t = self.tally();
        (t.calls, t.unmetered)
    }

    /// Coste en USD que declararon los backends; `None` si no hubo llamadas o alguna no lo trajo.
    pub fn reported_cost(&self) -> Option<f64> {
        let t = self.tally();
        (t.calls > 0 && t.uncosted == 0).then_some(t.cost)
    }

    fn miss(&self, key: &str) -> String {
        self.tally().missing.get_or_insert_with(|| key.to_string());
        format!("missing_recording:{key}")
    }

    /// Llamada a un backend por el transporte; `node` es la clave de la grabación.
    pub async fn backend(
        &self,
        state: &AppState,
        backend: &str,
        node: &str,
        body: Bytes,
    ) -> Result<(u16, Bytes), String> {
        self.backend_keys(state, backend, node, &[], body).await
    }

    /// Como `backend`, pero para un nodo `batch`: la grabación se busca pregunta a pregunta —`questions`
    /// son las claves completas, `"<nodo>.<pregunta>"`— y las respuestas se juntan en una sola (D3).
    pub async fn backend_many(
        &self,
        state: &AppState,
        backend: &str,
        node: &str,
        questions: &[String],
        body: Bytes,
    ) -> Result<(u16, Bytes), String> {
        self.backend_keys(state, backend, node, questions, body).await
    }

    async fn backend_keys(
        &self,
        state: &AppState,
        backend: &str,
        node: &str,
        questions: &[String],
        body: Bytes,
    ) -> Result<(u16, Bytes), String> {
        let result = match self.transport {
            Transport::Replay(rec) if !questions.is_empty() => {
                replay_many(rec, questions).ok_or_else(|| self.miss(&questions[0]))
            }
            Transport::Replay(rec) => {
                let hit = rec.get(node).ok_or_else(|| self.miss(node))?;
                let status = hit["status"].as_u64().unwrap_or(0) as u16;
                Ok((status, Bytes::from(hit["body"].to_string())))
            }
            _ => call_backend(state, backend, body).await,
        };
        // Una llamada fallida o sin `usage` no se puede tasar: el coste de la ejecución deja de ser conocido.
        let mut metered = false;
        let mut cost = None;
        if let Ok((status, bytes)) = &result {
            let parsed = serde_json::from_slice::<Value>(bytes).ok();
            if let Some(usage) = parsed.as_ref().map(|v| &v["usage"]) {
                cost = usage["cost"].as_f64().filter(|c| c.is_finite() && *c >= 0.0);
                metered = usage["input_tokens"].is_u64() && usage["output_tokens"].is_u64();
                let mut guard = self.tally();
                let t = &mut *guard;
                for (slot, key) in [(&mut t.input_tokens, "input_tokens"), (&mut t.output_tokens, "output_tokens")] {
                    if let Some(n) = usage[key].as_u64() {
                        *slot = Some(slot.unwrap_or(0) + n);
                    }
                }
            }
            if let (Transport::Record(rec), Some(body)) = (self.transport, parsed) {
                rec.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(node.into(), json!({ "status": status, "body": body }));
            }
        }
        let mut t = self.tally();
        t.calls += 1;
        t.unmetered += u32::from(!metered);
        match cost {
            Some(c) => t.cost += c,
            None => t.uncosted += 1,
        }
        drop(t);
        result
    }

    /// Respuesta de un especialista; `live` solo se sondea si el transporte no es `Replay`.
    pub async fn specialist(
        &self,
        key: &str,
        live: impl Future<Output = Result<Value, Status>>,
    ) -> Result<Value, Status> {
        if !matches!(self.transport, Transport::Replay(_)) {
            self.tally().specialist_calls += 1;
        }
        match self.transport {
            Transport::Replay(rec) => {
                rec.get(key).cloned().ok_or_else(|| Status::error(ErrorCode::ProviderFailed, self.miss(key)))
            }
            Transport::Live => live.await,
            Transport::Record(rec) => {
                let value = live.await?;
                rec.lock().unwrap_or_else(PoisonError::into_inner).insert(key.into(), value.clone());
                Ok(value)
            }
        }
    }
}
