//! `jevalan sessions`: reads the session log (`list`, `show`). The log is written by the daemon when
//! `sessions.enabled` is on; see `jevalan_daemon::session`.

use jevalan_daemon::session::log::{Session, Trace, group, read_dir};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

pub struct Opts {
    pub dir: Option<PathBuf>,
    pub action: Action,
}

pub enum Action {
    List { since: Option<String>, client: Option<String>, json: bool },
    Show { target: String, json: bool },
}

pub fn run(opts: Opts) -> ExitCode {
    let Some(dir) = opts.dir.or_else(jevalan_daemon::session::default_dir) else {
        eprintln!("error: no se encontró el directorio de datos; indica uno con --dir");
        return ExitCode::FAILURE;
    };
    if !dir.is_dir() {
        eprintln!("error: no existe {}: activa «sessions.enabled» en la configuración del daemon", dir.display());
        return ExitCode::FAILURE;
    }
    let sessions = group(read_dir(&dir));
    match opts.action {
        Action::List { since, client, json } => {
            let sessions = filter(sessions, since.as_deref(), client.as_deref());
            println!("{}", if json { list_json(&sessions) } else { list_text(&sessions) });
            ExitCode::SUCCESS
        }
        Action::Show { target, json } => match find(&sessions, &target) {
            Some(traces) => {
                println!("{}", if json { show_json(&traces) } else { show_text(&traces) });
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("error: ninguna sesión ni traza coincide con «{target}»");
                ExitCode::FAILURE
            }
        },
    }
}

/// Keeps the traces from `since` on (RFC 3339 prefix compare) and the clients that contain `client`.
fn filter(sessions: Vec<Session>, since: Option<&str>, client: Option<&str>) -> Vec<Session> {
    sessions
        .into_iter()
        .filter(|s| client.is_none_or(|c| s.client.contains(c)))
        .map(|mut s| {
            s.traces.retain(|t| since.is_none_or(|since| t.start() >= since));
            s
        })
        .filter(|s| !s.traces.is_empty())
        .collect()
}

/// The traces of the client named `target`, or the one trace with that id.
fn find(sessions: &[Session], target: &str) -> Option<Vec<Trace>> {
    if let Some(s) = sessions.iter().find(|s| s.client == target) {
        return Some(s.traces.clone());
    }
    sessions.iter().flat_map(|s| &s.traces).find(|t| t.trace_id == target).map(|t| vec![t.clone()])
}

/// What the capability decided, in one word: the verdict it carries, or its status when it carries none.
fn verdict(decision: &Value) -> String {
    if decision["status"] != "ok" {
        return decision["status"].as_str().unwrap_or("?").to_string();
    }
    ["decision", "verdict", "outcome"].iter().find_map(|k| decision["decision"][k].as_str()).unwrap_or("ok").to_string()
}

fn verdicts(session: &Session) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for trace in &session.traces {
        let word = trace.first("DECISION").map_or_else(|| "sin decisión".to_string(), verdict);
        *counts.entry(word).or_insert(0) += 1;
    }
    counts
}

fn day(ts: &str) -> &str {
    ts.get(..16).unwrap_or(ts)
}

fn list_text(sessions: &[Session]) -> String {
    if sessions.is_empty() {
        return "No hay sesiones registradas.".into();
    }
    let mut out = String::from("cliente | inicio | fin | trazas | decisiones\n");
    for s in sessions {
        let (first, last) = (s.traces.first().map_or("", Trace::start), s.traces.last().map_or("", Trace::end));
        let counts: Vec<String> = verdicts(s).iter().map(|(k, n)| format!("{k}×{n}")).collect();
        out.push_str(&format!(
            "{} | {} | {} | {} | {}\n",
            s.client,
            day(first),
            day(last),
            s.traces.len(),
            counts.join(" ")
        ));
    }
    out.trim_end().to_string()
}

fn list_json(sessions: &[Session]) -> String {
    let rows: Vec<Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "client": s.client,
                "start": s.traces.first().map_or("", Trace::start),
                "end": s.traces.last().map_or("", Trace::end),
                "traces": s.traces.len(),
                "decisions": verdicts(s),
            })
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap_or_default()
}

fn show_json(traces: &[Trace]) -> String {
    let rows: Vec<Value> =
        traces.iter().map(|t| json!({ "trace_id": t.trace_id, "start": t.start(), "lines": t.lines })).collect();
    serde_json::to_string_pretty(&rows).unwrap_or_default()
}

/// One trace as a timeline: the call, the path through the flow, the backends touched and the decision.
fn show_text(traces: &[Trace]) -> String {
    let mut out = String::new();
    for trace in traces {
        let client = trace.lines.iter().find_map(|l| l["client"].as_str()).unwrap_or("unknown");
        out.push_str(&format!("== {} · {} · {}\n", trace.trace_id, client, trace.start()));
        if let Some(input) = trace.first("INPUT") {
            out.push_str(&format!("entrada ({}): {}\n", input["capability"].as_str().unwrap_or("?"), input["input"]));
        }
        let events = |ty: &'static str| trace.all("EVENT").filter(move |l| l["payload"]["type"] == ty);
        for end in events("NODE_END") {
            let p = &end["payload"]["payload"];
            out.push_str(&format!(
                "  nodo {} → puerto {}\n",
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
            out.push_str(&format!(
                "  backend {} nodo {}: {status}{ms}\n",
                call["backend"].as_str().unwrap_or("?"),
                call["node"].as_str().unwrap_or("?")
            ));
        }
        match trace.first("DECISION") {
            Some(d) => {
                let detail = d["decision"].as_object().map_or(String::new(), |_| format!(" {}", d["decision"]));
                out.push_str(&format!(
                    "decisión: {}{}{}\n",
                    verdict(d),
                    d["code"].as_str().map_or(String::new(), |c| format!(" ({c})")),
                    detail
                ));
            }
            None => out.push_str("decisión: ninguna registrada\n"),
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(trace: &str, client: &str, ts: &str, ty: &str, extra: Value) -> Value {
        let mut v = json!({ "ts": ts, "trace_id": trace, "client": client, "type": ty });
        for (k, val) in extra.as_object().into_iter().flatten() {
            v[k] = val.clone();
        }
        v
    }

    fn sample() -> Vec<Session> {
        let lines = vec![
            line(
                "a",
                "crush",
                "2026-10-29T10:00:00Z",
                "INPUT",
                json!({ "capability": "tool.risk", "input": { "tool_name": "bash" } }),
            ),
            line(
                "a",
                "crush",
                "2026-10-29T10:00:01Z",
                "BACKEND_CALL",
                json!({ "backend": "kev", "node": "risks", "response": { "status": 200 } }),
            ),
            line(
                "a",
                "crush",
                "2026-10-29T10:00:02Z",
                "DECISION",
                json!({ "status": "ok", "decision": { "decision": "ask" } }),
            ),
            line(
                "b",
                "crush",
                "2026-10-30T09:00:00Z",
                "DECISION",
                json!({ "status": "error", "code": "provider_failed" }),
            ),
            line(
                "c",
                "cli",
                "2026-10-30T10:00:00Z",
                "DECISION",
                json!({ "status": "ok", "decision": { "decision": "allow" } }),
            ),
        ];
        group(lines)
    }

    #[test]
    fn the_list_counts_decisions_by_verdict() {
        let text = list_text(&sample());
        assert!(text.contains("crush | 2026-10-29T10:00 | 2026-10-30T09:00 | 2 | ask×1 error×1"), "{text}");
        assert!(text.contains("cli | 2026-10-30T10:00 | 2026-10-30T10:00 | 1 | allow×1"), "{text}");
        assert_eq!(list_text(&[]), "No hay sesiones registradas.");
    }

    #[test]
    fn filters_by_date_and_client() {
        let by_date = filter(sample(), Some("2026-10-30"), None);
        assert_eq!(
            by_date.iter().map(|s| (s.client.as_str(), s.traces.len())).collect::<Vec<_>>(),
            [("crush", 1), ("cli", 1)]
        );
        let by_client = filter(sample(), None, Some("cli"));
        assert_eq!(by_client.len(), 1);
    }

    #[test]
    fn show_draws_the_timeline_of_a_trace() {
        let traces = find(&sample(), "a").expect("traza");
        let text = show_text(&traces);
        assert!(text.contains("== a · crush"), "{text}");
        assert!(text.contains(r#"entrada (tool.risk): {"tool_name":"bash"}"#), "{text}");
        assert!(text.contains("backend kev nodo risks: HTTP 200"), "{text}");
        assert!(text.contains("decisión: ask"), "{text}");
        let failed = show_text(&find(&sample(), "b").unwrap());
        assert!(failed.contains("decisión: error (provider_failed)"), "{failed}");
    }

    #[test]
    fn a_client_or_a_trace_id_finds_its_traces_and_anything_else_finds_none() {
        assert_eq!(find(&sample(), "crush").unwrap().len(), 2);
        assert_eq!(find(&sample(), "c").unwrap().len(), 1);
        assert!(find(&sample(), "nadie").is_none());
    }

    #[test]
    fn json_output_parses_back() {
        let list: Value = serde_json::from_str(&list_json(&sample())).unwrap();
        assert_eq!(list[0]["client"], "crush");
        assert_eq!(list[0]["decisions"]["ask"], 1);
        let shown: Value = serde_json::from_str(&show_json(&find(&sample(), "a").unwrap())).unwrap();
        assert_eq!(shown[0]["trace_id"], "a");
    }
}
