//! The hook's shadow log: reading and classifying the lines of `shadow.jsonl`.
//! Note that everything here is pure, so that the report and the TUI share it.

use serde_json::Value;
use std::collections::BTreeMap;

/// Threshold above which a risk is considered dominant.
pub const RISK_THRESHOLD: f64 = 0.55;
pub const HIGH_THRESHOLD: f64 = 0.85;
/// Names of the probabilities; `authorized` is not a risk.
pub const NAMES: [&str; 8] = [
    "destructive",
    "exfiltration",
    "privilege",
    "secret_exposure",
    "off_task",
    "violates_constraint",
    "unresolved_choice",
    "authorized",
];

/// A log line, reduced to what the report needs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entry {
    pub ts: String,
    pub session_id: Option<String>,
    pub tool_name: String,
    pub status: String,
    pub verdict: Option<String>,
    pub source: Option<String>,
    pub reason: Option<String>,
    pub probabilities: BTreeMap<String, f64>,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
    pub tool_input: Option<String>,
}

impl Entry {
    /// Highest probability among the risks (everything except `authorized`).
    pub fn max_risk(&self) -> Option<(&str, f64)> {
        self.probabilities
            .iter()
            .filter(|(k, _)| k.as_str() != "authorized")
            .map(|(k, v)| (k.as_str(), *v))
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }

    pub fn is(&self, verdict: &str) -> bool {
        self.verdict.as_deref() == Some(verdict)
    }
}

/// Parses the log text; returns the entries and the number of ignored lines.
pub fn parse(text: &str) -> (Vec<Entry>, usize) {
    let mut entries = Vec::new();
    let mut ignored = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match parse_line(line) {
            Some(e) => entries.push(e),
            None => ignored += 1,
        }
    }
    (entries, ignored)
}

/// Parses a single line; `None` if it is not JSON or lacks `ts` or `status`.
pub fn parse_line(line: &str) -> Option<Entry> {
    serde_json::from_str::<Value>(line).ok().and_then(|v| entry_of(&v))
}

fn entry_of(v: &Value) -> Option<Entry> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let probabilities = v
        .get("probabilities")
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, p)| p.as_f64().map(|p| (k.clone(), p))).collect())
        .unwrap_or_default();
    let tool_input = v.get("tool_input").filter(|t| !t.is_null()).map(Value::to_string);
    Some(Entry {
        ts: text("ts")?,
        session_id: text("session_id"),
        tool_name: text("tool_name").unwrap_or_else(|| "(sin herramienta)".into()),
        status: text("status")?,
        verdict: text("verdict"),
        source: text("source"),
        reason: text("reason"),
        probabilities,
        latency_ms: v.get("latency_ms").and_then(Value::as_u64),
        error: text("error"),
        tool_input,
    })
}

/// Classifies the reason for an `ask` or `deny`.
pub fn category(e: &Entry) -> String {
    if e.reason.as_deref().is_some_and(|r| r.contains("input too large")) {
        return "contexto truncado".into();
    }
    if e.source.as_deref() == Some("static-rule") {
        return "regla fija".into();
    }
    match e.max_risk() {
        Some((name, p)) if p >= RISK_THRESHOLD => format!("riesgo: {name}"),
        _ => "otro".into(),
    }
}

/// A suspicious `ask`: the model doubts even though every risk stays under the threshold.
pub fn suspicious(e: &Entry) -> bool {
    e.is("ask") && e.source.as_deref() != Some("static-rule") && e.max_risk().is_some_and(|(_, p)| p < RISK_THRESHOLD)
}

/// Percentage of `n` over `total`; zero if there is no total.
pub fn pct(n: usize, total: usize) -> f64 {
    if total == 0 { 0.0 } else { n as f64 * 100.0 / total as f64 }
}

/// Sorted latencies of the calls that got a response (ok or abstain).
pub fn sorted_latencies<'a>(entries: impl Iterator<Item = &'a Entry>) -> Vec<u64> {
    let mut v: Vec<u64> = entries.filter(|e| e.status != "error").filter_map(|e| e.latency_ms).collect();
    v.sort_unstable();
    v
}
