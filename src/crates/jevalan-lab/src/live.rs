//! `record` y `live`: la matriz fixtures × targets × repeticiones contra backends reales, con presupuesto.

use crate::budget::{Tally, longest_path};
use crate::load::input_hash;
use crate::run::{Filter, finish_line, request};
use crate::{Case, Lab, ResultLine, RunOutcome, write};
use jevalan_daemon::hub::execute::execute;
use jevalan_daemon::hub::transport::{Recordings, Transport};
use jevalan_hub::capability::Provider;
use jevalan_hub::lab::config::Target;
use jevalan_hub::lab::fixture::FixtureMode;
use jevalan_hub::lab::metrics::{Cost, Sample, cost_of, reliability, summarize};
use jevalan_hub::lab::report::render_capability;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Llama en vivo y escribe cassettes.
    Record,
    /// Llama en vivo, repite y mide.
    Live,
}

impl Mode {
    fn as_str(self) -> &'static str {
        if self == Mode::Record { "record" } else { "live" }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LiveOpts {
    /// Solo este target (las capacidades `exec`/`http` solo lo admiten si se llama como su proveedor).
    pub target: Option<String>,
    /// Sustituye al `repeat` de `lab.yaml`; `record` ignora las repeticiones.
    pub repeat: Option<u32>,
    /// Sin pedir confirmación.
    pub yes: bool,
    /// `record`: sobrescribe la cassette aunque la grabación traiga fallos.
    pub force: bool,
}

/// Una ejecución prevista de la matriz.
struct Job<'a> {
    case: &'a Case,
    target: String,
    /// `None` en `exec`/`http`: el especialista se evalúa tal cual.
    spec: Option<&'a Target>,
    repeat: u32,
    calls: u64,
}

/// Lo que dejó una ejecución.
struct Done {
    line: ResultLine,
    cost: Cost,
    replies: Recordings,
    /// Llamadas a backends que se hicieron de verdad.
    made: u32,
}

impl Lab {
    /// Ejecuta la matriz; `Err` si no hay presupuesto, lo supera o no se confirma, antes de la primera llamada.
    pub async fn live(
        &self,
        mode: Mode,
        filter: &Filter,
        opts: &LiveOpts,
        confirm: &mut (dyn FnMut(&str) -> bool + Send),
    ) -> Result<RunOutcome, String> {
        let budget = self.config.budget.ok_or("lab.yaml necesita `budget` para record y live")?;
        if let Some(t) = opts.target.as_deref()
            && !self.config.targets.contains_key(t)
            && !self.cases.iter().any(|c| c.cap.provider_id == t)
        {
            return Err(format!("el target {t:?} no existe (hay: {})", self.order.join(", ")));
        }
        if let Some(t) = opts.target.as_deref().filter(|t| !self.has_backend(t)) {
            return Err(format!("target {t}: backend no configurado en esta config"));
        }
        let repeat = if mode == Mode::Record { 1 } else { opts.repeat.unwrap_or(self.config.repeat) };
        if repeat == 0 {
            return Err("repeat debe ser al menos 1".into());
        }
        let (jobs, mut results) = self.plan(filter, opts, repeat);
        let planned: u64 = jobs.iter().map(|j| j.calls).sum();
        if planned > budget.max_calls {
            return Err(format!(
                "presupuesto insuficiente: {planned} llamadas previstas y max_calls es {}; no se hace ninguna",
                budget.max_calls
            ));
        }
        let mut message = format!(
            "{} ejecuciones, {planned} llamadas previstas de {} permitidas (coste máximo ${:.2}).",
            jobs.len(),
            budget.max_calls,
            budget.max_cost_usd
        );
        let unpriced: BTreeSet<&str> = jobs
            .iter()
            .filter(|j| !self.priced(j) && !j.spec.is_some_and(|t| t.local))
            .map(|j| j.target.as_str())
            .collect();
        if !unpriced.is_empty() {
            let names = unpriced.into_iter().collect::<Vec<_>>().join(", ");
            let _ = write!(message, "\nsin precio para {names}: el presupuesto de coste no los controla");
        }
        if !opts.yes && !confirm(&message) {
            return Err("cancelado: no se confirmó".into());
        }

        let started = write::Stamp::now();
        let (mut tally, mut stopped) = (Tally::new(budget), false);
        let mut costs: Vec<Cost> = vec![Cost::Unknown; results.len()];
        let mut reported = BTreeMap::new();
        let (mut problems, mut banners) = (Vec::new(), Vec::new());
        for job in &jobs {
            if tally.exhausted(job.calls, self.priced(job)) {
                stopped = true;
                break;
            }
            let Done { line, cost, replies, made } = self.run_job(job).await;
            tally.record(job.calls, made, if let Cost::Known(usd) = cost { Some(usd) } else { None });
            if let Some(model) = replies.values().find_map(|r| r["body"]["model"].as_str()) {
                reported.entry(job.target.clone()).or_insert_with(|| model.to_string());
            }
            if mode == Mode::Record {
                match self.write_cassette(job, &replies, &line, &started.utc, opts.force) {
                    Ok(Some(why)) => banners.push(format!(
                        "cassette no actualizada: {}/{}/{}: {why} (usa --force para sobrescribirla)",
                        job.case.cap.name, job.case.fixture.id, job.target
                    )),
                    Ok(None) => {}
                    Err(e) => problems.push(e),
                }
            }
            results.push(line);
            costs.push(cost);
        }

        if stopped {
            banners.insert(0, "Detenido: presupuesto agotado (stopped: budget); los resultados son parciales.".into());
        }
        let report = self.report(mode, repeat, &results, &costs, &tally, &banners);
        let failed = results.iter().any(|r| r.skipped.is_none() && !r.passed);
        let used = write::Consumed { calls: tally.calls, cost_usd: tally.cost, stopped, repeat, reported };
        let run_dir = match write::write_run(self, mode.as_str(), &started, &results, &report, &used) {
            Ok(dir) => Some(dir),
            Err(e) => {
                problems.push(e);
                None
            }
        };
        let write_error = (!problems.is_empty()).then(|| problems.join("; "));
        Ok(RunOutcome { results, failed, report, run_dir, write_error, stopped })
    }

    /// La matriz en orden de fixture, target y repetición, más las líneas ya `skipped`.
    fn plan<'a>(&'a self, filter: &Filter, opts: &LiveOpts, repeat: u32) -> (Vec<Job<'a>>, Vec<ResultLine>) {
        let (mut jobs, mut skipped) = (Vec::new(), Vec::new());
        let admitted = self.cases.iter().filter(|c| {
            c.fixture.runs_in(FixtureMode::Live)
                && filter.capability.as_deref().is_none_or(|n| n == c.cap.name)
                && filter.fixture.as_deref().is_none_or(|f| f == c.fixture.id)
        });
        for case in admitted {
            let mut skip = |target: &str, why: String| {
                let mut line = ResultLine::blank(case, target, 1, "live");
                line.skipped = Some(why);
                skipped.push(line);
            };
            match &case.cap.provider {
                Provider::Flow { flow } => {
                    let calls = self.state.flows.get(flow).map(|f| longest_path(&f));
                    let wanted =
                        |n: &&String| opts.target.as_deref().is_none_or(|t| t == n.as_str()) && self.has_backend(n);
                    for name in self.order.iter().filter(wanted) {
                        let Some(calls) = calls else {
                            skip(name, format!("el flujo {flow} no está cargado"));
                            continue;
                        };
                        let spec = self.config.targets.get(name);
                        jobs.extend((1..=repeat).map(|r| Job { case, target: name.clone(), spec, repeat: r, calls }));
                    }
                }
                other => {
                    let id = &case.cap.provider_id;
                    if opts.target.as_deref().is_some_and(|t| t != id) {
                        continue;
                    }
                    if matches!(other, Provider::Exec { .. }) && !self.state.cfg.hub.allow_exec {
                        skip(id, "los proveedores exec están desactivados (hub.allow_exec)".into());
                        continue;
                    }
                    jobs.extend((1..=repeat).map(|r| Job {
                        case,
                        target: id.clone(),
                        spec: None,
                        repeat: r,
                        calls: 1,
                    }));
                }
            }
        }
        (jobs, skipped)
    }

    async fn run_job(&self, job: &Job<'_>) -> Done {
        let case = job.case;
        let mut line = ResultLine::blank(case, &job.target, job.repeat, "live");
        let req = match request(case) {
            Ok(req) => req,
            Err(e) => {
                line.skipped = Some(e);
                return Done { line, cost: Cost::Unknown, replies: Recordings::new(), made: 0 };
            }
        };
        let flow = job.spec.and_then(|t| self.target_flow(&case.cap, t));
        // `Record` actúa como `Live` y además deja ver lo que contestó el backend (el modelo, p. ej.).
        let tape = Arc::new(Mutex::new(Recordings::new()));
        let done = execute(&self.state, &req, Some(&case.cap), flow, &Transport::Record(tape.clone())).await;
        finish_line(&mut line, &case.fixture, &done);
        let m = &done.measure;
        let cost = match job.spec {
            Some(t) if !t.local && m.reported_cost_usd.is_some() => {
                Cost::Known(m.reported_cost_usd.unwrap_or_default())
            }
            Some(t) => {
                let price = self.config.prices.get(&t.model);
                cost_of(m.backend_calls, m.unmetered_calls, line.input_tokens, line.output_tokens, price, t.local)
            }
            None => Cost::Unknown,
        };
        line.cost_usd = match cost {
            Cost::Known(usd) => Some(usd),
            Cost::Local => Some(0.0),
            Cost::Unknown => None,
        };
        let replies = tape.lock().unwrap_or_else(PoisonError::into_inner).clone();
        Done { line, cost, replies, made: m.backend_calls }
    }

    /// ¿Tiene precio el modelo del target? Solo entonces el presupuesto de coste lo controla.
    fn priced(&self, job: &Job<'_>) -> bool {
        job.spec.is_some_and(|t| !t.local && self.config.prices.contains_key(&t.model))
    }

    /// `cassettes/<cap>/<fixture>/<target>.json`: solo `status` y cuerpo; ni cabeceras, ni claves, ni petición.
    /// Devuelve el motivo si respeta la cassette anterior en vez de sustituirla por una grabación con fallos.
    fn write_cassette(
        &self,
        job: &Job<'_>,
        replies: &Recordings,
        line: &ResultLine,
        at: &str,
        force: bool,
    ) -> Result<Option<String>, String> {
        if replies.is_empty() {
            return Ok(None);
        }
        if !force && let Some(why) = spoiled(replies, &line.status) {
            return Ok(Some(why));
        }
        let fx = &job.case.fixture;
        let cassette: Value = json!({
            "recorded_at": at,
            "fixture_version": fx.version,
            "input_hash": input_hash(fx),
            "target": job.target,
            "backend": job.spec.map(|t| &t.backend),
            "model": job.spec.map(|t| &t.model),
            "replies": replies,
        });
        let dir = self.root.join("cassettes").join(&job.case.cap.name).join(&fx.id);
        let path = dir.join(format!("{}.json", job.target));
        let text = serde_json::to_string_pretty(&cassette).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, text))
            .map(|()| None)
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    fn report(
        &self,
        mode: Mode,
        repeat: u32,
        results: &[ResultLine],
        costs: &[Cost],
        tally: &Tally,
        banners: &[String],
    ) -> String {
        let mut out = format!("# Laboratorio · {}\n\n", mode.as_str());
        for banner in banners {
            let _ = writeln!(out, "> {banner}\n");
        }
        let max = self.config.budget.map_or(0, |b| b.max_calls);
        let _ = writeln!(out, "Llamadas (cota): {} de {max}. Coste conocido: ${:.4}.\n", tally.calls, tally.cost);
        type Runs<'a> = Vec<(&'a ResultLine, Cost)>;
        let mut caps: BTreeMap<&str, BTreeMap<&str, Runs>> = BTreeMap::new();
        for (r, c) in results.iter().zip(costs) {
            caps.entry(&r.capability).or_default().entry(&r.target).or_default().push((r, *c));
        }
        for (cap, targets) in caps {
            let (mut rows, mut bins, mut notes) = (Vec::new(), Vec::new(), Vec::new());
            for (target, runs) in &targets {
                let samples: Vec<Sample> = runs.iter().map(|(r, c)| sample(r, *c)).collect();
                rows.push((target.to_string(), summarize(&samples)));
                bins.push((target.to_string(), reliability(&samples)));
                notes.extend(details(target, runs));
            }
            out.push_str(&render_capability(cap, repeat, &rows, &bins, &notes));
            out.push('\n');
        }
        out
    }
}

/// Motivo para no fiarse de una grabación: la ejecución acabó en error o algún backend no contestó 2xx.
fn spoiled(replies: &Recordings, status: &str) -> Option<String> {
    if let Some(code) = replies.values().filter_map(|r| r["status"].as_u64()).find(|c| !(200..300).contains(c)) {
        return Some(format!("el backend respondió {code}"));
    }
    (status == "error").then(|| "la ejecución terminó en error".to_string())
}

fn sample(r: &ResultLine, cost: Cost) -> Sample {
    Sample {
        fixture: r.fixture.clone(),
        skipped: r.skipped.is_some(),
        passed: r.passed,
        status: r.status.clone(),
        elapsed_ms: r.elapsed_ms,
        called: r.called,
        confidence: r.confidence,
        cost,
    }
}

/// Fallos con su porqué, inestables y `skipped` de un target.
fn details(target: &str, runs: &[(&ResultLine, Cost)]) -> Vec<String> {
    let mut notes = BTreeSet::new();
    let mut outcomes: BTreeMap<&str, (bool, bool)> = BTreeMap::new();
    for (r, _) in runs {
        if let Some(why) = &r.skipped {
            notes.insert(format!("`{}` skipped en {target}: {why}", r.fixture));
            continue;
        }
        let o = outcomes.entry(&r.fixture).or_default();
        if r.passed {
            o.0 = true
        } else {
            o.1 = true
        }
        for f in &r.failed_asserts {
            let reason = f.reason.as_deref().map(|x| format!(" ({x})")).unwrap_or_default();
            notes.insert(format!("`{}` falla en {target}: {}{reason}", r.fixture, f.why));
        }
    }
    for (fixture, _) in outcomes.iter().filter(|(_, (p, f))| *p && *f) {
        notes.insert(format!("`{fixture}` es inestable en {target}"));
    }
    notes.into_iter().collect()
}
