//! Reading side of the session log: parses the JSONL, tolerates broken lines and groups by client.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Every well-formed line of the log files in `dir`, oldest first. A line that does not parse is skipped.
pub fn read_dir(dir: &Path) -> Vec<Value> {
    let mut lines: Vec<Value> = super::files(dir)
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
