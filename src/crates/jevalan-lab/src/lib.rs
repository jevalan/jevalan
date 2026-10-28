//! Laboratorio Jev: carga `config/lab/`, ejecuta los fixtures y escribe `.local/runs/`.

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
    /// Por defecto, `.local/runs/` desde el directorio del proyecto.
    pub runs_dir: PathBuf,
    pub(crate) yaml: String,
    pub(crate) config: LabConfig,
    /// Targets en el orden de `lab.yaml`.
    pub(crate) order: Vec<String>,
    pub(crate) cases: Vec<Case>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FailedAssert {
    pub why: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Una línea de `results.jsonl` (spec §5.1).
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
    /// Texto del error cuando la ejecución termina en `error` (p. ej. «node not found»).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub passed: bool,
    pub failed_asserts: Vec<FailedAssert>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    pub elapsed_ms: u64,
    /// ¿Hizo alguna llamada a un backend o especialista? No se vuelca a `results.jsonl`.
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
    /// El presupuesto cortó la ejecución a medias.
    pub stopped: bool,
}

impl ResultLine {
    /// Línea vacía de un fixture para un target; la ejecución la va rellenando.
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

    /// Preguntas Jev (no `rule`) del flujo de la capacidad, con su tipo. Un nodo suelto da una entrada con
    /// su nombre; un nodo `batch`, una por pregunta, nombrada `"<nodo>.<pregunta>"`: esa es la clave con la
    /// que el daemon pide y graba cada respuesta (D3).
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

    /// El flujo de la capacidad con `backend`/`model` del target; el registro no se toca.
    pub(crate) fn target_flow(&self, cap: &Capability, target: &Target) -> Option<Arc<Flow>> {
        let Provider::Flow { flow } = &cap.provider else { return None };
        let mut flow = (*self.state.flows.get(flow)?).clone();
        for node in flow.nodes.values_mut().filter(|n| n.ty != NodeType::Rule) {
            node.backend = target.backend.clone();
            node.model = target.model.clone();
        }
        Some(Arc::new(flow))
    }

    /// ¿Puede ejecutarse el target? Los que no figuran en `lab.yaml` (fixtures de otros proveedores) cuentan que sí.
    pub(crate) fn has_backend(&self, target: &str) -> bool {
        self.config.targets.get(target).is_none_or(|t| self.state.cfg.backends.contains_key(&t.backend))
    }

    /// Targets que se saltan porque su backend no está en la configuración del daemon: aviso, no error.
    pub fn notes(&self) -> Vec<String> {
        let skipped = self.order.iter().filter(|n| !self.has_backend(n));
        skipped.map(|n| format!("target {n}: backend no configurado en esta config")).collect()
    }

    /// Validaciones de spec §3 sin red; vacío si todo está en orden.
    pub fn check(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for Case { cap, fixture } in &self.cases {
            // Un nodo `batch` responde por preguntas: las claves del replay son `"<nodo>.<pregunta>"` (D3).
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
