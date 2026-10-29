//! Bridge from the shadow log to the fixture bench: turns real decisions from `shadow.jsonl` into
//! `tool.risk` fixtures that the laboratory replays with the local flow.
//!
//! Note that nothing is decided anew here: the `replay` is the decision already taken and the
//! `expect` demands that the current flow takes it again. A fixture that passes is, therefore, a real case
//! that today's flow reproduces.

use crate::write::fnv1a64;
use jevalan_flow::binary_confidence;
use jevalan_hub::shadow::Entry;
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Capability whose fixtures are imported: the only one the shadow log covers.
const CAPABILITY: &str = "tool.risk";
/// `batch` node of the local flow (`config/flows/tool-risk-local.yaml`). Its questions (and not the
/// provider's name) are the `replay` key, `"<node>.<question>"`; `lab check` requires it so (D3).
const RISK_NODE: &str = "risks";
/// The four questions of the `risks` node. The log records eight probabilities, but the local flow
/// only asks these four: the others are not a valid key and `lab check` would reject them.
const RISK_QUESTIONS: [&str; 4] = ["destructive", "exfiltration", "privilege", "secret_exposure"];
/// Thresholds of the local flow: from 0.85 it denies and from 0.55 it asks.
const DENY_THRESHOLD: f64 = 0.85;
const ASK_THRESHOLD: f64 = 0.55;
/// Tools that `tool.risk` does not judge: the log leaves them without a verdict.
const UNJUDGED_TOOLS: [&str; 3] = ["Agent", "SubagentHandback", "Skill"];
/// Names tried before giving the id up for lost: the original and `<id>-2` … `<id>-9`.
const MAX_NAMES: u32 = 9;
/// Maximum length of the id's readable crumb.
const CRUMB_CHARS: usize = 32;
/// What the laboratory requires at the bench root; written only if missing.
const LAB_YAML: &str = "\
# Banco de fixtures importados del registro sombra (`jevalan shadow fixtures`).
# Los reproduce el flujo local `tool-risk-local` (nodo `batch` `risks`): se corren con `lab offline` y con la
# `--config` del daemon que sirve ese flujo. Sus fixtures son de modo `offline`: reproducen decisiones ya
# grabadas, no miden a ningún modelo. Véase docs/local-development.md.
# Los targets son opcionales: `offline` no necesita ninguno. Para `lab record` o `lab live`, declare aquí al
# menos uno (un nombre, el backend del daemon y el modelo) junto con un `budget`.
targets: {}
";

/// What the command line asked for.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Directory of the fixture bench; created if it does not exist.
    pub lab_dir: PathBuf,
    /// Only the `n` highest-risk cases; `None` wants them all.
    pub max: Option<usize>,
}

/// The fixture as written: the same fields, in the same order, as
/// `jevalan_hub::lab::fixture::Fixture` (`deny_unknown_fields` there).
#[derive(Debug, Serialize)]
struct OutFixture {
    id: String,
    version: u32,
    description: String,
    /// `offline` only, and not by oversight: this bench replays already-recorded decisions with `replay`, and
    /// in live mode the `replay` is not used (the model answers, at a high temperature), so that
    /// the exact equality the `expect` demands is impossible by construction. Running it live only
    /// produces a misleading zero.
    modes: Vec<&'static str>,
    input: Value,
    replay: BTreeMap<String, Replay>,
    expect: Vec<OutExpect>,
}

/// Recorded response to a question of the `batch` node: the flow requires `answer` and `confidence`.
#[derive(Debug, Serialize)]
struct Replay {
    answer: f64,
    confidence: f64,
}

#[derive(Debug, Serialize)]
struct OutExpect {
    cel: String,
    why: String,
}

/// Tally of everything discarded and everything written: each filter, counted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tally {
    /// Log lines that were not usable JSON.
    pub ignored: usize,
    pub no_verdict: usize,
    pub bad_status: usize,
    pub unjudged_tool: usize,
    pub truncated_context: usize,
    pub no_input: usize,
    pub missing_probabilities: usize,
    pub fixed_rule: usize,
    pub not_reproduced: usize,
    pub repeated: usize,
    pub beyond_max: usize,
    pub written: usize,
    pub renamed: usize,
    pub identical: usize,
    pub no_room: usize,
    pub lab_yaml_written: bool,
}

impl Tally {
    /// How many fixtures ended up in the bench: the new ones and the renamed ones.
    pub fn fixtures(&self) -> usize {
        self.written + self.renamed
    }

    /// The tally in prose, in the order the filters are applied.
    pub fn render(&self) -> String {
        let mut out = String::new();
        count(&mut out, "líneas ignoradas del registro", self.ignored);
        count(&mut out, "descartadas sin veredicto", self.no_verdict);
        count(&mut out, "descartadas por estado distinto de ok", self.bad_status);
        count(&mut out, "descartadas por herramienta que no se juzga", self.unjudged_tool);
        count(&mut out, "descartadas por contexto truncado", self.truncated_context);
        count(&mut out, "descartadas sin entrada", self.no_input);
        count(&mut out, "descartadas sin las cuatro probabilidades", self.missing_probabilities);
        count(&mut out, "descartadas porque una regla fija decide antes del batch", self.fixed_rule);
        count(&mut out, "descartadas porque el flujo ya no las reproduce", self.not_reproduced);
        count(&mut out, "descartadas por repetidas", self.repeated);
        count(&mut out, "recortadas por --max", self.beyond_max);
        count(&mut out, "fixtures escritos", self.fixtures());
        count(&mut out, "ya presentes e idénticos", self.identical);
        count(&mut out, "renombrados para no pisar", self.renamed);
        count(&mut out, "sin sitio tras -9", self.no_room);
        let _ = writeln!(out, "  lab.yaml: {}", if self.lab_yaml_written { "escrito" } else { "ya estaba" });
        out
    }
}

fn count(out: &mut String, label: &str, n: usize) {
    let _ = writeln!(out, "  {label}: {n}");
}

/// Turns the log entries into fixtures and writes them under `opts.lab_dir`; returns the
/// tally. A write failure is an error; finding no room for an id is a counted discard.
pub fn import(entries: &[Entry], ignored: usize, opts: &Opts) -> Result<Tally, String> {
    let mut tally = Tally { ignored, ..Tally::default() };
    let mut candidates = Vec::new();
    for e in entries {
        if let Some(c) = candidate(e, &mut tally) {
            candidates.push(c);
        }
    }
    // From highest risk to lowest: `--max` must keep what deserves the budget. Breaking ties by
    // instant makes the trimming reproducible.
    candidates.sort_by(|a, b| b.risk_max.total_cmp(&a.risk_max).then_with(|| a.entry.ts.cmp(&b.entry.ts)));
    // Duplicates are discarded after sorting: the highest-risk one survives.
    let mut seen = BTreeSet::new();
    let mut chosen = Vec::new();
    for c in candidates {
        if seen.insert(c.key.clone()) {
            chosen.push(c);
        } else {
            tally.repeated += 1;
        }
    }
    let keep = opts.max.unwrap_or(usize::MAX).min(chosen.len());
    tally.beyond_max = chosen.len() - keep;
    chosen.truncate(keep);

    if !chosen.is_empty() {
        let dir = opts.lab_dir.join("fixtures").join(CAPABILITY);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for c in &chosen {
            write_one(&dir, c, &mut tally)?;
        }
        ensure_lab_yaml(&opts.lab_dir, &mut tally)?;
    }
    Ok(tally)
}

/// A case that passed the filters: its input, its risk and the question that decides.
struct Candidate<'a> {
    entry: &'a Entry,
    input: Value,
    /// The four recorded probabilities, ready for the `replay`.
    risk: BTreeMap<String, f64>,
    /// The highest-probability question: the one the `expect` checks against the decision.
    question: &'static str,
    /// The highest of the four risks; orders the `--max` trimming.
    risk_max: f64,
    /// Tool and command: the same twice is a repeated case.
    key: String,
}

/// Applies the filters to an entry; `None` if it yields no fixture, counting the reason.
fn candidate<'a>(e: &'a Entry, tally: &mut Tally) -> Option<Candidate<'a>> {
    let Some(recorded) = e.verdict.as_deref() else {
        tally.no_verdict += 1;
        return None;
    };
    if e.status != "ok" {
        tally.bad_status += 1;
        return None;
    }
    if UNJUDGED_TOOLS.contains(&e.tool_name.as_str()) {
        tally.unjudged_tool += 1;
        return None;
    }
    if e.reason.as_deref().is_some_and(|r| r.contains("input too large")) {
        tally.truncated_context += 1;
        return None;
    }
    let Some(tool_input) =
        e.tool_input.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).filter(Value::is_object)
    else {
        tally.no_input += 1;
        return None;
    };
    let input = json!({ "tool_name": e.tool_name, "tool_input": tool_input });

    let mut risk = BTreeMap::new();
    for q in RISK_QUESTIONS {
        let Some(p) = e.probabilities.get(q) else {
            tally.missing_probabilities += 1;
            return None;
        };
        risk.insert(q.to_string(), *p);
    }
    // The highest risk, and in a tie the flow's first question: the one that decides.
    let mut question = RISK_QUESTIONS[0];
    let mut risk_max = risk[question];
    for q in RISK_QUESTIONS {
        if risk[q] > risk_max {
            question = q;
            risk_max = risk[q];
        }
    }

    match local_verdict(&input, risk_max) {
        // A fixed rule goes ahead of the `batch`: the `replay` would not reach the decision and the probabilities'
        // `expect` could not be met.
        Local::FixedRule => {
            tally.fixed_rule += 1;
            return None;
        }
        Local::Batch(verdict) if verdict != recorded => {
            tally.not_reproduced += 1;
            return None;
        }
        Local::Batch(_) => {}
    }

    let key = repetition_key(e, &input);
    Some(Candidate { entry: e, input, risk, question, risk_max, key })
}

/// What the local flow would decide today with the recorded probabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Local {
    /// The `batch` node decides by thresholds.
    Batch(&'static str),
    /// One of the flow's fixed rules decides before reaching the `batch`.
    FixedRule,
}

fn local_verdict(input: &Value, risk_max: f64) -> Local {
    if let Some(command) = input.get("tool_input").and_then(|t| t.get("command")).and_then(Value::as_str)
        && fixed_rule(command)
    {
        return Local::FixedRule;
    }
    let verdict = if risk_max >= DENY_THRESHOLD {
        "deny"
    } else if risk_max >= ASK_THRESHOLD {
        "ask"
    } else {
        "allow"
    };
    Local::Batch(verdict)
}

/// The local flow's two fixed rules (`rm_root` and `pipe_sh`), traced from `config/signals/rm_root.yaml` and `config/signals/pipe_sh.yaml`.
/// If they change there, they change here too.
fn fixed_rule(command: &str) -> bool {
    static RULES: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (rm_root, pipe_sh) = RULES.get_or_init(|| {
        let rm_root = Regex::new(r"rm\s+-[a-zA-Z]*[rf][a-zA-Z]*\s+(/|~|\$HOME)(\s|$)").expect("regla fija rm_root");
        let pipe_sh = Regex::new(r"(curl|wget|iwr|Invoke-WebRequest)[^|]*\|\s*(sh|bash|zsh|iex|Invoke-Expression)")
            .expect("regla fija pipe_sh");
        (rm_root, pipe_sh)
    });
    rm_root.is_match(command) || pipe_sh.is_match(command)
}

/// Repetition key: the tool and "the command" (`tool_input.command` when there is one, and otherwise the
/// whole input, so as not to confuse two edits to different files).
fn repetition_key(e: &Entry, input: &Value) -> String {
    let command = input.get("tool_input").and_then(|t| t.get("command")).and_then(Value::as_str);
    match command {
        Some(c) => format!("{}\u{1}{c}", e.tool_name),
        None => format!("{}\u{1}{}", e.tool_name, input.get("tool_input").map_or(String::new(), Value::to_string)),
    }
}

/// Fixture id: `shadow-<tool>-<readable crumb>-<FNV-1a of the input>`. It is a slug by
/// construction, stable even as the log grows, and cannot clobber a hand-written fixture: all
/// of those carry another name.
fn id_of(tool: &str, input: &Value) -> String {
    format!("shadow-{}-{}-{}", slug(tool, CRUMB_CHARS), crumb(input), fnv1a64(&input.to_string()))
}

/// The id's readable part: the command, and if there is none the file or pattern the input names.
fn crumb(input: &Value) -> String {
    let field = |k: &str| input.get("tool_input").and_then(|t| t.get(k)).and_then(Value::as_str);
    let text = field("command").or_else(|| field("file_path")).or_else(|| field("pattern")).unwrap_or("");
    let crumb = slug(text, CRUMB_CHARS);
    if crumb.is_empty() { "caso".into() } else { crumb }
}

/// Reduces to `[a-z0-9-]`: anything that is not a lowercase ASCII letter or digit becomes a hyphen, consecutive hyphens
/// merge and those at the ends are trimmed; cut to `max` characters.
fn slug(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut pending = false;
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending && !out.is_empty() {
                out.push('-');
            }
            pending = false;
            out.push(c);
            if out.len() >= max {
                break;
            }
        } else {
            pending = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Writes the fixture without clobbering anyone: identical content counts as written, different content looks for
/// `<id>-2` … `<id>-9`, and with no room it is counted as a failure.
fn write_one(dir: &Path, c: &Candidate<'_>, tally: &mut Tally) -> Result<(), String> {
    let base = id_of(&c.entry.tool_name, &c.input);
    for attempt in 0..MAX_NAMES {
        let id = if attempt == 0 { base.clone() } else { format!("{base}-{}", attempt + 1) };
        let yaml = render(&id, c)?;
        let path = dir.join(format!("{id}.yaml"));
        if path.is_file() {
            if std::fs::read_to_string(&path).is_ok_and(|t| t == yaml) {
                tally.identical += 1;
                return Ok(());
            }
            continue;
        }
        std::fs::write(&path, yaml).map_err(|e| format!("{}: {e}", path.display()))?;
        if attempt == 0 {
            tally.written += 1;
        } else {
            tally.renamed += 1;
        }
        return Ok(());
    }
    tally.no_room += 1;
    Ok(())
}

/// The fixture's YAML. The `replay` joins the four questions of the `batch` (each with its probability
/// and the confidence the flow would compute for a `noul` without one), and the `expect` is never empty:
/// state, recorded verdict and the probability of the deciding question, which is what proves that the
/// `batch` `replay` reaches the decision in full.
fn render(id: &str, c: &Candidate<'_>) -> Result<String, String> {
    let e = c.entry;
    let recorded = e.verdict.as_deref().unwrap_or_default();
    let replay = RISK_QUESTIONS
        .iter()
        .map(|q| {
            let p = c.risk[*q];
            (format!("{RISK_NODE}.{q}"), Replay { answer: p, confidence: binary_confidence(p) })
        })
        .collect();
    let fixture = OutFixture {
        id: id.to_string(),
        version: 1,
        description: format!(
            "Importado del registro sombra ({}): {} — {} ({recorded})",
            e.ts,
            e.tool_name,
            e.reason.as_deref().unwrap_or("sin motivo")
        ),
        modes: vec!["offline"],
        input: c.input.clone(),
        replay,
        expect: vec![
            OutExpect { cel: "status == 'ok'".into(), why: "El flujo local debe responder sin romperse".into() },
            OutExpect {
                cel: format!("decision.verdict == '{recorded}'"),
                why: format!("El veredicto grabado ({recorded}) debe reproducirse"),
            },
            OutExpect {
                cel: format!("decision.probabilities.{} == {:?}", c.question, c.risk[c.question]),
                why: format!(
                    "El replay del batch debe llegar entero a la decisión: {} decide ({})",
                    c.question, c.risk[c.question]
                ),
            },
        ],
    };
    serde_yaml::to_string(&fixture).map_err(|e| e.to_string())
}

/// The bench needs its `lab.yaml`: without it the laboratory does not load. Written only if missing.
fn ensure_lab_yaml(root: &Path, tally: &mut Tally) -> Result<(), String> {
    let path = root.join("lab.yaml");
    if path.is_file() {
        return Ok(());
    }
    std::fs::write(&path, LAB_YAML).map_err(|e| format!("{}: {e}", path.display()))?;
    tally.lab_yaml_written = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevalan_hub::lab::assert::assert;
    use jevalan_hub::lab::fixture::parse_fixture;
    use jevalan_hub::shadow::parse_line;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// Directorio temporal, borrado al soltarlo.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("jevalan-import-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A log line as the hook writes it.
    fn line(v: Value) -> Entry {
        parse_line(&v.to_string()).expect("la línea del registro debe interpretarse")
    }

    fn probabilities(p: [f64; 4]) -> Value {
        json!({
            "destructive": p[0], "exfiltration": p[1], "privilege": p[2], "secret_exposure": p[3],
            "off_task": 0.01, "violates_constraint": 0.01, "unresolved_choice": 0.01, "authorized": 0.99,
        })
    }

    /// The case that survives all the filters: `git status` allowed, no risk.
    fn allowed(ts: &str, command: &str) -> Entry {
        line(json!({
            "ts": ts, "session_id": "s1", "tool_name": "Bash", "tool_input": { "command": command },
            "status": "ok", "verdict": "allow", "source": "model", "reason": "todos los riesgos < 0.55",
            "probabilities": probabilities([0.1, 0.1, 0.1, 0.1]), "latency_ms": 10, "error": null,
        }))
    }

    fn opts(tmp: &Tmp) -> Opts {
        Opts { lab_dir: tmp.0.clone(), max: None }
    }

    fn fixtures_of(tmp: &Tmp) -> Vec<(String, String)> {
        let dir = tmp.0.join("fixtures").join(CAPABILITY);
        let mut out: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| {
                let stem = e.path().file_stem().unwrap().to_string_lossy().to_string();
                (stem, std::fs::read_to_string(e.path()).unwrap())
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn cada_filtro_descarta_y_cuenta() {
        let tmp = Tmp::new();
        let entries = vec![
            allowed("2026-10-10T07:00:00.000Z", "git status"),
            allowed("2026-10-10T07:01:00.000Z", "git status"),
            line(json!({
                "ts": "2026-10-10T07:02:00.000Z", "tool_name": "Bash", "tool_input": { "command": "ls" },
                "status": "abstain", "error": null,
            })),
            line(json!({
                "ts": "2026-10-10T07:03:00.000Z", "tool_name": "Bash", "tool_input": { "command": "ls" },
                "status": "error", "verdict": "allow", "error": "sin daemon",
            })),
            line(json!({
                "ts": "2026-10-10T07:04:00.000Z", "tool_name": "Skill", "tool_input": { "skill": "x" },
                "status": "ok", "verdict": "allow", "source": "model", "reason": "todos los riesgos < 0.55",
                "probabilities": probabilities([0.1, 0.1, 0.1, 0.1]),
            })),
            line(json!({
                "ts": "2026-10-10T07:05:00.000Z", "tool_name": "Bash", "tool_input": { "command": "zcat f" },
                "status": "ok", "verdict": "ask", "source": "model",
                "reason": "input too large to evaluate in full — confirm manually",
                "probabilities": probabilities([0.1, 0.1, 0.1, 0.1]),
            })),
            line(json!({
                "ts": "2026-10-10T07:06:00.000Z", "tool_name": "ToolSearch", "tool_input": null,
                "status": "ok", "verdict": "allow", "source": "model", "reason": "x",
                "probabilities": probabilities([0.1, 0.1, 0.1, 0.1]),
            })),
            line(json!({
                "ts": "2026-10-10T07:07:00.000Z", "tool_name": "Bash", "tool_input": { "command": "du -sh ." },
                "status": "ok", "verdict": "allow", "source": "model", "reason": "x",
                "probabilities": json!({ "destructive": 0.1, "exfiltration": 0.1, "privilege": 0.1 }),
            })),
            line(json!({
                "ts": "2026-10-10T07:08:00.000Z", "tool_name": "Bash", "tool_input": { "command": "chmod -R 777 x" },
                "status": "ok", "verdict": "ask", "source": "model", "reason": "riesgo privilege ≥ 0.55",
                "probabilities": probabilities([0.1, 0.1, 0.1, 0.1]),
            })),
            line(json!({
                "ts": "2026-10-10T07:09:00.000Z", "tool_name": "Bash", "tool_input": { "command": "rm -rf /" },
                "status": "ok", "verdict": "allow", "source": "model", "reason": "todos los riesgos < 0.55",
                "probabilities": probabilities([0.1, 0.1, 0.1, 0.1]),
            })),
        ];
        let tally = import(&entries, 2, &opts(&tmp)).unwrap();
        assert_eq!(
            tally,
            Tally {
                ignored: 2,
                no_verdict: 1,
                bad_status: 1,
                unjudged_tool: 1,
                truncated_context: 1,
                no_input: 1,
                missing_probabilities: 1,
                fixed_rule: 1,
                not_reproduced: 1,
                repeated: 1,
                written: 1,
                lab_yaml_written: true,
                ..Tally::default()
            }
        );
        assert_eq!(fixtures_of(&tmp).len(), 1, "solo `git status` llega al banco");
        assert!(tally.render().contains("descartadas por repetidas: 1"));
    }

    #[test]
    fn redondo_el_fixture_vuelve_a_parsearse() {
        let tmp = Tmp::new();
        let entries = vec![line(json!({
            "ts": "2026-10-10T07:00:00.000Z", "session_id": "s1", "tool_name": "Write",
            "tool_input": { "file_path": "E:\\x\\config.yaml", "content": "a: 1" },
            "status": "ok", "verdict": "deny", "source": "model", "reason": "algún riesgo ≥ 0.85",
            "probabilities": probabilities([0.9, 0.2, 0.1, 0.1]), "latency_ms": 30, "error": null,
        }))];
        let tally = import(&entries, 0, &opts(&tmp)).unwrap();
        assert_eq!((tally.written, tally.identical), (1, 0));

        let (stem, yaml) = fixtures_of(&tmp).remove(0);
        let fixture = parse_fixture(&stem, &yaml).unwrap_or_else(|e| panic!("{e}: {yaml}"));
        assert!(stem.starts_with("shadow-write-"));
        assert_eq!(fixture.expect.len(), 3, "el expect nunca queda vacío");
        assert_eq!(fixture.replay.len(), 4, "las cuatro preguntas del batch");
        for q in RISK_QUESTIONS {
            let key = format!("{RISK_NODE}.{q}");
            let p = match q {
                "destructive" => 0.9,
                "exfiltration" => 0.2,
                _ => 0.1,
            };
            assert_eq!(fixture.replay[&key]["answer"].as_f64(), Some(p));
            assert_eq!(fixture.replay[&key]["confidence"].as_f64(), Some(binary_confidence(p)));
        }
        assert!(fixture.expect[2].cel.contains("decision.probabilities.destructive == 0.9"), "{yaml}");
        assert!(fixture.expect.iter().all(|e| !e.why.trim().is_empty()));

        // And the three assertions hold with the decision the `replay` produces: the `batch` decides.
        let decision = json!({
            "status": "ok",
            "decision": { "verdict": "deny", "source": "model", "reason": "algún riesgo ≥ 0.85",
                          "probabilities": { "destructive": 0.9, "exfiltration": 0.2, "privilege": 0.1,
                                             "secret_exposure": 0.1 } },
        });
        assert_eq!(assert(&fixture, &decision), Vec::new());
    }

    #[test]
    fn nunca_pisa_un_fixture() {
        let tmp = Tmp::new();
        let entries = vec![allowed("2026-10-10T07:00:00.000Z", "git status")];
        import(&entries, 0, &opts(&tmp)).unwrap();
        let (stem, yaml) = fixtures_of(&tmp).remove(0);

        // Same again: it counts as written.
        let tally = import(&entries, 0, &opts(&tmp)).unwrap();
        assert_eq!((tally.written, tally.identical, tally.renamed), (0, 1, 0));

        // With other content in its place, `-2` is looked for and the original is not touched.
        let dir = tmp.0.join("fixtures").join(CAPABILITY);
        std::fs::write(dir.join(format!("{stem}.yaml")), "id: otro\n").unwrap();
        let tally = import(&entries, 0, &opts(&tmp)).unwrap();
        assert_eq!((tally.written, tally.identical, tally.renamed), (0, 0, 1));
        assert_eq!(std::fs::read_to_string(dir.join(format!("{stem}.yaml"))).unwrap(), "id: otro\n");
        assert_eq!(fixtures_of(&tmp).len(), 2);
        // Renaming also changes the id inside: the file must be named like its id.
        let renamed = format!("{stem}-2");
        let text = std::fs::read_to_string(dir.join(format!("{renamed}.yaml"))).unwrap();
        assert!(parse_fixture(&renamed, &text).is_ok(), "{text}");
        assert_ne!(yaml, text);

        // With all nine names taken, the case is counted as a failure.
        for n in 2..=MAX_NAMES {
            std::fs::write(dir.join(format!("{stem}-{n}.yaml")), "id: otro\n").unwrap();
        }
        let tally = import(&entries, 0, &opts(&tmp)).unwrap();
        assert_eq!(tally.no_room, 1);
        assert_eq!(tally.fixtures(), 0);
    }

    #[test]
    fn el_id_es_slug_y_lleva_el_hash_de_la_entrada() {
        let input = json!({ "tool_name": "Bash", "tool_input": { "command": "grep -R \"año\" ./src | head -3" } });
        let id = id_of("Bash", &input);
        assert!(id.starts_with("shadow-bash-"), "{id}");
        assert!(id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{id}");
        assert!(id.ends_with(&fnv1a64(&input.to_string())), "{id}");
        assert!(!slug("  ", CRUMB_CHARS).contains(' '));
        // The input decides the id: another input, another id.
        assert_ne!(id, id_of("Bash", &json!({ "tool_name": "Bash", "tool_input": { "command": "ls" } })));
    }

    #[test]
    fn max_se_queda_con_el_mayor_riesgo() {
        let tmp = Tmp::new();
        let entries = vec![
            line(json!({
                "ts": "2026-10-10T07:00:00.000Z", "tool_name": "Bash", "tool_input": { "command": "ls" },
                "status": "ok", "verdict": "ask", "source": "model", "reason": "riesgo destructive ≥ 0.55",
                "probabilities": probabilities([0.7, 0.1, 0.1, 0.1]),
            })),
            allowed("2026-10-10T07:01:00.000Z", "git status"),
        ];
        let tally = import(&entries, 0, &Opts { lab_dir: tmp.0.clone(), max: Some(1) }).unwrap();
        assert_eq!((tally.written, tally.beyond_max), (1, 1));
        let (stem, yaml) = fixtures_of(&tmp).remove(0);
        assert!(stem.contains("-ls-"), "{stem}: {yaml}");
        assert!(yaml.contains("decision.verdict == 'ask'"), "{yaml}");
    }

    #[test]
    fn el_lab_yaml_escrito_no_nombra_ningun_backend_ni_modelo_y_carga() {
        let config = jevalan_hub::lab::config::parse_lab_config(LAB_YAML).unwrap();
        assert!(config.targets.is_empty(), "los targets los elige quien usa el banco: {config:?}");
        for coupled in ["kev", "laya", "backend:", "model:"] {
            assert!(
                !LAB_YAML.contains(coupled),
                "{coupled} en el lab.yaml escrito:
{LAB_YAML}"
            );
        }
    }

    #[test]
    fn sin_casos_no_escribe_nada() {
        let tmp = Tmp::new();
        let tally = import(&[], 0, &opts(&tmp)).unwrap();
        assert_eq!(tally, Tally::default());
        assert!(!tmp.0.join("lab.yaml").exists(), "sin fixtures, ni lab.yaml");
        assert!(tally.render().contains("fixtures escritos: 0"));
    }
}
