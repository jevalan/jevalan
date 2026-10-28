//! Hook `PreToolUse` genérico: consulta `tool.risk` al daemon y, según el modo, calla o responde.
//! El `--client` fija con qué nombre reconoce el daemon la sesión y el `--dialect` el formato de
//! salida que entiende el harness. El hook solo puede endurecer los permisos, jamás relajarlos.

use clap::ValueEnum;
use jevalan_daemon::config::DEFAULT_ADDR;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::io::{Read, Write};
use std::process::ExitCode;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum Mode {
    Shadow,
    Enforce,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Shadow => "shadow",
            Mode::Enforce => "enforce",
        }
    }
}

/// Dialecto de salida que entiende el harness. Todo lo demás (leer stdin, consultar el hub,
/// interpretar la respuesta, el registro sombra) es común a cualquier cliente.
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum Dialect {
    /// Formato de Claude Code: `hookSpecificOutput.permissionDecision`.
    Claude,
    /// Formato nativo de Crush: `{"decision": …, "reason": …}`. Crush no sabe `ask`.
    Crush,
}

const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(6);

/// Respuesta del hub reducida a lo que el hook necesita.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Decision {
    pub status: String,
    pub verdict: Option<String>,
    pub source: Option<String>,
    pub reason: Option<String>,
    pub probabilities: Option<Value>,
    pub error: Option<String>,
}

impl Decision {
    fn failure(error: impl Into<String>) -> Self {
        Self { status: "error".into(), error: Some(error.into()), ..Self::default() }
    }
}

/// Siempre `SUCCESS`: ni un fallo interno, ni un pánico, deben estorbar al harness.
pub fn run(mode: Mode, client: &str, dialect: Dialect) -> ExitCode {
    let _ = std::panic::catch_unwind(|| {
        let mut raw = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut raw);
        if let Some(out) = process(mode, client, dialect, &String::from_utf8_lossy(&raw), true) {
            let mut stdout = std::io::stdout();
            let _ = writeln!(stdout, "{out}");
            let _ = stdout.flush();
        }
    });
    ExitCode::SUCCESS
}

/// Todo el trabajo; devuelve lo que ha de imprimirse, si algo.
fn process(mode: Mode, client: &str, dialect: Dialect, stdin: &str, log: bool) -> Option<String> {
    let started = Instant::now();
    let hook: Option<Value> = serde_json::from_str(stdin).ok();
    let input = hook.as_ref().and_then(extract_input);
    let decision = match &input {
        None => Decision::failure("entrada del hook ilegible"),
        Some(input) => ask(jevalan_daemon::discovery::read(), &|addr, token| query(addr, token, client, input)),
    };
    if log {
        log_line(mode, hook.as_ref(), &decision, started.elapsed().as_millis() as u64);
    }
    output(mode, dialect, &decision)
}

/// Campos que `tool.risk` admite; los ausentes se omiten. Exige `tool_name`.
pub fn extract_input(hook: &Value) -> Option<Value> {
    let tool_name = hook.get("tool_name")?.as_str()?;
    let mut input = Map::new();
    input.insert("tool_name".into(), json!(tool_name));
    input.insert("tool_input".into(), hook.get("tool_input").cloned().unwrap_or_else(|| json!({})));
    for key in ["cwd", "session_id", "tool_use_id", "transcript_path"] {
        if let Some(v) = hook.get(key).and_then(Value::as_str) {
            input.insert(key.into(), json!(v));
        }
    }
    Some(Value::Object(input))
}

type Query<'a> = dyn Fn(&str, Option<&str>) -> Result<Decision, String> + 'a;

/// A quién se le pregunta: al daemon que anuncia `daemon.json` y, si ese no contesta —o no hay
/// anuncio—, al de por defecto. Un daemon que muere sin retirar su anuncio deja el fichero caduco,
/// y entonces la dirección anunciada no lleva a ninguna parte: preguntarle solo a ella dejaba al
/// hook sin daemon para siempre.
fn ask(discovered: Option<(String, Option<String>)>, query: &Query<'_>) -> Decision {
    match discovered {
        Some((addr, token)) if addr != DEFAULT_ADDR => match query(&addr, token.as_deref()) {
            Ok(d) => d,
            // Se conserva el motivo del anuncio: es el que dice qué dirección falló y por qué.
            Err(why) => query(DEFAULT_ADDR, token.as_deref()).unwrap_or_else(|_| Decision::failure(why)),
        },
        Some((addr, token)) => query(&addr, token.as_deref()).unwrap_or_else(Decision::failure),
        None => query(DEFAULT_ADDR, None).unwrap_or_else(|_| Decision::failure("sin daemon")),
    }
}

fn query(addr: &str, token: Option<&str>, client: &str, input: &Value) -> Result<Decision, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TOTAL_TIMEOUT)
            .build()
            .map_err(|e| e.to_string())?;
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let request_id = match input.get("tool_use_id").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => format!("hook-{}-{millis}", std::process::id()),
        };
        let body = json!({
            "protocol_version": "jev-hub/0.1", "request_id": request_id,
            "capability": "tool.risk", "capability_version": "1", "input": input,
        });
        let mut req = http
            .post(format!("http://{addr}/v1/hub"))
            .header("X-Jevalan-Client", client_name(client, input))
            .json(&body);
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.map_err(|e| format!("daemon inalcanzable: {e}"))?;
        let status = resp.status();
        let reply: Value = resp.json().await.map_err(|_| format!("respuesta ilegible (HTTP {status})"))?;
        Ok(interpret(&reply))
    })
}

/// Nombre con que el daemon (y la TUI) reconocen al cliente: `<cliente>/<proyecto>:<sesión>`,
/// siendo el proyecto la última carpeta de `cwd` y la sesión los cuatro primeros caracteres de su id.
/// Viaja en una cabecera HTTP, que solo admite ASCII visible: todo lo demás se torna `_`.
pub fn client_name(base: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
    let ascii = |s: &str| s.chars().map(|c| if c.is_ascii_graphic() { c } else { '_' }).collect::<String>();
    let base = ascii(if base.is_empty() { "unknown" } else { base });
    let project = ascii(field("cwd").rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or("?"));
    let session = ascii(&field("session_id").chars().take(4).collect::<String>());
    if session.is_empty() { format!("{base}/{project}") } else { format!("{base}/{project}:{session}") }
}

/// Respuesta `jev-hub/0.1` a `Decision`.
pub fn interpret(reply: &Value) -> Decision {
    let text = |v: &Value| v.as_str().map(str::to_string);
    let Some(status) = reply.get("status").and_then(Value::as_str) else {
        return Decision::failure("respuesta sin estado");
    };
    let mut d = Decision { status: status.into(), ..Decision::default() };
    match status {
        "ok" => {
            let dec = &reply["decision"];
            d.verdict = text(&dec["verdict"]);
            d.source = text(&dec["source"]);
            d.reason = text(&dec["reason"]);
            d.probabilities = dec.get("probabilities").cloned();
        }
        "abstain" => d.reason = text(&reply["abstain"]["reason"]),
        _ => d.error = text(&reply["error"]["message"]).or_else(|| Some("fallo del hub".into())),
    }
    d
}

/// Traducción a la salida del dialecto pedido. Jamás emite `allow`: solo endurece.
pub fn output(mode: Mode, dialect: Dialect, d: &Decision) -> Option<String> {
    if mode == Mode::Shadow {
        return None;
    }
    match dialect {
        Dialect::Claude => claude_output(d),
        Dialect::Crush => crush_output(d),
    }
}

/// Dialecto de Claude Code. Ante una indecisión pide confirmación (`ask`).
fn claude_output(d: &Decision) -> Option<String> {
    let reason = d.reason.as_deref().unwrap_or("sin motivo");
    match (d.status.as_str(), d.verdict.as_deref()) {
        ("ok", Some("deny")) => Some(permission("deny", &format!("jevalan/toolgate: {reason}"))),
        ("ok", Some("ask")) => Some(permission("ask", &format!("jevalan/toolgate: {reason}"))),
        ("ok", Some("allow" | "passthrough")) => None,
        _ => {
            let why = d.error.as_deref().or(d.reason.as_deref()).unwrap_or(d.status.as_str());
            Some(permission("ask", &format!("jevalan no pudo decidir ({why})")))
        }
    }
}

/// Dialecto de Crush. Solo distingue `allow` de `deny` y no sabe pedir confirmación desde un
/// hook, así que `ask` y las indecisiones se callan y dejan decidir a sus permisos.
fn crush_output(d: &Decision) -> Option<String> {
    match (d.status.as_str(), d.verdict.as_deref()) {
        ("ok", Some("deny")) => {
            let reason = d.reason.as_deref().unwrap_or("sin motivo");
            Some(crush_verdict("deny", &format!("jevalan/tool.risk: {reason}")))
        }
        _ => None,
    }
}

/// Salida del hook en el dialecto de Claude Code (`hookSpecificOutput`).
fn permission(decision: &str, reason: &str) -> String {
    json!({ "hookSpecificOutput": {
        "hookEventName": "PreToolUse", "permissionDecision": decision, "permissionDecisionReason": reason,
    } })
    .to_string()
}

/// Salida del hook en la envoltura nativa de Crush: `{"decision": "deny", "reason": "…"}`.
/// Crush solo entiende `allow` y `deny`, y aquí jamás se emite `allow`.
fn crush_verdict(decision: &str, reason: &str) -> String {
    json!({ "decision": decision, "reason": reason }).to_string()
}

fn secret_patterns() -> &'static [(Regex, &'static str)] {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let table: [(&str, &str); 6] = [
            (r"Bearer\s+\S+", "[REDACTED]"),
            (r"sk-[A-Za-z0-9_-]{16,}", "[REDACTED]"),
            (r"gh[pousr]_[A-Za-z0-9]{20,}", "[REDACTED]"),
            (r"AKIA[0-9A-Z]{16}", "[REDACTED]"),
            (r"xox[baprs]-\S+", "[REDACTED]"),
            (r"(?i)((?:api[_-]?key|token|secret|password|passwd)\s*[=:]\s*)\S+", "${1}[REDACTED]"),
        ];
        table.iter().filter_map(|(re, rep)| Regex::new(re).ok().map(|re| (re, *rep))).collect()
    })
}

/// Tacha, en todas las cadenas, los secretos de forma conocida.
pub fn redact(value: &Value) -> Value {
    match value {
        Value::String(s) => {
            let mut out = s.clone();
            for (re, rep) in secret_patterns() {
                out = re.replace_all(&out, *rep).into_owned();
            }
            Value::String(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(redact).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), redact(v))).collect()),
        other => other.clone(),
    }
}

fn log_line(mode: Mode, hook: Option<&Value>, d: &Decision, latency_ms: u64) {
    let Some(dir) = jevalan_daemon::discovery::data_dir() else { return };
    let field = |k: &str| hook.and_then(|h| h.get(k)).cloned().unwrap_or(Value::Null);
    let line = json!({
        "ts": iso_now(), "mode": mode.as_str(), "session_id": field("session_id"),
        "tool_use_id": field("tool_use_id"), "cwd": field("cwd"), "tool_name": field("tool_name"),
        "tool_input": redact(&field("tool_input")), "status": d.status, "verdict": d.verdict,
        "source": d.source, "reason": d.reason.as_deref().map(|r| redact(&json!(r))),
        "probabilities": d.probabilities, "latency_ms": latency_ms, "error": d.error,
    });
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("shadow.jsonl")) {
        let _ = writeln!(f, "{line}");
    }
}

/// Instante actual en ISO 8601 (UTC, milisegundos).
fn iso_now() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    iso(d.as_secs() as i64, d.subsec_millis())
}

fn iso(secs: i64, millis: u32) -> String {
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Días desde 1970 a fecha civil (algoritmo de Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn red(s: &str) -> String {
        redact(&json!(s)).as_str().unwrap().to_string()
    }

    #[test]
    fn names_the_client_by_project_and_session() {
        let input = json!({"cwd": "C:\\src\\jevalan\\", "session_id": "25fe1794-e06b"});
        assert_eq!(client_name("claude-code", &input), "claude-code/jevalan:25fe");
        assert_eq!(client_name("crush", &input), "crush/jevalan:25fe");
        let name = client_name("claude-code", &json!({"cwd": "/p/año nuevo", "session_id": "ab"}));
        assert_eq!(name, "claude-code/a_o_nuevo:ab");
        assert!(reqwest::header::HeaderValue::from_str(&name).unwrap().to_str().is_ok());
        assert_eq!(client_name("crush", &json!({"cwd": "/home/a/web"})), "crush/web");
        assert_eq!(client_name("crush", &json!({})), "crush/?");
    }

    #[test]
    fn redacts_known_secret_shapes() {
        assert_eq!(red("curl -H Authorization: Bearer abc.def x"), "curl -H Authorization: [REDACTED] x");
        assert_eq!(red("k sk-or-v1-abcdefghijklmnop1234 z"), "k [REDACTED] z");
        assert_eq!(red("sk-ant-api03-abcdefghijklmnop"), "[REDACTED]");
        assert_eq!(red("t ghp_abcdefghijklmnopqrstuvwx"), "t [REDACTED]");
        assert_eq!(red("AKIAABCDEFGHIJKLMNOP"), "[REDACTED]");
        assert_eq!(red("xoxb-123-abc"), "[REDACTED]");
    }

    #[test]
    fn redacts_value_but_keeps_key_name() {
        assert_eq!(red("export API_KEY=hunter2 && ls"), "export API_KEY=[REDACTED] && ls");
        assert_eq!(red("password: s3cret"), "password: [REDACTED]");
        assert_eq!(red("git status"), "git status");
    }

    #[test]
    fn redacts_nested_strings_only() {
        let v = json!({ "a": ["token=zzz", 3, true], "b": { "c": "ok" } });
        assert_eq!(redact(&v), json!({ "a": ["token=[REDACTED]", 3, true], "b": { "c": "ok" } }));
    }

    #[test]
    fn extracts_input_and_omits_absent_fields() {
        let hook = json!({ "tool_name": "Bash", "tool_input": { "command": "ls" }, "cwd": "/x",
            "transcript_path": "/t.jsonl", "hook_event_name": "PreToolUse", "permission_mode": "default" });
        assert_eq!(
            extract_input(&hook).unwrap(),
            json!({ "tool_name": "Bash", "tool_input": { "command": "ls" }, "cwd": "/x", "transcript_path": "/t.jsonl" })
        );
        assert!(extract_input(&json!({ "tool_input": {} })).is_none());
        assert!(extract_input(&json!([1])).is_none());
    }

    fn ok(verdict: &str) -> Decision {
        interpret(
            &json!({ "status": "ok", "decision": { "verdict": verdict, "reason": "porque sí", "source": "model" } }),
        )
    }

    #[test]
    fn shadow_is_always_silent() {
        for d in [ok("deny"), ok("ask"), ok("allow"), Decision::failure("x")] {
            assert_eq!(output(Mode::Shadow, Dialect::Claude, &d), None);
            assert_eq!(output(Mode::Shadow, Dialect::Crush, &d), None);
        }
    }

    #[test]
    fn claude_dialect_enforce_denies_and_asks_but_never_allows() {
        let deny: Value = serde_json::from_str(&output(Mode::Enforce, Dialect::Claude, &ok("deny")).unwrap()).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(deny["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(deny["hookSpecificOutput"]["permissionDecisionReason"], "jevalan/toolgate: porque sí");
        let ask: Value = serde_json::from_str(&output(Mode::Enforce, Dialect::Claude, &ok("ask")).unwrap()).unwrap();
        assert_eq!(ask["hookSpecificOutput"]["permissionDecision"], "ask");
        assert_eq!(output(Mode::Enforce, Dialect::Claude, &ok("allow")), None);
        assert_eq!(output(Mode::Enforce, Dialect::Claude, &ok("passthrough")), None);
    }

    #[test]
    fn claude_dialect_enforce_asks_when_undecided() {
        let abstain = interpret(&json!({ "status": "abstain", "abstain": { "reason": "sin opinión" } }));
        let error = interpret(&json!({ "status": "error", "error": { "message": "caído" } }));
        let junk = interpret(&json!({ "nope": 1 }));
        for d in [abstain, error, junk, Decision::failure("sin daemon"), ok("rarísimo")] {
            let out: Value = serde_json::from_str(&output(Mode::Enforce, Dialect::Claude, &d).unwrap()).unwrap();
            assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "ask");
        }
    }

    #[test]
    fn crush_dialect_enforce_blocks_on_deny_and_stays_out_of_the_way_otherwise() {
        let deny: Value = serde_json::from_str(&output(Mode::Enforce, Dialect::Crush, &ok("deny")).unwrap()).unwrap();
        assert_eq!(deny["decision"], "deny");
        assert_eq!(deny["reason"], "jevalan/tool.risk: porque sí");
        // Crush no sabe pedir confirmación desde un hook: `ask` y las indecisiones se callan.
        assert_eq!(output(Mode::Enforce, Dialect::Crush, &ok("ask")), None);
        assert_eq!(output(Mode::Enforce, Dialect::Crush, &ok("allow")), None);
        assert_eq!(output(Mode::Enforce, Dialect::Crush, &ok("passthrough")), None);
        assert_eq!(output(Mode::Enforce, Dialect::Crush, &Decision::failure("sin daemon")), None);
    }

    #[test]
    fn iso_formats_epoch_and_leap_day() {
        assert_eq!(iso(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_709_210_096, 5), "2024-02-29T12:34:56.005Z");
    }

    #[test]
    fn bad_stdin_never_panics() {
        // Sin `tool_name` no se llega a consultar al daemon, cualquiera que sea el entorno.
        assert_eq!(process(Mode::Shadow, "claude-code", Dialect::Claude, "no es json", false), None);
        let out = process(Mode::Enforce, "claude-code", Dialect::Claude, "no es json", false).unwrap();
        assert!(out.contains("\"ask\""));
        assert_eq!(process(Mode::Enforce, "crush", Dialect::Crush, "no es json", false), None);
    }

    /// Doble de `query`: solo contesta desde las direcciones que se le digan, y apunta a quién preguntó.
    type Calls = Rc<RefCell<Vec<String>>>;

    fn doubler(vivas: &[&'static str]) -> (impl Fn(&str, Option<&str>) -> Result<Decision, String>, Calls) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let seen = calls.clone();
        let f = move |addr: &str, _: Option<&str>| {
            seen.borrow_mut().push(addr.to_string());
            if vivas.contains(&addr) { Ok(ok("allow")) } else { Err(format!("daemon inalcanzable: {addr}")) }
        };
        (f, calls)
    }

    #[test]
    fn a_stale_announcement_falls_back_to_the_default_daemon() {
        let (query, calls) = doubler(&[DEFAULT_ADDR]);
        let d = ask(Some(("127.0.0.1:9999".into(), None)), &query);
        assert_eq!(d.verdict.as_deref(), Some("allow"), "debía contestar el de por defecto: {:?}", d.error);
        assert_eq!(*calls.borrow(), vec!["127.0.0.1:9999".to_string(), DEFAULT_ADDR.to_string()]);
    }

    #[test]
    fn without_an_announcement_the_default_daemon_is_asked() {
        let (query, calls) = doubler(&[DEFAULT_ADDR]);
        let d = ask(None, &query);
        assert_eq!(d.verdict.as_deref(), Some("allow"), "sin fichero se prueba el de por defecto: {:?}", d.error);
        assert_eq!(*calls.borrow(), vec![DEFAULT_ADDR.to_string()]);
    }

    #[test]
    fn the_announced_daemon_is_asked_first_and_alone_when_it_answers() {
        let (query, calls) = doubler(&["127.0.0.1:9999"]);
        let d = ask(Some(("127.0.0.1:9999".into(), None)), &query);
        assert_eq!(d.verdict.as_deref(), Some("allow"));
        assert_eq!(calls.borrow().len(), 1, "si contesta, no se molesta al de por defecto");
    }

    #[test]
    fn the_default_is_not_asked_twice_when_it_is_the_announced_one() {
        let (query, calls) = doubler(&[DEFAULT_ADDR]);
        let d = ask(Some((DEFAULT_ADDR.into(), Some("t".into()))), &query);
        assert_eq!(d.verdict.as_deref(), Some("allow"));
        assert_eq!(calls.borrow().len(), 1);
    }

    #[test]
    fn the_announced_failure_is_kept_when_nobody_answers() {
        let (query, calls) = doubler(&[]);
        let d = ask(Some(("127.0.0.1:9999".into(), None)), &query);
        assert_eq!(calls.borrow().len(), 2, "se prueban las dos direcciones");
        assert!(
            d.error.as_deref().unwrap_or_default().contains("9999"),
            "el motivo ha de ser el del anuncio: {:?}",
            d.error
        );
    }
}
