//! State derived from the events: complete traces and metrics. Pure logic.

use jevalan_events::{Event, EventKind};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, Instant};

pub const MAX_TRACES: usize = 500;
pub const MAX_DECISIONS: usize = 200;
pub const MAX_CLIENTS: usize = 200;
/// Hub responses remembered per capability, and distinct capabilities kept up to date.
pub const MAX_HUB_RESULTS: usize = 20;
pub const MAX_HUB_CAPABILITIES: usize = 100;
pub const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Seconds of latency history per backend: one bucket per second.
pub const LATENCY_BUCKETS: usize = 60;

/// A backend's response: arrival instant, duration and whether it went well.
#[derive(Debug, Clone, Copy)]
struct Sample {
    at: Instant,
    ms: u64,
    ok: bool,
}

#[derive(Debug, Clone)]
pub struct TraceRecord {
    pub trace_id: String,
    pub client: Option<String>,
    /// Events sorted by `seq`, with the arrival instant.
    pub events: Vec<(Instant, Event)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub trace_id: String,
    pub ts: String,
    pub at: Instant,
    pub outcome: String,
    pub client: String,
    pub flow: String,
    /// Confidence of the trace's last `NODE_END`; 0 if there was none.
    pub confidence: f64,
    /// Hub responses only: `outcome` is the `status`, `flow` the capability, and this its duration.
    pub hub_ms: Option<u64>,
}

/// A hub response, as a capability's card lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubResult {
    pub ts: String,
    pub status: String,
    pub code: Option<String>,
    pub ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackendStats {
    pub calls: u64,
    pub failures: u64,
    pub last_ms: Option<i64>,
    total_ms: i64,
    responses: u64,
}

impl BackendStats {
    pub fn avg_ms(&self) -> Option<f64> {
        (self.responses > 0).then(|| self.total_ms as f64 / self.responses as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientStats {
    pub requests: u64,
    pub last_seen: Instant,
}

#[derive(Debug, Default)]
pub struct Store {
    traces: HashMap<String, TraceRecord>,
    order: VecDeque<String>,
    decisions: VecDeque<Decision>,
    requests: VecDeque<Instant>,
    outcomes: BTreeMap<String, u64>,
    /// Flujo → outcome → decisiones.
    flows: BTreeMap<String, BTreeMap<String, u64>>,
    backends: BTreeMap<String, BackendStats>,
    latencies: BTreeMap<String, VecDeque<Sample>>,
    clients: BTreeMap<String, ClientStats>,
    /// Capability → latest responses, from newest to oldest.
    hub_results: BTreeMap<String, VecDeque<HubResult>>,
    /// Capabilities announced by `CAPABILITY_LOADED`: only they are counted.
    known_caps: BTreeSet<String>,
    errors: u64,
    lagged: u64,
    dropped: u64,
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores the event in its trace. Returns `false` if (`trace_id`, `seq`) had already been seen.
    pub fn ingest(&mut self, e: Event, now: Instant) -> bool {
        if !self.traces.contains_key(&e.trace_id) {
            if self.order.len() == MAX_TRACES
                && let Some(old) = self.order.pop_front()
            {
                self.traces.remove(&old);
            }
            self.order.push_back(e.trace_id.clone());
            self.traces.insert(
                e.trace_id.clone(),
                TraceRecord { trace_id: e.trace_id.clone(), client: None, events: Vec::new() },
            );
        }
        let trace = self.traces.get_mut(&e.trace_id).expect("traza recién insertada");
        let pos = match trace.events.binary_search_by_key(&e.seq, |(_, ev)| ev.seq) {
            Ok(_) => return false,
            Err(pos) => pos,
        };

        match &e.kind {
            EventKind::RequestReceived { client, .. } | EventKind::HubRequest { client, .. } => {
                trace.client = Some(client.clone());
                self.requests.push_back(now);
                let stats = self.clients.entry(client.clone()).or_insert(ClientStats { requests: 0, last_seen: now });
                stats.requests += 1;
                stats.last_seen = now;
                if self.clients.len() > MAX_CLIENTS {
                    // ponytail: linear search for the oldest, O(n) per new client; plenty for 200
                    let oldest = self.clients.iter().min_by_key(|(_, s)| s.last_seen).map(|(n, _)| n.clone());
                    if let Some(oldest) = oldest {
                        self.clients.remove(&oldest);
                    }
                }
            }
            EventKind::BackendRequest { backend, .. } => {
                self.backends.entry(backend.clone()).or_default().calls += 1;
            }
            EventKind::BackendResponse { backend, ok, duration_ms, .. } => {
                let ms = *duration_ms as i64;
                let stats = self.backends.entry(backend.clone()).or_default();
                if !ok {
                    stats.failures += 1;
                }
                stats.last_ms = Some(ms);
                stats.total_ms += ms;
                stats.responses += 1;
                let samples = self.latencies.entry(backend.clone()).or_default();
                samples.push_back(Sample { at: now, ms: *duration_ms, ok: *ok });
                while samples.front().is_some_and(|s| now.duration_since(s.at) >= RATE_WINDOW) {
                    samples.pop_front();
                }
            }
            // A hub trace is counted by its response, not by its flow's outcome.
            EventKind::DecisionMade { .. }
                if trace.events.iter().any(|(_, ev)| matches!(ev.kind, EventKind::HubRequest { .. })) => {}
            EventKind::HubResponse { capability, status, code, elapsed_ms, .. } => {
                if self.known_caps.contains(capability) {
                    *self.outcomes.entry(status.clone()).or_default() += 1;
                    *self.flows.entry(capability.clone()).or_default().entry(status.clone()).or_default() += 1;
                    let results = self.hub_results.entry(capability.clone()).or_default();
                    if results.len() == MAX_HUB_RESULTS {
                        results.pop_back();
                    }
                    results.push_front(HubResult {
                        ts: e.ts.clone(),
                        status: status.clone(),
                        code: code.clone(),
                        ms: *elapsed_ms,
                    });
                    if self.decisions.len() == MAX_DECISIONS {
                        self.decisions.pop_back();
                    }
                    self.decisions.push_front(Decision {
                        trace_id: e.trace_id.clone(),
                        ts: e.ts.clone(),
                        at: now,
                        outcome: status.clone(),
                        client: trace.client.clone().unwrap_or_else(|| "?".into()),
                        flow: capability.clone(),
                        confidence: 0.0,
                        hub_ms: Some(*elapsed_ms),
                    });
                }
            }
            EventKind::DecisionMade { flow, outcome, .. } => {
                *self.outcomes.entry(outcome.clone()).or_default() += 1;
                *self.flows.entry(flow.clone()).or_default().entry(outcome.clone()).or_default() += 1;
                let confidence = trace
                    .events
                    .iter()
                    .rev()
                    .find_map(|(_, ev)| match &ev.kind {
                        EventKind::NodeEnd { confidence, .. } => Some(*confidence),
                        _ => None,
                    })
                    .unwrap_or(0.0);
                if self.decisions.len() == MAX_DECISIONS {
                    self.decisions.pop_back();
                }
                self.decisions.push_front(Decision {
                    trace_id: e.trace_id.clone(),
                    ts: e.ts.clone(),
                    at: now,
                    outcome: outcome.clone(),
                    client: trace.client.clone().unwrap_or_else(|| "?".into()),
                    flow: flow.clone(),
                    confidence,
                    hub_ms: None,
                });
            }
            EventKind::Error { .. } => self.errors += 1,
            EventKind::Lagged { dropped } => self.lagged += *dropped,
            EventKind::NodeStart { .. }
            | EventKind::NodeEnd { .. }
            | EventKind::NodeAudit { .. }
            | EventKind::PassthroughDone { .. }
            | EventKind::Backends { .. }
            | EventKind::FlowLoaded { .. }
            | EventKind::FlowInvalid { .. }
            | EventKind::FlowRemoved { .. }
            | EventKind::CapabilityInvalid { .. } => {}
            EventKind::CapabilityLoaded { capability, .. } => {
                if self.known_caps.len() < MAX_HUB_CAPABILITIES {
                    self.known_caps.insert(capability.clone());
                }
            }
            EventKind::CapabilityRemoved { capability } => {
                self.known_caps.remove(capability);
                self.hub_results.remove(capability);
            }
        }

        trace.events.insert(pos, (now, e));
        while self.requests.front().is_some_and(|t| now.duration_since(*t) > RATE_WINDOW) {
            self.requests.pop_front();
        }
        true
    }

    /// Counts an event that could not be deserialised.
    pub fn note_dropped(&mut self) {
        self.dropped += 1;
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn lagged(&self) -> u64 {
        self.lagged
    }

    pub fn errors(&self) -> u64 {
        self.errors
    }

    pub fn trace(&self, trace_id: &str) -> Option<&TraceRecord> {
        self.traces.get(trace_id)
    }

    pub fn trace_count(&self) -> usize {
        self.traces.len()
    }

    /// Recent decisions, from newest to oldest.
    pub fn decisions(&self) -> impl Iterator<Item = &Decision> {
        self.decisions.iter()
    }

    /// Decisions by outcome, across all flows.
    pub fn outcomes(&self) -> &BTreeMap<String, u64> {
        &self.outcomes
    }

    pub fn outcome_total(&self) -> u64 {
        self.outcomes.values().sum()
    }

    /// Whole-number percentage (0-100) of an outcome; 0 if there is no data.
    pub fn outcome_pct(&self, outcome: &str) -> u64 {
        let total = self.outcome_total();
        if total == 0 { 0 } else { (self.outcomes.get(outcome).copied().unwrap_or(0) * 100 + total / 2) / total }
    }

    /// Decisions by outcome for one flow.
    pub fn flow(&self, name: &str) -> Option<&BTreeMap<String, u64>> {
        self.flows.get(name)
    }

    /// Latest hub responses for a capability, from newest to oldest.
    pub fn hub_results(&self, capability: &str) -> impl Iterator<Item = &HubResult> {
        self.hub_results.get(capability).into_iter().flatten()
    }

    pub fn backend(&self, name: &str) -> Option<&BackendStats> {
        self.backends.get(name)
    }

    pub fn client(&self, name: &str) -> Option<&ClientStats> {
        self.clients.get(name)
    }

    /// Latency of a backend's good responses over the last 60 s, from oldest to most recent.
    /// One bucket per second with the mean of its samples; `None` if there was no good response in that second.
    pub fn latency_series(&self, backend: &str, now: Instant) -> Vec<Option<u64>> {
        let mut sums = vec![(0u64, 0u64); LATENCY_BUCKETS];
        for s in self.window(backend, now).filter(|s| s.ok) {
            let slot = &mut sums[LATENCY_BUCKETS - 1 - now.duration_since(s.at).as_secs() as usize];
            *slot = (slot.0 + s.ms, slot.1 + 1);
        }
        sums.into_iter().map(|(sum, n)| (n > 0).then(|| sum / n)).collect()
    }

    /// Buckets (as in `latency_series`) where at least one response failed.
    pub fn latency_failures(&self, backend: &str, now: Instant) -> Vec<bool> {
        let mut out = vec![false; LATENCY_BUCKETS];
        for s in self.window(backend, now).filter(|s| !s.ok) {
            out[LATENCY_BUCKETS - 1 - now.duration_since(s.at).as_secs() as usize] = true;
        }
        out
    }

    /// Latency of the backend's last response.
    pub fn last_latency(&self, backend: &str) -> Option<u64> {
        self.latencies.get(backend)?.back().map(|s| s.ms)
    }

    fn window(&self, backend: &str, now: Instant) -> impl Iterator<Item = &Sample> {
        let in_window = move |s: &&Sample| now.checked_duration_since(s.at).is_some_and(|d| d < RATE_WINDOW);
        self.latencies.get(backend).into_iter().flatten().filter(in_window)
    }

    /// Requests per second, mean of the last 5 s.
    pub fn rate(&self, now: Instant) -> f64 {
        let n = self.requests.iter().filter(|t| now.duration_since(**t) < Duration::from_secs(5)).count();
        n as f64 / 5.0
    }

    /// Requests per second over the last 60 s, from oldest to most recent.
    pub fn sparkline(&self, now: Instant) -> Vec<u64> {
        let mut buckets = vec![0u64; 60];
        for t in &self.requests {
            let ago = now.duration_since(*t).as_secs() as usize;
            if ago < 60 {
                buckets[59 - ago] += 1;
            }
        }
        buckets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevalan_events::{Endpoint, SCHEMA_VERSION};
    use serde_json::json;

    fn ev(trace: &str, seq: u64, kind: EventKind) -> Event {
        Event {
            v: SCHEMA_VERSION,
            id: format!("{trace}-{seq}"),
            ts: "2026-10-07T12:00:00Z".into(),
            trace_id: trace.into(),
            seq,
            source: "test".into(),
            kind,
        }
    }

    fn req(trace: &str, client: &str) -> Event {
        let kind = EventKind::RequestReceived {
            client: client.into(),
            endpoint: Endpoint::Decide,
            flow: Some("agent-guard".into()),
            backend: None,
        };
        ev(trace, 1, kind)
    }

    fn node_end(trace: &str, seq: u64, confidence: f64) -> Event {
        let kind = EventKind::NodeEnd {
            flow: "agent-guard".into(),
            node: "n".into(),
            answer: json!(0.5),
            confidence,
            port: "true".into(),
        };
        ev(trace, seq, kind)
    }

    fn decision(trace: &str, seq: u64, outcome: &str) -> Event {
        let kind = EventKind::DecisionMade {
            flow: "agent-guard".into(),
            outcome: outcome.into(),
            path_len: 1,
            duration_ms: 5,
        };
        ev(trace, seq, kind)
    }

    fn backend_req(trace: &str, seq: u64) -> Event {
        ev(trace, seq, EventKind::BackendRequest { backend: "ollama".into(), model: "m".into(), questions: 1 })
    }

    fn backend_resp(trace: &str, seq: u64, ok: bool, duration_ms: u64) -> Event {
        let kind = EventKind::BackendResponse {
            backend: "ollama".into(),
            ok,
            status: if ok { 200 } else { 0 },
            duration_ms,
            input_tokens: None,
        };
        ev(trace, seq, kind)
    }

    #[test]
    fn ingest_orders_by_seq_and_ignores_duplicates() {
        let mut s = Store::new();
        let now = Instant::now();
        assert!(s.ingest(node_end("t", 3, 0.9), now));
        assert!(s.ingest(req("t", "crush"), now));
        assert!(!s.ingest(req("t", "crush"), now));
        let seqs: Vec<u64> = s.trace("t").unwrap().events.iter().map(|(_, e)| e.seq).collect();
        assert_eq!(seqs, vec![1, 3]);
        assert_eq!(s.trace("t").unwrap().client.as_deref(), Some("crush"));
        assert_eq!(s.client("crush").unwrap().requests, 1);
    }

    #[test]
    fn decision_takes_the_last_node_confidence() {
        let mut s = Store::new();
        let now = Instant::now();
        s.ingest(req("a", "crush"), now);
        s.ingest(node_end("a", 2, 0.4), now);
        s.ingest(node_end("a", 3, 0.93), now);
        s.ingest(decision("a", 4, "allow"), now);
        s.ingest(req("b", "vscode"), now);
        s.ingest(decision("b", 2, "deny"), now);
        let d: Vec<&Decision> = s.decisions().collect();
        assert_eq!((d[0].trace_id.as_str(), d[0].outcome.as_str(), d[0].client.as_str()), ("b", "deny", "vscode"));
        assert_eq!(d[0].confidence, 0.0, "sin NODE_END la confianza es 0");
        assert_eq!((d[1].flow.as_str(), d[1].confidence), ("agent-guard", 0.93));
        assert_eq!(s.flow("agent-guard").unwrap()["allow"], 1);
    }

    #[test]
    fn passthrough_is_not_a_decision() {
        let mut s = Store::new();
        let now = Instant::now();
        let done =
            EventKind::PassthroughDone { backend: "ollama".into(), model: "m".into(), status: 200, duration_ms: 7 };
        s.ingest(ev("p", 4, done), now);
        assert_eq!(s.decisions().count(), 0);
        assert_eq!(s.outcome_total(), 0);
    }

    #[test]
    fn outcome_percentages() {
        let mut s = Store::new();
        let now = Instant::now();
        assert_eq!(s.outcome_pct("allow"), 0);
        for (i, o) in ["allow", "allow", "allow", "deny"].into_iter().enumerate() {
            s.ingest(decision(&format!("t{i}"), 1, o), now);
        }
        assert_eq!((s.outcome_pct("allow"), s.outcome_pct("deny"), s.outcome_pct("abstain")), (75, 25, 0));
        assert_eq!(s.outcome_total(), 4);
    }

    #[test]
    fn decision_without_request_uses_a_placeholder_client() {
        let mut s = Store::new();
        s.ingest(decision("x", 1, "abstain"), Instant::now());
        assert_eq!(s.decisions().next().unwrap().client, "?");
    }

    #[test]
    fn backend_stats_track_calls_failures_and_latency() {
        let mut s = Store::new();
        let now = Instant::now();
        s.ingest(backend_req("t", 1), now);
        s.ingest(backend_resp("t", 2, true, 100), now);
        s.ingest(backend_req("t", 3), now);
        s.ingest(backend_resp("t", 4, false, 800), now);
        let b = s.backend("ollama").unwrap();
        assert_eq!((b.calls, b.failures, b.last_ms), (2, 1, Some(800)));
        assert_eq!(b.avg_ms(), Some(450.0));
        assert!(s.backend("typesafe").is_none());
    }

    #[test]
    fn errors_lagged_and_dropped_are_counted() {
        let mut s = Store::new();
        let now = Instant::now();
        s.ingest(ev("t", 1, EventKind::Error { stage: "h".into(), message: "m".into() }), now);
        s.ingest(ev("t", 2, EventKind::Lagged { dropped: 3 }), now);
        s.note_dropped();
        assert_eq!((s.errors(), s.lagged(), s.dropped()), (1, 3, 1));
    }

    #[test]
    fn rate_and_sparkline_use_a_sliding_window() {
        let mut s = Store::new();
        let t0 = Instant::now();
        s.ingest(req("old", "crush"), t0);
        let later = t0 + Duration::from_secs(61);
        for i in 0..10 {
            s.ingest(req(&format!("n{i}"), "crush"), later);
        }
        assert_eq!(s.rate(later), 2.0);
        let spark = s.sparkline(later);
        assert_eq!(spark.len(), 60);
        assert_eq!(spark[59], 10);
        assert_eq!(spark.iter().sum::<u64>(), 10, "la petición de hace 61 s queda fuera");
    }

    #[test]
    fn oldest_trace_is_evicted_beyond_the_cap() {
        let mut s = Store::new();
        let now = Instant::now();
        for i in 0..=MAX_TRACES {
            s.ingest(req(&format!("t{i}"), "crush"), now);
        }
        assert_eq!(s.trace_count(), MAX_TRACES);
        assert!(s.trace("t0").is_none());
        assert!(s.trace(&format!("t{MAX_TRACES}")).is_some());
    }

    #[test]
    fn clients_are_bounded() {
        let mut s = Store::new();
        let t0 = Instant::now();
        for i in 0..MAX_CLIENTS + 5 {
            s.ingest(req(&format!("t{i}"), &format!("c{i}")), t0 + Duration::from_millis(i as u64));
        }
        assert_eq!(s.clients.len(), MAX_CLIENTS);
        assert!(s.client("c0").is_none(), "el cliente más antiguo sale");
        assert!(s.client(&format!("c{}", MAX_CLIENTS + 4)).is_some());
    }

    #[test]
    fn decisions_are_capped() {
        let mut s = Store::new();
        let now = Instant::now();
        for i in 0..MAX_DECISIONS + 5 {
            s.ingest(decision(&format!("t{i}"), 1, "allow"), now);
        }
        assert_eq!(s.decisions().count(), MAX_DECISIONS);
        assert_eq!(s.decisions().next().unwrap().trace_id, format!("t{}", MAX_DECISIONS + 4));
    }

    #[test]
    fn latency_series_buckets_by_second() {
        let mut s = Store::new();
        let t0 = Instant::now();
        s.ingest(backend_resp("t", 1, true, 100), t0);
        s.ingest(backend_resp("t", 2, true, 300), t0 + Duration::from_millis(400));
        s.ingest(backend_resp("t", 3, true, 50), t0 + Duration::from_secs(2));
        let now = t0 + Duration::from_millis(2500);
        let series = s.latency_series("ollama", now);
        assert_eq!(series.len(), LATENCY_BUCKETS);
        assert_eq!(series[59], Some(50));
        assert_eq!(series[58], None);
        assert_eq!(series[57], Some(200), "media de 100 y 300");
        assert_eq!(s.last_latency("ollama"), Some(50));
        assert!(s.latency_series("nadie", now).iter().all(Option::is_none));
    }

    #[test]
    fn old_samples_fall_out_of_the_window() {
        let mut s = Store::new();
        let t0 = Instant::now();
        s.ingest(backend_resp("t", 1, true, 100), t0);
        s.ingest(backend_resp("t", 2, true, 70), t0 + Duration::from_secs(30));
        let later = t0 + Duration::from_secs(61);
        let series = s.latency_series("ollama", later);
        assert_eq!(series.iter().flatten().count(), 1);
        assert_eq!(series[28], Some(70));
        s.ingest(backend_resp("t", 3, true, 10), later);
        assert_eq!(s.latencies["ollama"].len(), 2, "la muestra vieja se purga al ingerir");
    }

    fn hub_request(trace: &str, client: &str) -> Event {
        let kind = EventKind::HubRequest {
            capability: "tool.risk".into(),
            capability_version: "1".into(),
            provider: "toolgate".into(),
            client: client.into(),
        };
        ev(trace, 1, kind)
    }

    fn cap_loaded(name: &str) -> Event {
        let kind = EventKind::CapabilityLoaded {
            capability: name.into(),
            version: "1".into(),
            provider: "toolgate".into(),
            provider_type: "exec".into(),
            flow: None,
            description: String::new(),
            input_fields: vec![],
            decision_fields: vec![],
        };
        ev(&format!("capability:{name}:1"), 1, kind)
    }

    fn hub_response(trace: &str, seq: u64, status: &str, code: Option<&str>) -> Event {
        let kind = EventKind::HubResponse {
            capability: "tool.risk".into(),
            provider: "toolgate".into(),
            status: status.into(),
            code: code.map(Into::into),
            elapsed_ms: 120,
        };
        ev(trace, seq, kind)
    }

    #[test]
    fn hub_responses_feed_the_decisions_and_the_outcome_counts() {
        let mut s = Store::new();
        let now = Instant::now();
        s.ingest(cap_loaded("tool.risk"), now);
        s.ingest(hub_request("a", "vscode"), now);
        s.ingest(hub_response("a", 2, "ok", None), now);
        s.ingest(hub_request("b", "cron"), now);
        s.ingest(hub_response("b", 2, "error", Some("deadline_exceeded")), now);
        let d: Vec<&Decision> = s.decisions().collect();
        assert_eq!((d[0].client.as_str(), d[0].flow.as_str(), d[0].outcome.as_str()), ("cron", "tool.risk", "error"));
        assert_eq!((d[1].client.as_str(), d[1].hub_ms), ("vscode", Some(120)));
        assert_eq!((s.outcomes()["ok"], s.outcomes()["error"]), (1, 1));
        assert_eq!(s.flow("tool.risk").unwrap()["ok"], 1);
        assert_eq!(s.client("vscode").unwrap().requests, 1, "la petición al hub cuenta como cliente");
        let results: Vec<(&str, Option<&str>)> =
            s.hub_results("tool.risk").map(|r| (r.status.as_str(), r.code.as_deref())).collect();
        assert_eq!(results, [("error", Some("deadline_exceeded")), ("ok", None)]);
    }

    #[test]
    fn a_flow_decision_inside_a_hub_trace_is_not_counted_twice() {
        let mut s = Store::new();
        let now = Instant::now();
        s.ingest(cap_loaded("tool.risk"), now);
        s.ingest(hub_request("a", "vscode"), now);
        s.ingest(decision("a", 2, "ok"), now);
        assert_eq!(s.decisions().count(), 0);
        assert_eq!(s.outcome_total(), 0);
        s.ingest(hub_response("a", 3, "ok", None), now);
        assert_eq!(s.outcome_total(), 1);
    }

    #[test]
    fn hub_results_are_capped_per_capability_and_unknown_names_are_ignored() {
        let mut s = Store::new();
        let now = Instant::now();
        s.ingest(cap_loaded("tool.risk"), now);
        for i in 0..MAX_HUB_RESULTS as u64 + 5 {
            s.ingest(hub_response(&format!("t{i}"), 1, "ok", None), now);
        }
        assert_eq!(s.hub_results("tool.risk").count(), MAX_HUB_RESULTS);
        for i in 0..MAX_HUB_CAPABILITIES + 20 {
            let kind = EventKind::HubResponse {
                capability: format!("c{i}"),
                provider: "-".into(),
                status: "error".into(),
                code: Some("unknown_capability".into()),
                elapsed_ms: 1,
            };
            s.ingest(ev(&format!("u{i}"), 1, kind), now);
        }
        assert_eq!(s.hub_results.len(), 1, "los nombres que elige el cliente no cuentan ni ocupan sitio");
        s.ingest(cap_loaded("agent.wakeup"), now);
        assert!(s.known_caps.contains("agent.wakeup"), "los desconocidos no consumen el cupo");
    }

    #[test]
    fn failures_are_marked() {
        let mut s = Store::new();
        let t0 = Instant::now();
        s.ingest(backend_resp("t", 1, true, 100), t0);
        s.ingest(backend_resp("t", 2, false, 800), t0 + Duration::from_secs(1));
        let now = t0 + Duration::from_secs(1);
        let (series, failed) = (s.latency_series("ollama", now), s.latency_failures("ollama", now));
        assert_eq!((series[59], failed[59]), (None, true));
        assert_eq!((series[58], failed[58]), (Some(100), false));
    }
}
