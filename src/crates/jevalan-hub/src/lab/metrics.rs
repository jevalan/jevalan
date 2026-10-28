use crate::lab::config::Price;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cost {
    Known(f64),
    Unknown,
    Local,
}

/// Una ejecución de un fixture para un target.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub fixture: String,
    pub skipped: bool,
    pub passed: bool,
    pub status: String,
    pub elapsed_ms: u64,
    /// ¿Llamó a algún backend o especialista? Las que no, solo aplican reglas y no cuentan en la latencia.
    pub called: bool,
    /// Confianza del último nodo Jev, si la hubo.
    pub confidence: Option<f64>,
    pub cost: Cost,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bin {
    pub n: u32,
    pub hits: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub runs: u32,
    pub passed: u32,
    pub skipped: u32,
    pub stable: u32,
    pub fixtures: u32,
    pub abstain_pct: f64,
    pub error_pct: f64,
    pub p50: Option<u64>,
    pub p95: Option<u64>,
    pub cost: Cost,
    pub brier: Option<f64>,
}

/// Coste en USD de una ejecución con `calls` llamadas a backends, `unmetered` de ellas sin `usage`.
/// Sin llamadas no hay gasto (0 exacto); con alguna sin tokens o sin precio es `Unknown`, nunca una estimación.
pub fn cost_of(
    calls: u32,
    unmetered: u32,
    input: Option<u64>,
    output: Option<u64>,
    price: Option<&Price>,
    local: bool,
) -> Cost {
    if local {
        return Cost::Local;
    }
    if calls == 0 {
        return Cost::Known(0.0);
    }
    match (unmetered, input, output, price) {
        (0, Some(i), Some(o), Some(p)) => {
            Cost::Known(i as f64 / 1e6 * p.input_per_mtok + o as f64 / 1e6 * p.output_per_mtok)
        }
        _ => Cost::Unknown,
    }
}

/// Percentil por el método del rango más cercano sobre valores ordenados.
pub fn percentile(sorted: &[u64], p: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted.get(rank.min(sorted.len()) - 1).copied()
}

fn live(samples: &[Sample]) -> impl Iterator<Item = &Sample> {
    samples.iter().filter(|s| !s.skipped)
}

/// Una confianza NaN no es una confianza.
fn confidence(s: &Sample) -> Option<f64> {
    s.confidence.filter(|c| !c.is_nan())
}

pub fn brier(samples: &[Sample]) -> Option<f64> {
    let scored: Vec<f64> =
        live(samples).filter_map(|s| confidence(s).map(|c| (c - if s.passed { 1.0 } else { 0.0 }).powi(2))).collect();
    (!scored.is_empty()).then(|| scored.iter().sum::<f64>() / scored.len() as f64)
}

/// Cinco tramos `[0,0.2) … [0.8,1]` con ejecuciones y aciertos.
pub fn reliability(samples: &[Sample]) -> [Bin; 5] {
    let mut bins = [Bin::default(); 5];
    for s in live(samples) {
        if let Some(c) = confidence(s) {
            let i = ((c.clamp(0.0, 1.0) * 5.0) as usize).min(4);
            bins[i].n += 1;
            bins[i].hits += u32::from(s.passed);
        }
    }
    bins
}

/// (fixtures con el mismo resultado en todas sus repeticiones, fixtures).
pub fn stability(samples: &[Sample]) -> (u32, u32) {
    let mut by: BTreeMap<&str, (bool, bool)> = BTreeMap::new();
    for s in live(samples) {
        let e = by.entry(s.fixture.as_str()).or_insert((false, false));
        if s.passed { e.0 = true } else { e.1 = true }
    }
    let stable = by.values().filter(|(p, f)| !(*p && *f)).count() as u32;
    (stable, by.len() as u32)
}

pub fn summarize(samples: &[Sample]) -> Summary {
    let runs: Vec<&Sample> = live(samples).collect();
    let n = runs.len() as u32;
    let pct = |status: &str| {
        if n == 0 { 0.0 } else { runs.iter().filter(|s| s.status == status).count() as f64 * 100.0 / n as f64 }
    };
    let mut ms: Vec<u64> = runs.iter().filter(|s| s.called).map(|s| s.elapsed_ms).collect();
    ms.sort_unstable();
    let cost = if runs.iter().all(|s| s.cost == Cost::Local) && n > 0 {
        Cost::Local
    } else if runs.iter().any(|s| s.cost == Cost::Unknown) {
        Cost::Unknown
    } else {
        Cost::Known(runs.iter().fold(0.0, |acc, s| if let Cost::Known(c) = s.cost { acc + c } else { acc }))
    };
    let (stable, fixtures) = stability(samples);
    Summary {
        runs: n,
        passed: runs.iter().filter(|s| s.passed).count() as u32,
        skipped: samples.iter().filter(|s| s.skipped).count() as u32,
        stable,
        fixtures,
        abstain_pct: pct("abstain"),
        error_pct: pct("error"),
        p50: percentile(&ms, 50.0),
        p95: percentile(&ms, 95.0),
        cost,
        brier: brier(samples),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab::config::Price;

    fn s(fixture: &str, passed: bool, status: &str, ms: u64, conf: Option<f64>, cost: Cost) -> Sample {
        Sample {
            fixture: fixture.into(),
            skipped: false,
            passed,
            status: status.into(),
            elapsed_ms: ms,
            called: true,
            confidence: conf,
            cost,
        }
    }

    #[test]
    fn latency_ignores_runs_without_calls() {
        let v = [
            Sample { called: false, ..s("rules", true, "ok", 0, None, Cost::Local) },
            Sample { called: false, ..s("rules2", true, "ok", 1, None, Cost::Local) },
            s("a", true, "ok", 400, None, Cost::Local),
            s("b", true, "ok", 800, None, Cost::Local),
        ];
        let sum = summarize(&v);
        assert_eq!((sum.p50, sum.p95), (Some(400), Some(800)));
        let none = summarize(&[Sample { called: false, ..s("r", true, "ok", 0, None, Cost::Local) }]);
        assert_eq!((none.p50, none.p95), (None, None));
    }

    #[test]
    fn nearest_rank_percentiles() {
        let v = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
        assert_eq!(percentile(&v, 50.0), Some(50));
        assert_eq!(percentile(&v, 95.0), Some(100));
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(percentile(&[7], 95.0), Some(7));
    }

    #[test]
    fn brier_of_known_cases() {
        let perfect = [s("a", true, "ok", 1, Some(1.0), Cost::Local), s("b", false, "ok", 1, Some(0.0), Cost::Local)];
        assert_eq!(brier(&perfect), Some(0.0));
        let half = [s("a", true, "ok", 1, Some(0.5), Cost::Local)];
        assert_eq!(brier(&half), Some(0.25));
        assert_eq!(brier(&[s("a", true, "ok", 1, None, Cost::Local)]), None);
    }

    #[test]
    fn reliability_bins() {
        let b = reliability(&[
            s("a", true, "ok", 1, Some(0.1), Cost::Local),
            s("b", false, "ok", 1, Some(0.15), Cost::Local),
            s("c", true, "ok", 1, Some(1.0), Cost::Local),
        ]);
        assert_eq!((b[0].n, b[0].hits), (2, 1));
        assert_eq!((b[4].n, b[4].hits), (1, 1));
    }

    #[test]
    fn stability_counts_fixtures_with_one_outcome() {
        let v = [
            s("a", true, "ok", 1, None, Cost::Local),
            s("a", true, "ok", 1, None, Cost::Local),
            s("b", true, "ok", 1, None, Cost::Local),
            s("b", false, "ok", 1, None, Cost::Local),
        ];
        assert_eq!(stability(&v), (1, 2));
    }

    #[test]
    fn cost_from_tokens_and_price() {
        let p = Price { input_per_mtok: 1.0, output_per_mtok: 2.0 };
        assert_eq!(cost_of(1, 0, Some(1_000_000), Some(500_000), Some(&p), false), Cost::Known(2.0));
        assert_eq!(cost_of(1, 0, Some(1), Some(1), None, true), Cost::Local);
    }

    #[test]
    fn cost_without_price_is_unknown() {
        assert_eq!(cost_of(1, 0, Some(10), Some(10), None, false), Cost::Unknown);
        let p = Price { input_per_mtok: 1.0, output_per_mtok: 1.0 };
        assert_eq!(cost_of(1, 1, None, None, Some(&p), false), Cost::Unknown);
    }

    #[test]
    fn no_backend_calls_cost_exactly_zero() {
        assert_eq!(cost_of(0, 0, None, None, None, false), Cost::Known(0.0));
    }

    #[test]
    fn a_response_without_usage_makes_the_cost_unknown() {
        let p = Price { input_per_mtok: 1.0, output_per_mtok: 1.0 };
        // Sumar solo las llamadas medidas daría un coste parcial: se declara desconocido.
        assert_eq!(cost_of(2, 1, Some(10), Some(10), Some(&p), false), Cost::Unknown);
    }

    #[test]
    fn nan_confidence_counts_as_none() {
        let v = [s("a", true, "ok", 1, Some(f64::NAN), Cost::Local), s("b", true, "ok", 1, Some(1.0), Cost::Local)];
        assert_eq!(brier(&v), Some(0.0));
        let bins = reliability(&v);
        assert_eq!(bins.iter().map(|b| b.n).sum::<u32>(), 1);
        assert_eq!(brier(&v[..1]), None);
    }

    #[test]
    fn summary_aggregates_a_target() {
        let v = [
            s("a", true, "ok", 100, Some(0.9), Cost::Known(0.01)),
            s("a", true, "ok", 300, Some(0.8), Cost::Known(0.01)),
            s("b", false, "abstain", 200, None, Cost::Unknown),
            Sample { skipped: true, ..s("c", false, "ok", 0, None, Cost::Unknown) },
        ];
        let sum = summarize(&v);
        assert_eq!((sum.runs, sum.passed, sum.skipped), (3, 2, 1));
        assert_eq!((sum.stable, sum.fixtures), (2, 2));
        assert_eq!(sum.abstain_pct, 100.0 / 3.0);
        assert_eq!((sum.p50, sum.p95), (Some(200), Some(300)));
        assert_eq!(sum.cost, Cost::Unknown);
    }
}
