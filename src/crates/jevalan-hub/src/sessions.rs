//! The session log, read side: finds the daily files, parses the JSONL, tolerates broken lines and groups by
//! client. The daemon writes it (`jevalan_daemon::session`); the CLI and the TUI read it through here.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const PREFIX: &str = "log-";
pub const SUFFIX: &str = ".jsonl";

/// The log files of `dir`, oldest first.
pub fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(SUFFIX))
        })
        .collect();
    found.sort();
    found
}

/// Every well-formed line of the log files in `dir`, oldest first. A line that does not parse is skipped.
pub fn read_dir(dir: &Path) -> Vec<Value> {
    let mut lines: Vec<Value> = files(dir)
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .flat_map(|text| text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect::<Vec<_>>())
        .filter(|v| v["type"].is_string() && v["trace_id"].is_string() && v["ts"].is_string())
        .collect();
    // Stable: lines written within the same second keep the order they were appended in.
    lines.sort_by(|a, b| a["ts"].as_str().cmp(&b["ts"].as_str()));
    lines
}

/// One request and everything that was recorded about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub trace_id: String,
    pub lines: Vec<Value>,
}

impl Trace {
    pub fn first(&self, ty: &str) -> Option<&Value> {
        self.lines.iter().find(|l| l["type"] == ty)
    }

    pub fn all<'a>(&'a self, ty: &'a str) -> impl Iterator<Item = &'a Value> {
        self.lines.iter().filter(move |l| l["type"] == ty)
    }

    pub fn start(&self) -> &str {
        self.lines.first().and_then(|l| l["ts"].as_str()).unwrap_or("")
    }

    pub fn end(&self) -> &str {
        self.lines.last().and_then(|l| l["ts"].as_str()).unwrap_or("")
    }
}

/// The traces of one client, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub client: String,
    pub traces: Vec<Trace>,
}

/// Groups lines by trace and traces by client. A trace whose lines name no client falls under `unknown`.
pub fn group(lines: Vec<Value>) -> Vec<Session> {
    let mut by_trace: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut order = Vec::new();
    for line in lines {
        let id = line["trace_id"].as_str().unwrap_or_default().to_string();
        if !by_trace.contains_key(&id) {
            order.push(id.clone());
        }
        by_trace.entry(id).or_default().push(line);
    }
    let mut sessions: Vec<Session> = Vec::new();
    for id in order {
        let lines = by_trace.remove(&id).unwrap_or_default();
        let client = lines.iter().find_map(|l| l["client"].as_str()).unwrap_or("unknown").to_string();
        let trace = Trace { trace_id: id, lines };
        match sessions.iter_mut().find(|s| s.client == client) {
            Some(session) => session.traces.push(trace),
            None => sessions.push(Session { client, traces: vec![trace] }),
        }
    }
    sessions
}

/// What the capability decided, in one word: the verdict it carries, or its status when it carries none.
pub fn verdict(decision: &Value) -> String {
    if decision["status"] != "ok" {
        return decision["status"].as_str().unwrap_or("?").to_string();
    }
    let word = ["decision", "verdict", "outcome"].iter().find_map(|k| decision["decision"][k].as_str());
    // `agent.stop` and its kin decide with a boolean `stop`.
    let stop = decision["decision"]["stop"].as_bool().map(|s| if s { "stop" } else { "continue" });
    word.or(stop).unwrap_or("ok").to_string()
}

/// How many traces of the session ended in each verdict.
pub fn verdict_counts(session: &Session) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for trace in &session.traces {
        let word = trace.first("DECISION").map_or_else(|| "sin decisión".to_string(), verdict);
        *counts.entry(word).or_insert(0) += 1;
    }
    counts
}

/// A trace as a timeline of lines: the input, the path through the flow, the backends touched and the decision.
/// The input is the tool's own, in clear: screens that may end up in a screenshot pass `with_input: false`.
pub fn timeline(trace: &Trace, with_input: bool) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(input) = trace.first("INPUT") {
        let capability = input["capability"].as_str().unwrap_or("?");
        out.push(if with_input {
            format!("entrada ({capability}): {}", input["input"])
        } else {
            format!("entrada ({capability}): oculta; `jevalan sessions show` la muestra")
        });
    }
    let events = |ty: &'static str| trace.all("EVENT").filter(move |l| l["payload"]["type"] == ty);
    for end in events("NODE_END") {
        let p = &end["payload"]["payload"];
        out.push(format!(
            "  nodo {} → puerto {}",
            p["node"].as_str().unwrap_or("?"),
            p["port"].as_str().unwrap_or("?")
        ));
    }
    let latencies: Vec<u64> =
        events("BACKEND_RESPONSE").filter_map(|l| l["payload"]["payload"]["duration_ms"].as_u64()).collect();
    for (i, call) in trace.all("BACKEND_CALL").enumerate() {
        let status = call["response"]["status"].as_u64().map_or_else(
            || format!("fallo: {}", call["response"]["error"].as_str().unwrap_or("?")),
            |s| format!("HTTP {s}"),
        );
        let ms = latencies.get(i).map_or(String::new(), |ms| format!(" ({ms} ms)"));
        out.push(format!(
            "  backend {} nodo {}: {status}{ms}",
            call["backend"].as_str().unwrap_or("?"),
            call["node"].as_str().unwrap_or("?")
        ));
    }
    out.push(match trace.first("DECISION") {
        Some(d) => {
            let detail = d["decision"].as_object().map_or(String::new(), |_| format!(" {}", d["decision"]));
            let code = d["code"].as_str().map_or(String::new(), |c| format!(" ({c})"));
            format!("decisión: {}{code}{detail}", verdict(d))
        }
        None => "decisión: ninguna registrada".to_string(),
    });
    out
}
