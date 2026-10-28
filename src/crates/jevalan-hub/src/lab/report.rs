use crate::lab::metrics::{Bin, Cost, Summary};
use std::fmt::Write;

fn ms(v: Option<u64>) -> String {
    v.map_or("-".into(), |m| format!("{m}ms"))
}

fn cost(c: Cost) -> String {
    match c {
        Cost::Known(usd) => format!("${usd:.4}"),
        Cost::Unknown => "unknown".into(),
        Cost::Local => "$0 (local)".into(),
    }
}

const BINS: [&str; 5] = ["[0.0, 0.2)", "[0.2, 0.4)", "[0.4, 0.6)", "[0.6, 0.8)", "[0.8, 1.0]"];

/// Sección Markdown de una capacidad: tabla comparada, fiabilidad y notas.
pub fn render_capability(
    capability: &str,
    repeat: u32,
    rows: &[(String, Summary)],
    bins: &[(String, [Bin; 5])],
    notes: &[String],
) -> String {
    let mut out = format!("## {capability}\n\n");
    if repeat == 1 {
        out.push_str("> Aviso: repeat = 1; no hay base para estabilidad ni para conclusiones.\n\n");
    }
    out.push_str("| target | pasa | estable | abstain | error | p50 | p95 | coste | Brier |\n");
    out.push_str("|---|---|---|---|---|---|---|---|---|\n");
    for (t, s) in rows {
        let brier = s.brier.map_or("n/a".into(), |b| format!("{b:.3}"));
        let _ = writeln!(
            out,
            "| {t} | {}/{} | {}/{} | {:.0}% | {:.0}% | {} | {} | {} | {brier} |",
            s.passed,
            s.runs,
            s.stable,
            s.fixtures,
            s.abstain_pct,
            s.error_pct,
            ms(s.p50),
            ms(s.p95),
            cost(s.cost)
        );
    }
    for (t, b) in bins {
        let _ = write!(out, "\n### Fiabilidad · {t}\n\n| confianza | n | acierto |\n|---|---|---|\n");
        for (label, bin) in BINS.iter().zip(b) {
            let rate =
                if bin.n == 0 { "-".to_string() } else { format!("{:.0}%", bin.hits as f64 * 100.0 / bin.n as f64) };
            let _ = writeln!(out, "| {label} | {} | {rate} |", bin.n);
        }
    }
    if !notes.is_empty() {
        out.push_str("\n### Detalles\n\n");
        for n in notes {
            let _ = writeln!(out, "- {n}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab::metrics::{Bin, Cost, Summary};

    fn row(cost: Cost) -> Summary {
        Summary {
            runs: 4,
            passed: 3,
            skipped: 0,
            stable: 1,
            fixtures: 2,
            abstain_pct: 25.0,
            error_pct: 0.0,
            p50: Some(120),
            p95: Some(400),
            cost,
            brier: Some(0.0832),
        }
    }

    #[test]
    fn table_lists_each_target() {
        let out = render_capability(
            "tool.risk",
            2,
            &[("jev".into(), row(Cost::Known(0.0123))), ("nimble".into(), row(Cost::Local))],
            &[],
            &[],
        );
        assert!(out.contains("## tool.risk"));
        assert!(out.contains("| jev | 3/4 | 1/2 | 25% | 0% | 120ms | 400ms | $0.0123 | 0.083 |"), "{out}");
        assert!(out.contains("$0 (local)"));
    }

    #[test]
    fn unknown_cost_and_missing_brier_are_explicit() {
        let mut r = row(Cost::Unknown);
        r.brier = None;
        let out = render_capability("x.y", 2, &[("t".into(), r)], &[], &[]);
        assert!(out.contains("unknown") && out.contains("n/a"), "{out}");
    }

    #[test]
    fn empty_known_cost_is_positive_zero() {
        let s = crate::lab::metrics::summarize(&[]);
        let out = render_capability("x.y", 2, &[("t".into(), s)], &[], &[]);
        assert!(!out.contains("$-0"), "{out}");
    }

    #[test]
    fn single_repeat_warns() {
        let out = render_capability("x.y", 1, &[("t".into(), row(Cost::Local))], &[], &[]);
        assert!(out.contains("repeat = 1"), "{out}");
    }

    #[test]
    fn bins_and_notes_are_rendered() {
        let bins = [Bin { n: 2, hits: 1 }, Bin::default(), Bin::default(), Bin::default(), Bin { n: 1, hits: 1 }];
        let out = render_capability(
            "x.y",
            2,
            &[("t".into(), row(Cost::Local))],
            &[("t".into(), bins)],
            &["rm-root falla: nunca allow".into()],
        );
        assert!(out.contains("[0.0, 0.2)") && out.contains("2 | 50%") && out.contains("rm-root falla"), "{out}");
    }
}
