//! The OTLP mirror of the session log: gathers the lines of each trace and posts it when its decision arrives.
//! Best-effort like the log itself: a collector that is down or slow costs the mirror of that trace and a count, and
//! never delays a request.

use crate::config::Otlp;
use jevalan_hub::otlp::request;
use jevalan_hub::sessions::group;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

/// Traces held while they wait for their decision; past this the buffer starts over, so a trace that never
/// decides cannot grow it without bound.
const MAX_OPEN: usize = 1024;
const POST_TIMEOUT: Duration = Duration::from_secs(5);

pub fn spawn(otlp: Otlp, failures: Arc<AtomicU64>) -> UnboundedSender<Value> {
    let (tx, mut rx) = unbounded_channel::<Value>();
    let http = reqwest::Client::builder().timeout(POST_TIMEOUT).build().unwrap_or_default();
    tokio::spawn(async move {
        let mut open: HashMap<String, Vec<Value>> = HashMap::new();
        while let Some(line) = rx.recv().await {
            let Some(id) = line["trace_id"].as_str().map(str::to_string) else { continue };
            if open.len() >= MAX_OPEN && !open.contains_key(&id) {
                open.clear();
            }
            let done = line["type"] == "DECISION";
            open.entry(id.clone()).or_default().push(line);
            if !done {
                continue;
            }
            let lines = open.remove(&id).unwrap_or_default();
            let body = group(lines).into_iter().flat_map(|s| s.traces).find_map(|t| request(&t, otlp.content));
            if let Some(body) = body {
                tokio::spawn(post(http.clone(), otlp.clone(), body, failures.clone()));
            }
        }
    });
    tx
}

async fn post(http: reqwest::Client, otlp: Otlp, body: Value, failures: Arc<AtomicU64>) {
    let mut req = http.post(&otlp.endpoint).json(&body);
    for (name, value) in &otlp.headers {
        req = req.header(name, value);
    }
    if !req.send().await.is_ok_and(|r| r.status().is_success()) {
        failures.fetch_add(1, Ordering::Relaxed);
    }
}
