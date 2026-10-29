//! Shadow log report: aggregates `shadow.jsonl` and presents it as Markdown.
//! Note that the aggregation is pure; the rest is a thin command-line layer.

use jevalan_hub::lab::metrics::percentile;
use jevalan_hub::shadow::{
    Entry, HIGH_THRESHOLD, NAMES, RISK_THRESHOLD, category, parse, pct, sorted_latencies, suspicious,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const MAX_EXAMPLES: usize = 10;
const INPUT_CHARS: usize = 120;

fn count_by<'a>(
    entries: impl Iterator<Item = &'a Entry>,
    key: impl Fn(&'a Entry) -> String,
) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for e in entries {
        *map.entry(key(e)).or_insert(0) += 1;
    }
    map
}

fn count_table(out: &mut String, title: &str, counts: &BTreeMap<String, usize>, total: usize) {
    let _ = writeln!(out, "| {title} | Llamadas | % |\n|---|---:|---:|");
    let mut rows: Vec<_> = counts.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (k, n) in rows {
        let _ = writeln!(out, "| {k} | {n} | {:.1} |", pct(*n, total));
    }
    out.push('\n');
}

fn ms(v: Option<u64>) -> String {
    v.map_or("-".into(), |v| v.to_string())
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max).collect::<String>() + "…" }
}

/// Markdown report. Without `with_inputs` it includes no `tool_input` content at all.
pub fn report(entries: &[Entry], ignored: usize, with_inputs: bool) -> String {
    let total = entries.len();
    let mut out = String::from("# Informe del registro sombra\n\n");

    // 1. Resumen
    out.push_str("## Resumen\n\n");
    let _ = writeln!(out, "- Llamadas totales: {total}");
    let first = entries.iter().map(|e| e.ts.as_str()).min().unwrap_or("-");
    let last = entries.iter().map(|e| e.ts.as_str()).max().unwrap_or("-");
    let _ = writeln!(out, "- Periodo: {first} a {last}");
    let sessions: std::collections::BTreeSet<_> = entries.iter().filter_map(|e| e.session_id.as_deref()).collect();
    let _ = writeln!(out, "- Sesiones: {}", sessions.len());
    let _ = writeln!(out, "- líneas ignoradas: {ignored}\n");
    count_table(&mut out, "Estado", &count_by(entries.iter(), |e| e.status.clone()), total);
    let verdicts = count_by(entries.iter(), |e| e.verdict.clone().unwrap_or_else(|| "(ninguno)".into()));
    count_table(&mut out, "Veredicto", &verdicts, total);
    let sources = count_by(entries.iter(), |e| e.source.clone().unwrap_or_else(|| "(ninguna)".into()));
    count_table(&mut out, "Fuente", &sources, total);

    // 2. By tool
    out.push_str("## Por herramienta\n\n| Herramienta | Llamadas | % allow | % ask | % deny | p50 ms | p95 ms |\n|---|---:|---:|---:|---:|---:|---:|\n");
    let mut tools: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in entries {
        tools.entry(e.tool_name.as_str()).or_default().push(e);
    }
    let mut tools: Vec<_> = tools.into_iter().collect();
    tools.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    for (name, rows) in tools {
        let n = rows.len();
        let of = |v: &str| pct(rows.iter().filter(|e| e.is(v)).count(), n);
        let lat = sorted_latencies(rows.iter().copied());
        let _ = writeln!(
            out,
            "| {} | {n} | {:.1} | {:.1} | {:.1} | {} | {} |",
            cell(name),
            of("allow"),
            of("ask"),
            of("deny"),
            ms(percentile(&lat, 50.0)),
            ms(percentile(&lat, 95.0))
        );
    }

    // 3. Anatomy of the ask
    out.push_str("\n## Anatomía del ask\n\n");
    let asks: Vec<&Entry> = entries.iter().filter(|e| e.is("ask")).collect();
    let _ = writeln!(out, "Total de ask: {}\n", asks.len());
    let cats = count_by(asks.iter().copied(), category);
    count_table(&mut out, "Categoría", &cats, asks.len());
    let sus: Vec<&Entry> = entries.iter().filter(|e| suspicious(e)).collect();
    let truncated = sus.iter().filter(|e| category(e) == "contexto truncado").count();
    let _ = writeln!(
        out,
        "ask con todos los riesgos < {RISK_THRESHOLD}: {} ({:.1}% de los ask), de ellos {truncated} por contexto truncado\n",
        sus.len(),
        pct(sus.len(), asks.len())
    );

    // 4. Probabilidades
    out.push_str("## Probabilidades\n\n| Nombre | n | media | p50 | p95 | máx | ≥ 0.55 | ≥ 0.85 |\n|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for name in NAMES {
        let mut v: Vec<f64> = entries.iter().filter_map(|e| e.probabilities.get(name).copied()).collect();
        v.sort_by(f64::total_cmp);
        if v.is_empty() {
            let _ = writeln!(out, "| {name} | 0 | - | - | - | - | 0 | 0 |");
            continue;
        }
        let rank = |p: f64| v[(((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize).min(v.len()) - 1];
        let _ = writeln!(
            out,
            "| {name} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {} | {} |",
            v.len(),
            v.iter().sum::<f64>() / v.len() as f64,
            rank(50.0),
            rank(95.0),
            v[v.len() - 1],
            v.iter().filter(|p| **p >= RISK_THRESHOLD).count(),
            v.iter().filter(|p| **p >= HIGH_THRESHOLD).count()
        );
    }

    // 5. Latencia y errores
    out.push_str("\n## Latencia y errores\n\n");
    let lat = sorted_latencies(entries.iter());
    let _ = writeln!(
        out,
        "Latencia (ok y abstain): n = {}, p50 = {} ms, p95 = {} ms, máx = {} ms\n",
        lat.len(),
        ms(percentile(&lat, 50.0)),
        ms(percentile(&lat, 95.0)),
        ms(lat.last().copied())
    );
    let errors = count_by(entries.iter().filter(|e| e.status == "error"), |e| {
        e.error.clone().unwrap_or_else(|| "(sin mensaje)".into())
    });
    if errors.is_empty() {
        out.push_str("Sin errores.\n");
    } else {
        let n_err = errors.values().sum();
        count_table(&mut out, "Error", &errors, n_err);
    }

    // 6. Ejemplos
    out.push_str("\n## Ejemplos de ask sospechosos y deny\n\n");
    let examples: Vec<&Entry> = entries.iter().filter(|e| suspicious(e) || e.is("deny")).take(MAX_EXAMPLES).collect();
    if examples.is_empty() {
        out.push_str("Ninguno.\n");
    } else {
        out.push_str("| ts | Herramienta | Veredicto | Categoría | Riesgo máx |");
        out.push_str(if with_inputs { " Entrada |\n|---|---|---|---|---|---|\n" } else { "\n|---|---|---|---|---|\n" });
        for e in examples {
            let risk = e.max_risk().map_or("-".into(), |(n, p)| format!("{n} {p:.2}"));
            let _ = write!(
                out,
                "| {} | {} | {} | {} | {risk} |",
                e.ts,
                cell(&e.tool_name),
                e.verdict.as_deref().unwrap_or("-"),
                category(e)
            );
            if with_inputs {
                let _ = write!(out, " {} |", cell(&truncate(e.tool_input.as_deref().unwrap_or(""), INPUT_CHARS)));
            }
            out.push('\n');
        }
    }
    out
}

/// Argumentos de `jevalan shadow report`.
pub struct Opts {
    pub log: Option<PathBuf>,
    pub since: Option<String>,
    pub out: Option<PathBuf>,
    pub with_inputs: bool,
}

/// Keeps the entries since `since` (ISO string; lexical comparison is valid in UTC).
pub fn since_filter(entries: Vec<Entry>, since: &str) -> Vec<Entry> {
    entries.into_iter().filter(|e| e.ts.as_str() >= since).collect()
}

pub fn default_log() -> Option<PathBuf> {
    jevalan_daemon::discovery::data_dir().map(|d| d.join("shadow.jsonl"))
}

pub fn run(opts: Opts) -> ExitCode {
    let Some(path) = opts.log.or_else(default_log) else {
        eprintln!("error: no se encuentra el directorio de datos; indique --log");
        return ExitCode::FAILURE;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: no se puede leer {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let (mut entries, ignored) = parse(&text);
    if let Some(since) = &opts.since {
        entries = since_filter(entries, since);
    }
    let md = report(&entries, ignored, opts.with_inputs);
    match &opts.out {
        Some(file) => write_out(file, &md),
        None => {
            print!("{md}");
            ExitCode::SUCCESS
        }
    }
}

fn write_out(file: &Path, md: &str) -> ExitCode {
    match std::fs::write(file, md) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: no se puede escribir {}: {e}", file.display());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const TRUNC: &str = "input too large to evaluate in full — confirm manually (all risks below 55% ...)";

    fn line(
        ts: &str,
        tool: &str,
        verdict: Option<&str>,
        source: Option<&str>,
        reason: Option<&str>,
        p: Value,
        ms: u64,
    ) -> String {
        let status = if verdict.is_some() { "ok" } else { "error" };
        json!({ "ts": ts, "session_id": "s1", "tool_name": tool, "tool_input": { "command": "echo secreto-visible" },
            "status": status, "verdict": verdict, "source": source, "reason": reason, "probabilities": p,
            "latency_ms": ms, "error": if verdict.is_some() { Value::Null } else { json!("sin daemon") } })
        .to_string()
    }

    fn sample() -> String {
        let low = json!({ "destructive": 0.1, "authorized": 0.9 });
        let high = json!({ "destructive": 0.9, "exfiltration": 0.2, "authorized": 0.1 });
        [
            line("2026-10-10T07:00:00.000Z", "Bash", Some("ask"), Some("model"), Some(TRUNC), low.clone(), 100),
            line("2026-10-10T07:01:00.000Z", "Bash", Some("ask"), Some("static-rule"), Some("regla"), json!(null), 1),
            line("2026-10-10T07:02:00.000Z", "Write", Some("deny"), Some("model"), Some("peligro"), high.clone(), 300),
            line("2026-10-10T07:03:00.000Z", "Bash", Some("ask"), Some("model"), Some("dudoso"), high, 200),
            line("2026-10-10T07:04:00.000Z", "Read", Some("allow"), Some("model"), Some("ok"), low, 50),
            line("2026-10-10T07:05:00.000Z", "Read", None, None, None, json!(null), 0),
            "esto no es json".into(),
        ]
        .join("\n")
    }

    #[test]
    fn parses_and_skips_malformed_lines() {
        let (entries, ignored) = parse(&sample());
        assert_eq!((entries.len(), ignored), (6, 1));
    }

    #[test]
    fn classifies_ask_reasons() {
        let (e, _) = parse(&sample());
        assert_eq!(category(&e[0]), "contexto truncado");
        assert_eq!(category(&e[1]), "regla fija");
        assert_eq!(category(&e[3]), "riesgo: destructive");
        assert!(suspicious(&e[0]) && !suspicious(&e[1]) && !suspicious(&e[3]));
    }

    #[test]
    fn report_aggregates_without_leaking_inputs() {
        let (e, ignored) = parse(&sample());
        let md = report(&e, ignored, false);
        assert!(md.contains("Llamadas totales: 6"), "{md}");
        assert!(md.contains("líneas ignoradas: 1"));
        assert!(md.contains("| contexto truncado | 1 |"));
        assert!(md.contains("| regla fija | 1 |"));
        assert!(
            md.contains("ask con todos los riesgos < 0.55: 1 (33.3% de los ask), de ellos 1 por contexto truncado")
        );
        assert!(md.contains("| sin daemon | 1 |"));
        assert!(md.contains("| Write | 1 | 0.0 | 0.0 | 100.0 |"), "{md}");
        assert!(md.contains("| destructive | 4 | 0.500 |"), "{md}");
        assert!(!md.contains("secreto-visible"));
        let with = report(&e, ignored, true);
        assert!(with.contains("secreto-visible"));
    }

    #[test]
    fn since_filters_by_timestamp() {
        let (e, _) = parse(&sample());
        assert_eq!(since_filter(e.clone(), "2026-10-10T07:03").len(), 3);
        assert_eq!(since_filter(e, "2026-10-11").len(), 0);
    }

    #[test]
    fn empty_log_reports_zero() {
        let md = report(&[], 0, false);
        assert!(md.contains("Llamadas totales: 0") && md.contains("Sin errores."));
    }
}
