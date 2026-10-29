//! Where calls to the outside go out: live, replayed from a recording, or recorded on the fly.

use crate::AppState;
use crate::proxy::call_backend;
use axum::body::Bytes;
use jevalan_hub::classify::{ErrorCode, Status};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

/// Key: node name (the backend's `{status, body}`) or `provider.id` (the specialist's JSON).
pub type Recordings = BTreeMap<String, Value>;

#[derive(Clone)]
pub enum Transport {
    Live,
    /// It never launches processes or opens connections.
    Replay(Arc<Recordings>),
    /// Calls live and stores only the response: never headers or the request.
    Record(Arc<Mutex<Recordings>>),
}

/// System One response for `node` as a backend would record it; useful for repeating by hand.
pub fn system_one_reply(node: &str, node_type: &str, answer: &Value, confidence: f64) -> Value {
    json!({ "status": 200, "body": { "answers": { node: { "type": node_type, node_type: answer, "confidence": confidence } } } })
}

/// Response joining the recordings of each question of a `batch` node: one request, many answers.
/// `questions` are the full keys, `"<node>.<question>"`. The HTTP status is set by the first one;
/// no recording, no response.
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
    /// Calls to backends and, of those, the ones that did not return both `usage` counters.
    calls: u32,
    unmetered: u32,
    specialist_calls: u32,
    /// Sum of `usage.cost` and the calls that did not carry it.
    cost: f64,
    uncosted: u32,
}

/// Transport plus what is measured during a run.
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

    /// Only a live transport falls back to the next member of a route: a recording is deterministic.
    pub fn is_live(&self) -> bool {
        matches!(self.transport, Transport::Live)
    }

    pub fn is_replay(&self) -> bool {
        matches!(self.transport, Transport::Replay(_))
    }

    pub fn specialist_calls(&self) -> u32 {
        self.tally().specialist_calls
    }

    /// Probability of the answer given by the last non-`rule` node.
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

    /// Cost in USD declared by the backends; `None` if there were no calls or any of them did not carry it.
    pub fn reported_cost(&self) -> Option<f64> {
        let t = self.tally();
        (t.calls > 0 && t.uncosted == 0).then_some(t.cost)
    }

    fn miss(&self, key: &str) -> String {
        self.tally().missing.get_or_insert_with(|| key.to_string());
        format!("missing_recording:{key}")
    }

    /// Call to a backend through the transport; `node` is the recording key.
    pub async fn backend(
        &self,
        state: &AppState,
        backend: &str,
        node: &str,
        body: Bytes,
    ) -> Result<(u16, Bytes), String> {
        self.backend_keys(state, backend, node, &[], body).await
    }

    /// Like `backend`, but for a `batch` node: the recording is looked up question by question (`questions`
    /// are the full keys, `"<node>.<question>"`) and the responses are merged into one (D3).
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
        // A failed call, or one without `usage`, cannot be priced: the run's cost stops being known.
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
                let mut tape = rec.lock().unwrap_or_else(PoisonError::into_inner);
                if questions.is_empty() {
                    tape.insert(node.into(), json!({ "status": status, "body": body }));
                } else {
                    // A `batch` is replayed question by question (`replay_many`), so it is recorded that way too.
                    // A reply without the question's answer (a failure) is kept whole, for `--force` to inspect.
                    for key in questions {
                        let kept = match body["answers"].get(key) {
                            Some(answer) => json!({ "answers": { key: answer }, "model": body["model"] }),
                            None => body.clone(),
                        };
                        tape.insert(key.clone(), json!({ "status": status, "body": kept }));
                    }
                }
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

    /// Specialist response; `live` is only polled if the transport is not `Replay`.
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
