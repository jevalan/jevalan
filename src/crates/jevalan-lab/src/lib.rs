//! Jev laboratory: loads `config/lab/`, runs the fixtures and writes `.local/runs/`.

mod budget;
pub mod import;
mod live;
mod load;
mod run;
mod write;

use jevalan_daemon::AppState;
use jevalan_flow::{Flow, NodeType};
use jevalan_hub::capability::{Capability, Provider};
use jevalan_hub::lab::config::{LabConfig, Target};
use jevalan_hub::lab::fixture::{Fixture, check_fixture};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

pub use budget::ask;
pub use live::{LiveOpts, Mode};
pub use run::Filter;

pub(crate) struct Case {
    pub cap: Arc<Capability>,
    pub fixture: Fixture,
}

pub struct Lab {
    pub(crate) state: AppState,
    pub(crate) root: PathBuf,
    /// By default, `.local/runs/` from the project directory.
    pub runs_dir: PathBuf,
    pub(crate) yaml: String,
    pub(crate) config: LabConfig,
    /// Targets in the order of `lab.yaml`.
    pub(crate) order: Vec<String>,
    pub(crate) cases: Vec<Case>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FailedAssert {
    pub why: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A line of `results.jsonl` (spec §5.1).
#[derive(Debug, Clone, Serialize)]
pub struct ResultLine {
    pub fixture: String,
    pub fixture_version: u32,
    pub capability: String,
    pub target: String,
    pub repeat: u32,
    pub source: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Error text when the run ends in `error` (e.g. "node not found").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub passed: bool,
    pub failed_asserts: Vec<FailedAssert>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    pub elapsed_ms: u64,
    /// Did it make any call to a backend or specialist? It is not dumped to `results.jsonl`.
    #[serde(skip)]
    pub called: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// `null` = desconocido.
    pub cost_usd: Option<f64>,
}

pub struct RunOutcome {
    pub results: Vec<ResultLine>,
    pub failed: bool,
    pub report: String,
    pub run_dir: Option<PathBuf>,
    pub write_error: Option<String>,
    /// The budget cut the run short.
    pub stopped: bool,
}

impl ResultLine {
    /// Empty line of a fixture for a target; the run fills it in.
    pub(crate) fn blank(case: &Case, target: &str, repeat: u32, source: &str) -> Self {
        Self {
            fixture: case.fixture.id.clone(),
            fixture_version: case.fixture.version,
            capability: case.cap.name.clone(),
            target: target.into(),
            repeat,
            source: source.into(),
            status: "skipped".into(),
            code: None,
            error_message: None,
            passed: false,
            failed_asserts: Vec::new(),
            skipped: None,
            elapsed_ms: 0,
            called: false,
            confidence: None,
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
        }
    }
}

impl Lab {
    pub fn fixture_count(&self) -> usize {
        self.cases.len()
    }

    /// Jev questions (non-`rule`) of the capability's flow, with their type. A standalone node gives one entry with
    /// its name; a `batch` node, one per question, named `"<node>.<question>"`: that is the key with
    /// which the daemon asks for and records each answer (D3).
    pub(crate) fn jev_questions(&self, cap: &Capability) -> Vec<(String, NodeType)> {
        let Provider::Flow { flow } = &cap.provider else { return Vec::new() };
        let Some(flow) = self.state.flows.get(flow) else { return Vec::new() };
        flow.nodes
            .iter()
            .filter(|(_, n)| n.ty != NodeType::Rule)
            .flat_map(|(name, node)| match node.ty {
                NodeType::Batch => {
                    node.questions.iter().map(|(question, q)| (format!("{name}.{question}"), q.ty)).collect::<Vec<_>>()
                }
                ty => vec![(name.clone(), ty)],
            })
            .collect()
    }

    /// The capability's flow with the target's `backend`/`model`; the registry is not touched.
    pub(crate) fn target_flow(&self, cap: &Capability, target: &Target) -> Option<Arc<Flow>> {
        let Provider::Flow { flow } = &cap.provider else { return None };
        let mut flow = (*self.state.flows.get(flow)?).clone();
        for node in flow.nodes.values_mut().filter(|n| n.ty != NodeType::Rule) {
            node.backend = target.backend.clone();
            node.model = target.model.clone();
        }
        Some(Arc::new(flow))
    }

    /// Can the target run? Those not in `lab.yaml` (fixtures of other providers) count as yes.
    pub(crate) fn has_backend(&self, target: &str) -> bool {
        self.config.targets.get(target).is_none_or(|t| self.state.cfg.backends.contains_key(&t.backend))
    }

    /// Targets that are skipped because their backend is not in the daemon's configuration: a warning, not an error.
    pub fn notes(&self) -> Vec<String> {
        let skipped = self.order.iter().filter(|n| !self.has_backend(n));
        skipped.map(|n| format!("target {n}: backend no configurado en esta config")).collect()
    }

    /// Spec §3 validations without network; empty if everything is in order.
    pub fn check(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for Case { cap, fixture } in &self.cases {
            // A `batch` node answers per question: the replay keys are `"<node>.<question>"` (D3).
            let questions = self.jev_questions(cap);
            let is_flow = matches!(cap.provider, Provider::Flow { .. });
            let key_ok = |k: &str| {
                if is_flow { questions.iter().any(|(n, _)| n == k) } else { k == cap.provider_id }
            };
            let at = format!("{}/{}", cap.name, fixture.id);
            problems.extend(check_fixture(fixture, cap, &key_ok).into_iter().map(|p| format!("{at}: {p}")));
            for (node, _) in questions.iter().filter(|(n, _)| fixture.replay.contains_key(n)) {
                let v = &fixture.replay[node];
                if v.get("answer").is_none() || !v["confidence"].is_number() {
                    problems.push(format!("{at}: el replay de {node:?} necesita answer y confidence"));
                } else if !v["confidence"].as_f64().is_some_and(|c| (0.0..=1.0).contains(&c)) {
                    problems.push(format!("{at}: la confidence del replay de {node:?} debe estar en [0, 1]"));
                }
            }
        }
        problems
    }
}
