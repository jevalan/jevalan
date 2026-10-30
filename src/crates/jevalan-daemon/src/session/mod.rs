//! Session capture: an append-only JSONL log of what the daemon was asked and what it decided.
//!
//! The input, the decision and the backend calls reach the `Recorder` directly and never the public bus (D2): the
//! live TUI must keep seeing neither. It is opt-in, local-only and best-effort: a write that fails is dropped and
//! counted, never turned into a failed request (D7).

pub use jevalan_hub::sessions as log;
pub use jevalan_hub::sessions::files;

use crate::bus::Bus;
use jevalan_events::v2::{Event, EventKind, rfc3339};
use jevalan_hub::sessions::{PREFIX, SUFFIX};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

/// How many `trace_id → client` pairs are remembered before the map starts over.
const MAX_CLIENTS: usize = 4096;

struct Active {
    /// `YYYYMMDD` of the file held open.
    day: String,
    file: Option<File>,
}

pub struct Recorder {
    dir: PathBuf,
    active: Mutex<Active>,
    clients: Mutex<HashMap<String, String>>,
    failures: AtomicU64,
}

impl Recorder {
    /// Opens the log in `dir` (created if missing) and prunes the files older than `retention_days`.
    pub fn open(dir: PathBuf, retention_days: u32) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let recorder = Self {
            dir,
            active: Mutex::new(Active { day: String::new(), file: None }),
            clients: Mutex::new(HashMap::new()),
            failures: AtomicU64::new(0),
        };
        recorder.prune(retention_days, &day_of(SystemTime::now()));
        recorder
    }

    /// Lines that could not be written since the start.
    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    /// Follows the public bus and stores every event of a trace. The task ends with the bus.
    pub fn follow(self: &Arc<Self>, bus: &Bus) {
        let (recorder, mut rx) = (self.clone(), bus.subscribe());
        tokio::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match rx.recv().await {
                    Ok(event) => recorder.event(&event),
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    /// The tool's input, exactly as the client sent it.
    pub fn input(&self, trace_id: &str, client: &str, capability: &str, input: &Value) {
        self.remember(trace_id, client);
        self.write(json!({
            "trace_id": trace_id, "client": client, "type": "INPUT", "capability": capability, "input": input,
        }));
    }

    /// What the capability answered: its decision, or why there is none.
    pub fn decision(
        &self,
        trace_id: &str,
        capability: &str,
        status: &str,
        decision: Option<&Value>,
        code: Option<&str>,
    ) {
        let mut line = json!({ "trace_id": trace_id, "type": "DECISION", "capability": capability, "status": status });
        if let Some(decision) = decision {
            line["decision"] = decision.clone();
        }
        if let Some(code) = code {
            line["code"] = json!(code);
        }
        self.write(line);
    }

    /// A call to a backend: the request body and what came back, untouched.
    pub fn backend_call(
        &self,
        trace_id: &str,
        backend: &str,
        node: &str,
        request: &[u8],
        response: Result<(u16, &[u8]), &str>,
    ) {
        let body = |bytes: &[u8]| {
            serde_json::from_slice::<Value>(bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(bytes)))
        };
        let response = match response {
            Ok((status, bytes)) => json!({ "status": status, "body": body(bytes) }),
            Err(message) => json!({ "error": message }),
        };
        self.write(json!({
            "trace_id": trace_id, "type": "BACKEND_CALL", "backend": backend, "node": node,
            "request": body(request), "response": response,
        }));
    }

    /// What an `exec`/`http` specialist answered, under the key a replay will ask for (`provider.id`).
    pub fn specialist_call(&self, trace_id: &str, key: &str, response: &Value) {
        self.write(json!({ "trace_id": trace_id, "type": "SPECIALIST_CALL", "key": key, "response": response }));
    }

    /// A specialist that failed, so that the trace can be replayed to the same error.
    pub fn specialist_failure(&self, trace_id: &str, key: &str, code: &str, message: &str) {
        self.write(json!({
            "trace_id": trace_id, "type": "SPECIALIST_CALL", "key": key,
            "failure": { "code": code, "message": message },
        }));
    }

    fn event(&self, event: &Event) {
        match &event.kind {
            EventKind::HubRequest { client, .. } | EventKind::RequestReceived { client, .. } => {
                self.remember(&event.trace_id, client);
            }
            _ if self.client_of(&event.trace_id).is_some() => {}
            // Announcements and the like belong to no trace of any client.
            _ => return,
        }
        if let Ok(payload) = serde_json::to_value(event) {
            self.write(json!({ "trace_id": event.trace_id, "type": "EVENT", "payload": payload }));
        }
    }

    fn remember(&self, trace_id: &str, client: &str) {
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        if clients.len() >= MAX_CLIENTS {
            clients.clear();
        }
        clients.insert(trace_id.into(), client.into());
    }

    fn client_of(&self, trace_id: &str) -> Option<String> {
        self.clients.lock().unwrap_or_else(PoisonError::into_inner).get(trace_id).cloned()
    }

    /// Stamps the line with the time and, if it has none, the client of its trace; then appends it.
    fn write(&self, mut line: Value) {
        let now = SystemTime::now();
        line["ts"] = json!(rfc3339(now));
        if line.get("client").is_none()
            && let Some(client) = line["trace_id"].as_str().and_then(|t| self.client_of(t))
        {
            line["client"] = json!(client);
        }
        let day = day_of(now);
        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        if active.day != day || active.file.is_none() {
            let path = self.dir.join(format!("{PREFIX}{day}{SUFFIX}"));
            active.file = OpenOptions::new().create(true).append(true).open(path).ok();
            active.day = day;
        }
        let written = active.file.as_mut().is_some_and(|f| writeln!(f, "{line}").is_ok());
        if !written {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Deletes the log files more than `retention_days` older than `today`. Today's file is never touched.
    pub fn prune(&self, retention_days: u32, today: &str) {
        let Some(today_days) = days_since_epoch(today) else { return };
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(day) = name.strip_prefix(PREFIX).and_then(|n| n.strip_suffix(SUFFIX)) else { continue };
            if day != today && days_since_epoch(day).is_some_and(|d| today_days - d > i64::from(retention_days)) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// `YYYYMMDD` (UTC) of an instant.
fn day_of(t: SystemTime) -> String {
    rfc3339(t).chars().take(10).filter(|c| *c != '-').collect()
}

/// Days from 1970-01-01 to a `YYYYMMDD` date (Howard Hinnant's civil-days algorithm); `None` if it is not one.
fn days_since_epoch(day: &str) -> Option<i64> {
    if day.len() != 8 || !day.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (day[..4].parse().ok()?, day[4..6].parse().ok()?, day[6..].parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The directory the log lives in when the configuration names none: `<data>/sessions`.
pub fn default_dir() -> Option<PathBuf> {
    crate::discovery::data_dir().map(|d| d.join("sessions"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jevalan-session-{name}-{}", crate::bus::new_trace_id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn civil_days_match_known_dates() {
        assert_eq!(days_since_epoch("19700101"), Some(0));
        assert_eq!(days_since_epoch("20000301"), Some(11_017));
        assert_eq!(days_since_epoch("20261029").unwrap() - days_since_epoch("20261015").unwrap(), 14);
        assert_eq!(days_since_epoch("2026-10-29"), None);
        assert_eq!(days_since_epoch("20261329"), None);
    }

    #[test]
    fn it_writes_input_decision_and_backend_call_under_one_trace() {
        let dir = temp("write");
        let recorder = Recorder::open(dir.clone(), 14);
        recorder.input("t1", "crush/crush:243a", "tool.risk", &json!({ "tool_name": "bash" }));
        recorder.backend_call("t1", "kev", "risks", br#"{"model":"m"}"#, Ok((200, br#"{"answers":{}}"#)));
        recorder.decision("t1", "tool.risk", "ok", Some(&json!({ "decision": "ask" })), None);

        let lines = log::read_dir(&dir);
        let kinds: Vec<_> = lines.iter().map(|l| l["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["INPUT", "BACKEND_CALL", "DECISION"]);
        assert!(lines.iter().all(|l| l["client"] == "crush/crush:243a"), "{lines:?}");
        assert_eq!(lines[1]["response"]["status"], 200);
        assert_eq!(lines[2]["decision"]["decision"], "ask");
        assert_eq!(recorder.failures(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_backend_call_keeps_its_message() {
        let dir = temp("failed");
        let recorder = Recorder::open(dir.clone(), 14);
        recorder.backend_call("t1", "kev", "risks", b"{}", Err("connection refused"));
        assert_eq!(log::read_dir(&dir)[0]["response"]["error"], "connection refused");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pruning_respects_the_retention_and_never_the_active_file() {
        let dir = temp("prune");
        std::fs::create_dir_all(&dir).unwrap();
        for day in ["20260101", "20261010", "20261016", "20261028", "20261029"] {
            std::fs::write(dir.join(format!("log-{day}.jsonl")), "").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let recorder = Recorder::open(dir.clone(), u32::MAX);
        recorder.prune(14, "20261029");
        let left: Vec<String> =
            files(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["log-20261016.jsonl", "log-20261028.jsonl", "log-20261029.jsonl"]);
        assert!(dir.join("notes.txt").exists());
        // Even with no retention at all, the file of the day being written stays.
        recorder.prune(0, "20261029");
        assert!(dir.join("log-20261029.jsonl").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn broken_lines_are_skipped_and_sessions_group_by_client() {
        let dir = temp("read");
        std::fs::create_dir_all(&dir).unwrap();
        let good = |trace: &str, client: &str, ts: &str| {
            json!({ "ts": ts, "trace_id": trace, "client": client, "type": "INPUT" }).to_string()
        };
        let text = [
            good("a", "crush", "2026-10-29T10:00:00Z"),
            "{ esto no es json".to_string(),
            good("b", "cli", "2026-10-29T10:00:01Z"),
            good("c", "crush", "2026-10-29T10:00:02Z"),
            r#"{"type":"INPUT"}"#.to_string(),
        ]
        .join("\n");
        std::fs::write(dir.join("log-20261029.jsonl"), text).unwrap();

        let sessions = log::group(log::read_dir(&dir));

        assert_eq!(
            sessions.iter().map(|s| (s.client.as_str(), s.traces.len())).collect::<Vec<_>>(),
            [("crush", 2), ("cli", 1)]
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
