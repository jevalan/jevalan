//! Lectura de `config/lab/`: configuración, fixtures y cassettes.

use crate::write::fnv1a64;
use crate::{Case, Lab};
use jevalan_daemon::AppState;
use jevalan_daemon::config::Config;
use jevalan_hub::lab::config::parse_lab_config;
use jevalan_hub::lab::fixture::{Fixture, parse_fixture};
use serde_json::Value;
use std::path::{Path, PathBuf};

impl Lab {
    /// Carga flujos y capacidades como `serve`, pero sin servidor ni vigilantes.
    pub fn load(root: &Path, cfg: &Config) -> Result<Lab, Vec<String>> {
        let yaml = std::fs::read_to_string(root.join("lab.yaml"))
            .map_err(|e| vec![format!("{}: {e}", root.join("lab.yaml").display())])?;
        let config = parse_lab_config(&yaml).map_err(|e| vec![e])?;
        let state = AppState::new(cfg.clone());
        state.flows.load_dir(&cfg.flows_dir, &state.cfg, &state.bus);
        state.capabilities.load_dir(&cfg.hub.capabilities_dir, &state.flows, &cfg.hub, &state.bus);

        let mut problems = Vec::new();
        let mut cases = Vec::new();
        for dir in sorted(&root.join("fixtures"), true) {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            let Some(cap) = state.capabilities.get(&name) else {
                let why =
                    state.capabilities.list()["invalid"][&name].as_str().map(|r| format!(": {r}")).unwrap_or_default();
                problems.push(format!("fixtures/{name}: la capacidad no está cargada{why}"));
                continue;
            };
            for file in sorted(&dir, false).into_iter().filter(|f| f.extension().is_some_and(|e| e == "yaml")) {
                let stem = file.file_stem().and_then(|n| n.to_str()).unwrap_or_default();
                let parsed = std::fs::read_to_string(&file)
                    .map_err(|e| e.to_string())
                    .and_then(|t| parse_fixture(stem, &t).map_err(|e| e.to_string()));
                match parsed {
                    Ok(fixture) => cases.push(Case { cap: cap.clone(), fixture }),
                    Err(e) => problems.push(format!("fixtures/{name}/{stem}.yaml: {e}")),
                }
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        let order = target_order(&yaml);
        let runs_dir = PathBuf::from(".local/runs");
        Ok(Lab { state, root: root.to_path_buf(), runs_dir, yaml, config, order, cases })
    }

    /// Cassette entera de un fixture para un target; `None` si no hay.
    pub(crate) fn cassette(&self, cap: &str, fixture: &str, target: &str) -> Result<Option<Value>, String> {
        let path = self.root.join("cassettes").join(cap).join(fixture).join(format!("{target}.json"));
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let parsed: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Some(parsed))
    }
}

/// Por qué una cassette ya no vale para el fixture (otra versión u otra entrada), si es el caso.
/// Las cassettes sin estos campos son anteriores y se tratan como desactualizadas.
pub(crate) fn stale(cassette: &Value, fx: &Fixture) -> Option<String> {
    let version = cassette["fixture_version"].as_u64();
    if version != Some(u64::from(fx.version)) {
        let was = version.map_or("sin versión".to_string(), |v| format!("versión {v}"));
        return Some(format!("cassette desactualizada ({was}, actual {})", fx.version));
    }
    (cassette["input_hash"].as_str() != Some(&input_hash(fx)))
        .then(|| format!("cassette desactualizada (la entrada cambió sin subir la versión {})", fx.version))
}

/// Huella FNV-1a de la entrada del fixture.
pub(crate) fn input_hash(fx: &Fixture) -> String {
    fnv1a64(&fx.input.to_string())
}

/// Entradas de un directorio en orden alfabético; solo directorios o solo ficheros.
fn sorted(dir: &Path, dirs: bool) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() == dirs)
        .collect();
    out.sort();
    out
}

/// Los targets en el orden de `lab.yaml` (el mapa tipado los ordena por nombre).
fn target_order(yaml: &str) -> Vec<String> {
    let doc: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap_or(serde_yaml::Value::Null);
    doc.get("targets")
        .and_then(|t| t.as_mapping())
        .map(|m| m.keys().filter_map(|k| k.as_str().map(String::from)).collect())
        .unwrap_or_default()
}
