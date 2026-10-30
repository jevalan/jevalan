//! A recorded trace as an OTLP/JSON export request, in the OpenTelemetry GenAI vocabulary.
//!
//! Pure on purpose: it takes the lines of the session log and returns the body to POST, so that what is exported can
//! be tested without a network. The JSONL stays the source of truth; this is a mirror for any standard viewer.
//!
//! The tool's input and the decision are only rendered when the caller asks for the content. Backend requests, which
//! carry the input inside their questions, are never rendered.

use crate::sessions::{Trace, verdict};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const KIND_INTERNAL: u8 = 1;
const KIND_CLIENT: u8 = 3;
const STATUS_OK: u8 = 1;
const STATUS_ERROR: u8 = 2;

/// The `POST /v1/traces` body of one trace; `None` if the trace has no input to anchor it.
pub fn request(trace: &Trace, content: bool) -> Option<Value> {
    let input = trace.first("INPUT")?;
    let capability = input["capability"].as_str().unwrap_or("?");
    let client = input["client"].as_str().unwrap_or("unknown");
    let trace_id = hex_id(&trace.trace_id, 16);
    let root_id = hex_id(&format!("{}:root", trace.trace_id), 8);
    let start = nanos(input).unwrap_or(0);

    let latencies: Vec<u64> = trace
        .all("EVENT")
        .filter(|l| l["payload"]["type"] == "BACKEND_RESPONSE")
        .filter_map(|l| l["payload"]["payload"]["duration_ms"].as_u64())
        .collect();
    let mut spans = Vec::new();
    let mut last_end = start;
    for (i, call) in trace.all("BACKEND_CALL").enumerate() {
        let end = nanos(call).unwrap_or(start).max(start);
        let begin = end.saturating_sub(latencies.get(i).copied().unwrap_or(0) * 1_000_000).max(start);
        last_end = last_end.max(end);
        let model = call["request"]["model"].as_str().or_else(|| call["response"]["body"]["model"].as_str());
        let status = call["response"]["status"].as_u64();
        let mut attrs = vec![
            text("gen_ai.operation.name", "chat"),
            text("gen_ai.provider.name", call["backend"].as_str().unwrap_or("?")),
            text("jevalan.node", call["node"].as_str().unwrap_or("?")),
        ];
        if let Some(model) = model {
            attrs.push(text("gen_ai.request.model", model));
        }
        if let Some(status) = status {
            attrs.push(int("http.response.status_code", status));
        }
        spans.push(span(Span {
            trace_id: &trace_id,
            span_id: &hex_id(&format!("{}:call:{i}", trace.trace_id), 8),
            parent: Some(&root_id),
            name: &format!("chat {}", model.unwrap_or("unknown")),
            kind: KIND_CLIENT,
            start: begin,
            end,
            attrs,
            failed: status.is_none_or(|s| s >= 400),
        }));
    }

    let decision = trace.first("DECISION");
    let end = decision.and_then(nanos).unwrap_or(start).max(last_end);
    let mut attrs = vec![
        text("gen_ai.operation.name", "invoke_workflow"),
        text("gen_ai.conversation.id", client),
        text("langfuse.session.id", client),
        text("jevalan.client", client),
        text("jevalan.capability", capability),
    ];
    if let Some(d) = decision {
        attrs.push(text("jevalan.status", d["status"].as_str().unwrap_or("?")));
        attrs.push(text("jevalan.verdict", &verdict(d)));
    }
    if content {
        attrs.push(text("gen_ai.tool.call.arguments", &input["input"].to_string()));
        if let Some(d) = decision.and_then(|d| d.get("decision")) {
            attrs.push(text("gen_ai.tool.call.result", &d.to_string()));
        }
    }
    spans.insert(
        0,
        span(Span {
            trace_id: &trace_id,
            span_id: &root_id,
            parent: None,
            name: &format!("invoke_workflow {capability}"),
            kind: KIND_INTERNAL,
            start,
            end,
            attrs,
            failed: decision.is_some_and(|d| d["status"] == "error"),
        }),
    );

    Some(json!({
        "resourceSpans": [{
            "resource": { "attributes": [text("service.name", "jevalan")] },
            "scopeSpans": [{ "scope": { "name": "jevalan.sessions" }, "spans": spans }],
        }],
    }))
}

struct Span<'a> {
    trace_id: &'a str,
    span_id: &'a str,
    parent: Option<&'a str>,
    name: &'a str,
    kind: u8,
    start: u64,
    end: u64,
    attrs: Vec<Value>,
    failed: bool,
}

fn span(s: Span) -> Value {
    let mut v = json!({
        "traceId": s.trace_id,
        "spanId": s.span_id,
        "name": s.name,
        "kind": s.kind,
        // OTLP/JSON carries 64-bit integers as strings.
        "startTimeUnixNano": s.start.to_string(),
        "endTimeUnixNano": s.end.to_string(),
        "attributes": s.attrs,
        "status": { "code": if s.failed { STATUS_ERROR } else { STATUS_OK } },
    });
    if let Some(parent) = s.parent {
        v["parentSpanId"] = json!(parent);
    }
    v
}

fn text(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": { "stringValue": value } })
}

fn int(key: &str, value: u64) -> Value {
    json!({ "key": key, "value": { "intValue": value.to_string() } })
}

/// `bytes` bytes of a hash of `seed`, in hex: deterministic, so exporting a trace again names the same spans.
fn hex_id(seed: &str, bytes: usize) -> String {
    Sha256::digest(seed.as_bytes()).iter().take(bytes).map(|b| format!("{b:02x}")).collect()
}

/// Unix nanoseconds of the `ts` (RFC 3339, UTC, whole seconds) of a log line.
fn nanos(line: &Value) -> Option<u64> {
    let ts = line["ts"].as_str()?;
    let field = |from: usize, to: usize| ts.get(from..to)?.parse::<i64>().ok();
    let (y, m, d) = (field(0, 4)?, field(5, 7)?, field(8, 10)?);
    let (h, min, s) = (field(11, 13)?, field(14, 16)?, field(17, 19)?);
    // Howard Hinnant's days-from-civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from((days * 86_400 + h * 3600 + min * 60 + s) * 1_000_000_000).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::group;

    fn line(ts: &str, ty: &str, extra: Value) -> Value {
        let mut v = json!({ "ts": ts, "trace_id": "t1", "client": "crush/demo:1", "type": ty });
        for (k, val) in extra.as_object().into_iter().flatten() {
            v[k] = val.clone();
        }
        v
    }

    fn trace(status: &str) -> Trace {
        let lines = vec![
            line("2026-10-29T10:00:00Z", "INPUT", json!({ "capability": "tool.risk", "input": { "cmd": "SECRETO" } })),
            line(
                "2026-10-29T10:00:01Z",
                "EVENT",
                json!({ "payload": { "type": "BACKEND_RESPONSE", "payload": { "duration_ms": 400 } } }),
            ),
            line(
                "2026-10-29T10:00:01Z",
                "BACKEND_CALL",
                json!({ "backend": "ollama", "node": "risks", "request": { "model": "nimble" },
                        "response": { "status": 200, "body": {} } }),
            ),
            line(
                "2026-10-29T10:00:02Z",
                "DECISION",
                json!({ "capability": "tool.risk", "status": status, "decision": { "verdict": "allow" } }),
            ),
        ];
        group(lines).remove(0).traces.remove(0)
    }

    fn spans(body: &Value) -> &Vec<Value> {
        body["resourceSpans"][0]["scopeSpans"][0]["spans"].as_array().unwrap()
    }

    #[test]
    fn a_trace_is_a_root_span_with_a_child_per_backend_call() {
        let body = request(&trace("ok"), false).unwrap();
        let spans = spans(&body);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0]["name"], "invoke_workflow tool.risk");
        assert_eq!(spans[1]["name"], "chat nimble");
        assert_eq!(spans[1]["parentSpanId"], spans[0]["spanId"]);
        assert_eq!(spans[1]["traceId"], spans[0]["traceId"]);
        assert_eq!(spans[0]["traceId"].as_str().unwrap().len(), 32);
        // The call lasted 400 ms and ended at :01.
        let (s, e) = (&spans[1]["startTimeUnixNano"], &spans[1]["endTimeUnixNano"]);
        assert_eq!(
            e.as_str().unwrap().parse::<u64>().unwrap() - s.as_str().unwrap().parse::<u64>().unwrap(),
            400_000_000
        );
        assert_eq!(e, "1793268001000000000");
    }

    #[test]
    fn ids_are_deterministic_and_status_follows_the_decision() {
        assert_eq!(request(&trace("ok"), false), request(&trace("ok"), false));
        assert_eq!(spans(&request(&trace("ok"), false).unwrap())[0]["status"]["code"], STATUS_OK);
        assert_eq!(spans(&request(&trace("error"), false).unwrap())[0]["status"]["code"], STATUS_ERROR);
    }

    #[test]
    fn the_input_and_the_decision_travel_only_when_the_content_is_asked_for() {
        let without = request(&trace("ok"), false).unwrap().to_string();
        assert!(!without.contains("SECRETO") && !without.contains("gen_ai.tool.call"), "{without}");
        assert!(without.contains("crush/demo:1"), "the session still names the client");
        let with = request(&trace("ok"), true).unwrap().to_string();
        assert!(with.contains("SECRETO") && with.contains("gen_ai.tool.call.result"), "{with}");
    }

    #[test]
    fn a_trace_without_input_has_nothing_to_export() {
        let mut t = trace("ok");
        t.lines.retain(|l| l["type"] != "INPUT");
        assert!(request(&t, true).is_none());
    }
}
