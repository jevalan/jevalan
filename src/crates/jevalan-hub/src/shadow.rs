//! Registro sombra del hook: lectura y clasificación de las líneas de `shadow.jsonl`.
//! Cabe señalar que todo cuanto aquí figura es puro, de suerte que el informe y la TUI lo comparten.

use serde_json::Value;
use std::collections::BTreeMap;

/// Umbral a partir del cual un riesgo se considera dominante.
pub const RISK_THRESHOLD: f64 = 0.55;
pub const HIGH_THRESHOLD: f64 = 0.85;
/// Nombres de las probabilidades; `authorized` no es un riesgo.
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

/// Una línea del registro, reducida a lo que el informe necesita.
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
    /// Mayor probabilidad entre los riesgos (todo salvo `authorized`).
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

/// Interpreta el texto del registro; devuelve las entradas y el número de líneas ignoradas.
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

/// Interpreta una sola línea; `None` si no es JSON o le falta `ts` o `status`.
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

/// Clasifica el motivo de un `ask` o `deny`.
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

/// Un `ask` sospechoso: el modelo duda aunque todos los riesgos queden por debajo del umbral.
pub fn suspicious(e: &Entry) -> bool {
    e.is("ask") && e.source.as_deref() != Some("static-rule") && e.max_risk().is_some_and(|(_, p)| p < RISK_THRESHOLD)
}

/// Porcentaje de `n` sobre `total`; cero si no hay total.
pub fn pct(n: usize, total: usize) -> f64 {
    if total == 0 { 0.0 } else { n as f64 * 100.0 / total as f64 }
}

/// Latencias ordenadas de las llamadas que obtuvieron respuesta (ok o abstain).
pub fn sorted_latencies<'a>(entries: impl Iterator<Item = &'a Entry>) -> Vec<u64> {
    let mut v: Vec<u64> = entries.filter(|e| e.status != "error").filter_map(|e| e.latency_ms).collect();
    v.sort_unstable();
    v
}
