//! Execution of the fixtures matrix.

use crate::load::stale;
use crate::{Case, FailedAssert, Lab, ResultLine, RunOutcome, write};
use jevalan_daemon::hub::execute::{Executed, execute};
use jevalan_daemon::hub::transport::{Recordings, Transport, system_one_reply};
use jevalan_hub::capability::Provider;
use jevalan_hub::envelope::{HubRequest, parse_request};
use jevalan_hub::lab::assert::assert;
use jevalan_hub::lab::fixture::{Fixture, FixtureMode};
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub capability: Option<String>,
    pub fixture: Option<String>,
}

impl Filter {
    fn admits(&self, case: &Case) -> bool {
        self.capability.as_deref().is_none_or(|c| c == case.cap.name)
            && self.fixture.as_deref().is_none_or(|f| f == case.fixture.id)
    }
}

impl Lab {
    /// Replays each fixture with its `replay` or with the first target's cassette; no network or processes.
    pub async fn offline(&self, filter: &Filter) -> RunOutcome {
        let started = write::Stamp::now();
        let mut results = Vec::new();
        for case in self.cases.iter().filter(|c| filter.admits(c) && c.fixture.runs_in(FixtureMode::Offline)) {
            results.push(self.offline_case(case).await);
        }
        let failed = results.iter().any(|r| r.skipped.is_none() && !r.passed);
        let report = write::offline_report(&results);
        let (run_dir, write_error) =
            match write::write_run(self, "offline", &started, &results, &report, &write::Consumed::default()) {
                Ok(dir) => (Some(dir), None),
                Err(e) => (None, Some(e)),
            };
        RunOutcome { results, failed, report, run_dir, write_error, stopped: false }
    }

    async fn offline_case(&self, case: &Case) -> ResultLine {
        let (cap, fx) = (&case.cap, &case.fixture);
        let is_flow = matches!(cap.provider, Provider::Flow { .. });
        let target = if is_flow {
            self.order.iter().find(|n| self.has_backend(n)).cloned().unwrap_or_default()
        } else {
            cap.provider_id.clone()
        };
        // Without any target (a bench that only replays) the report still needs something to put in the column.
        let mut line = ResultLine::blank(case, if target.is_empty() { "-" } else { &target }, 1, "replay");

        // The manual replay rules; the cassette only fills in the missing keys. A `batch`'s keys
        // are those of its questions, `"<node>.<question>"`, which is what the daemon will ask for (D3).
        let nodes = self.jev_questions(cap);
        let mut rec = Recordings::new();
        for (key, value) in &fx.replay {
            let reply = match nodes.iter().find(|(n, _)| n == key) {
                Some((_, ty)) => {
                    system_one_reply(key, ty.as_str(), &value["answer"], value["confidence"].as_f64().unwrap_or(0.0))
                }
                None => value.clone(),
            };
            rec.insert(key.clone(), reply);
        }
        match self.cassette(&cap.name, &fx.id, &target) {
            Ok(Some(cassette)) => {
                if let Some(why) = stale(&cassette, fx) {
                    line.skipped = Some(why);
                    return line;
                }
                for (key, value) in cassette["replies"].as_object().into_iter().flatten() {
                    if !rec.contains_key(key) {
                        rec.insert(key.clone(), value.clone());
                        line.source = "cassette".into();
                    }
                }
            }
            Ok(None) => {}
            // A broken cassette is not a case to record: it is a failure that must be seen.
            Err(e) => {
                line.status = "error".into();
                line.failed_asserts =
                    vec![FailedAssert { why: "la cassette debe poder leerse".into(), reason: Some(e) }];
                return line;
            }
        }

        let req = match request(case) {
            Ok(req) => req,
            Err(e) => {
                line.skipped = Some(e);
                return line;
            }
        };
        let flow = self.config.targets.get(&target).and_then(|t| self.target_flow(cap, t));
        let done = execute(&self.state, &req, Some(cap), flow, &Transport::Replay(Arc::new(rec))).await;
        line.elapsed_ms = done.measure.elapsed_ms;
        if let Some(key) = done.missing {
            line.skipped = Some(format!("falta la grabación de {key}"));
            return line;
        }
        finish_line(&mut line, fx, &done);
        line
    }
}

/// A fixture's `jev-hub` request.
pub(crate) fn request(case: &Case) -> Result<HubRequest, String> {
    let (cap, fx) = (&case.cap, &case.fixture);
    let body = json!({
        "protocol_version": "jev-hub/0.1", "request_id": fx.id,
        "capability": cap.name, "capability_version": cap.version, "input": fx.input,
    });
    parse_request(body.to_string().as_bytes())
}

/// Dumps the response and the measurement into the line and evaluates the assertions.
pub(crate) fn finish_line(line: &mut ResultLine, fx: &Fixture, done: &Executed) {
    let failures = assert(fx, &done.response);
    line.status = done.response["status"].as_str().unwrap_or("error").to_string();
    line.code = done.response["error"]["code"].as_str().map(String::from);
    line.error_message = error_message(&done.response);
    line.passed = failures.is_empty();
    line.failed_asserts = failures.into_iter().map(|f| FailedAssert { why: f.why, reason: f.reason }).collect();
    line.elapsed_ms = done.measure.elapsed_ms;
    line.called = done.measure.backend_calls + done.measure.specialist_calls > 0;
    line.confidence = done.measure.confidence;
    line.input_tokens = done.measure.input_tokens;
    line.output_tokens = done.measure.output_tokens;
}

/// Error text of a response whose status is `error`; in any other case, nothing.
fn error_message(response: &serde_json::Value) -> Option<String> {
    if response["status"].as_str() != Some("error") {
        return None;
    }
    response["error"]["message"].as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::error_message;
    use serde_json::json;

    #[test]
    fn records_the_message_only_for_errors() {
        let failed = json!({ "status": "error", "error": { "code": "internal", "message": "node not found" } });
        assert_eq!(error_message(&failed).as_deref(), Some("node not found"));
        assert_eq!(error_message(&json!({ "status": "ok", "error": { "message": "x" } })), None);
        assert_eq!(error_message(&json!({ "status": "error" })), None);
    }
}
