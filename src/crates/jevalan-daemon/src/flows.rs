//! Registro de flujos: carga, versiones, recarga en caliente y su rastro en el bus.

use crate::bus::Bus;
use crate::config::Config;
use jevalan_events::v2::{Event, EventKind, new_event};
use jevalan_flow::{Flow, FlowError, parse_with};
use notify::{EventKind as FsKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError};
use std::sync::{OnceLock, RwLock};

const SOURCE: &str = "daemon";

type Hook = Box<dyn Fn(&Registry) + Send + Sync>;

#[derive(Default)]
struct Inner {
    flows: BTreeMap<String, Arc<Flow>>,
    /// Sube en cada carga válida y sobrevive al borrado del flujo.
    versions: BTreeMap<String, u64>,
    invalid: BTreeMap<String, String>,
}

/// `std::sync::RwLock` y no el de tokio: el callback de `notify` es síncrono.
#[derive(Default)]
pub struct Registry {
    inner: RwLock<Inner>,
    counter: AtomicU64,
    on_change: OnceLock<Hook>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Aviso, con el cerrojo ya soltado, cada vez que cambia el conjunto de flujos vigentes.
    pub fn set_on_change(&self, hook: impl Fn(&Registry) + Send + Sync + 'static) {
        let _ = self.on_change.set(Box::new(hook));
    }

    fn changed(&self) {
        if let Some(hook) = self.on_change.get() {
            hook(self);
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<Flow>> {
        self.read().flows.get(name).cloned()
    }

    pub fn load_dir(&self, dir: &Path, cfg: &Config, bus: &Bus) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| flow_name(p).is_some()).collect();
        paths.sort();
        for path in paths {
            self.load_file(&path, cfg, bus);
        }
    }

    /// Un fichero ilegible (a medio escribir, bloqueado) cuenta como inválido; la versión vigente se respeta.
    pub fn load_file(&self, path: &Path, cfg: &Config, bus: &Bus) {
        let Some(name) = flow_name(path) else { return };
        let backends: Vec<&str> = cfg.backends.keys().map(String::as_str).collect();
        let (signals, invalid) = crate::signals::load_dir(&cfg.hub.signals_dir);
        let parsed = std::fs::read_to_string(path)
            .map_err(|e| format!("no se puede leer {}: {e}", path.display()))
            .and_then(|text| {
                parse_with(&name, &text, &signals, &backends, &cfg.default_backend).map_err(|e| why(e, &invalid))
            });

        // Se publica con el cerrojo tomado: quien se suscribe antes de leer la instantánea no pierde nada.
        let mut inner = self.write();
        match parsed {
            Ok(flow) => {
                inner.invalid.remove(&name);
                if inner.flows.get(&name).is_some_and(|old| **old == flow) {
                    return;
                }
                let version = inner.versions.entry(name.clone()).or_insert(0);
                *version += 1;
                let event = loaded_event(&name, *version, &flow);
                inner.flows.insert(name, Arc::new(flow));
                bus.publish(event);
                drop(inner);
                self.changed();
            }
            Err(reason) => {
                if inner.invalid.get(&name) == Some(&reason) {
                    return;
                }
                inner.invalid.insert(name.clone(), reason.clone());
                let trace_id = format!("flow:{name}:invalid:{}", self.tick());
                bus.publish(new_event(&trace_id, 1, SOURCE, EventKind::FlowInvalid { flow: name, reason }));
            }
        }
    }

    pub fn remove(&self, name: &str, bus: &Bus) {
        let mut inner = self.write();
        let was_loaded = inner.flows.remove(name).is_some();
        if inner.invalid.remove(name).is_some() || was_loaded {
            let trace_id = format!("flow:{name}:removed:{}", self.tick());
            bus.publish(new_event(&trace_id, 1, SOURCE, EventKind::FlowRemoved { flow: name.into() }));
        }
        drop(inner);
        if was_loaded {
            self.changed();
        }
    }

    /// Un `FLOW_LOADED` por flujo vigente, para quien acaba de conectarse.
    pub fn loaded_events(&self) -> Vec<Event> {
        let inner = self.read();
        inner.flows.iter().map(|(name, flow)| loaded_event(name, inner.versions[name], flow)).collect()
    }

    /// Definición completa de cada flujo y motivo de cada inválido.
    pub fn list(&self) -> Value {
        let inner = self.read();
        let flows: BTreeMap<_, _> = inner.flows.iter().map(|(name, flow)| (name, &**flow)).collect();
        json!({ "flows": flows, "invalid": inner.invalid })
    }

    /// El vigilante deja de actuar al soltarse el valor devuelto.
    pub fn watch(self: Arc<Self>, cfg: Arc<Config>, bus: Bus) -> notify::Result<RecommendedWatcher> {
        let dir = cfg.flows_dir.clone();
        let signals_dir = cfg.hub.signals_dir.clone();
        // ponytail: un `signals_dir` que no existía al arrancar no se vigila; reiniciar el daemon tras crearlo.
        let signals_canon = signals_dir.canonicalize().ok();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, FsKind::Access(_)) {
                return;
            }
            for path in &event.paths {
                // Una señal cambió: cualquier flujo puede depender de ella, así que se recargan todos.
                if signals_canon.is_some() && path.parent().and_then(|p| p.canonicalize().ok()) == signals_canon {
                    self.load_dir(&cfg.flows_dir, &cfg, &bus);
                    continue;
                }
                let Some(name) = flow_name(path) else { continue };
                if matches!(event.kind, FsKind::Remove(_)) || !path.exists() {
                    self.remove(&name, &bus);
                } else {
                    self.load_file(path, &cfg, &bus);
                }
            }
        })?;
        watcher.watch(&dir, RecursiveMode::NonRecursive)?;
        if signals_dir.is_dir() {
            watcher.watch(&signals_dir, RecursiveMode::NonRecursive)?;
        }
        Ok(watcher)
    }

    fn tick(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Nombre del flujo si `path` es un `*.yaml` / `*.yml`.
pub(crate) fn flow_name(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    if ext != "yaml" && ext != "yml" {
        return None;
    }
    path.file_stem()?.to_str().map(str::to_string)
}

/// Motivo de un flujo que no carga; si cita una señal cuyo fichero no vale, dice también por qué no vale.
fn why(e: FlowError, invalid: &[(String, String)]) -> String {
    match &e {
        FlowError::UnknownSignal { signal, .. } => match invalid.iter().find(|(name, _)| name == signal) {
            Some((_, reason)) => format!("{e} ({reason})"),
            None => e.to_string(),
        },
        _ => e.to_string(),
    }
}

fn loaded_event(name: &str, version: u64, flow: &Flow) -> Event {
    let kind = EventKind::FlowLoaded {
        flow: name.into(),
        nodes: flow.nodes.len() as u32,
        definition: without_instructions(flow),
    };
    new_event(&format!("flow:{name}:{version}"), 1, SOURCE, kind)
}

/// Los eventos jamás llevan las `instructions` —tampoco las de las preguntas de un nodo `batch`, que son
/// el prompt—; `GET /v1/flows` sí las muestra.
fn without_instructions(flow: &Flow) -> Value {
    let mut def = serde_json::to_value(flow).unwrap_or(Value::Null);
    if let Some(nodes) = def.get_mut("nodes").and_then(Value::as_object_mut) {
        for node in nodes.values_mut().filter_map(Value::as_object_mut) {
            node.remove("instructions");
            if let Some(questions) = node.get_mut("questions").and_then(Value::as_object_mut) {
                for question in questions.values_mut().filter_map(Value::as_object_mut) {
                    question.remove("instructions");
                }
            }
        }
    }
    def
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::new_trace_id;
    use crate::config::Backend;
    use std::path::PathBuf;
    use std::time::Duration;

    const GUARD: &str = include_str!("../../../../config/flows/agent-guard.yaml");

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("jevalan-test-{}", new_trace_id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn cfg(dir: &Path) -> Config {
        let backend = |url: &str| Backend {
            url: url.into(),
            path: "/v1/systemone".into(),
            api_key: None,
            timeout: Duration::from_millis(100),
        };
        Config {
            addr: "127.0.0.1:0".parse().unwrap(),
            token: None,
            backends: BTreeMap::from([
                ("ollama".into(), backend("http://x")),
                ("typesafe".into(), backend("http://y")),
            ]),
            default_backend: "ollama".into(),
            flows_dir: dir.into(),
            hub: Default::default(),
        }
    }

    #[test]
    fn loaded_events_use_the_version_and_it_rises_on_reload() {
        let dir = TempDir::new();
        let (cfg, bus) = (cfg(&dir.0), Bus::new(8));
        let reg = Registry::new();
        let file = dir.0.join("agent-guard.yaml");
        std::fs::write(&file, GUARD).unwrap();
        reg.load_dir(&dir.0, &cfg, &bus);

        let events = reg.loaded_events();
        assert_eq!((events.len(), events[0].trace_id.as_str(), events[0].seq), (1, "flow:agent-guard:1", 1));

        std::fs::write(&file, GUARD.replace("Guardián de comandos", "Otro guardián")).unwrap();
        reg.load_file(&file, &cfg, &bus);
        assert_eq!(reg.loaded_events()[0].trace_id, "flow:agent-guard:2");

        // Releer lo mismo no es una versión nueva.
        reg.load_file(&file, &cfg, &bus);
        assert_eq!(reg.loaded_events()[0].trace_id, "flow:agent-guard:2");
    }

    #[test]
    fn invalid_file_keeps_the_previous_version_and_removal_clears_all() {
        let dir = TempDir::new();
        let (cfg, bus) = (cfg(&dir.0), Bus::new(8));
        let mut rx = bus.subscribe();
        let reg = Registry::new();
        let file = dir.0.join("agent-guard.yaml");
        std::fs::write(&file, GUARD).unwrap();
        reg.load_file(&file, &cfg, &bus);
        std::fs::write(&file, "start: [").unwrap();
        reg.load_file(&file, &cfg, &bus);

        assert!(reg.get("agent-guard").is_some());
        assert!(reg.list()["invalid"]["agent-guard"].is_string());
        assert_eq!(reg.loaded_events()[0].trace_id, "flow:agent-guard:1");
        let kinds: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).map(|e| e.kind.name()).collect();
        assert_eq!(kinds, ["FLOW_LOADED", "FLOW_INVALID"]);

        reg.remove("agent-guard", &bus);
        assert!(reg.get("agent-guard").is_none());
        assert!(reg.list()["invalid"].as_object().unwrap().is_empty());
        assert_eq!(rx.try_recv().unwrap().kind.name(), "FLOW_REMOVED");
    }

    #[test]
    fn events_never_carry_instructions() {
        let dir = TempDir::new();
        let (cfg, bus) = (cfg(&dir.0), Bus::new(8));
        let reg = Registry::new();
        std::fs::write(dir.0.join("agent-guard.yaml"), GUARD).unwrap();
        reg.load_dir(&dir.0, &cfg, &bus);

        let text = serde_json::to_string(&reg.loaded_events()).unwrap();
        assert!(!text.contains("instructions") && !text.contains("borra o sobrescribe"));
        assert!(reg.list().to_string().contains("borra o sobrescribe"));
    }

    /// Un nodo `batch` lleva el prompt dentro de cada pregunta: tampoco eso sale en los eventos.
    const BATCH: &str = r#"
defaults: { backend: ollama, model: nimble }
start: risks
nodes:
  risks:
    type: batch
    questions:
      destructive: { type: noul, instructions: "¿Rompe algo?" }
      privilege: { type: noul, instructions: "¿Eleva privilegios?" }
    route:
      - { when: "answers.risks.destructive >= 0.85", to: deny }
    else: allow
outputs:
  allow: { outcome: allow }
  deny: { outcome: deny }
"#;

    #[test]
    fn events_never_carry_the_instructions_of_a_batch() {
        let dir = TempDir::new();
        let (cfg, bus) = (cfg(&dir.0), Bus::new(8));
        let reg = Registry::new();
        std::fs::write(dir.0.join("tool-risk.yaml"), BATCH).unwrap();
        reg.load_dir(&dir.0, &cfg, &bus);

        let text = serde_json::to_string(&reg.loaded_events()).unwrap();
        assert!(!text.contains("instructions") && !text.contains("¿Rompe algo?"), "{text}");
        // La pregunta sigue nombrada: el grafo necesita saber que existe.
        assert!(text.contains("destructive"), "{text}");
        assert!(reg.list().to_string().contains("¿Rompe algo?"), "el listado sí las muestra");
    }

    const GUARDED: &str = "start: g
nodes:
  g: { type: rule, when: 'signals.root', out: { \"true\": a, \"false\": b } }
outputs:
  a: { outcome: ok }
  b: { outcome: ok }
";

    #[test]
    fn un_flujo_lee_sus_senales_del_catalogo_y_explica_la_que_no_vale() {
        let dir = TempDir::new();
        let (flows, signals) = (dir.0.join("flows"), dir.0.join("signals"));
        std::fs::create_dir_all(&flows).unwrap();
        std::fs::create_dir_all(&signals).unwrap();
        let mut cfg = cfg(&flows);
        cfg.hub.signals_dir = signals.clone();
        let (bus, reg) = (Bus::new(8), Registry::new());

        std::fs::write(
            signals.join("root.yaml"),
            "type: rule
when: 'input.x == 1'
",
        )
        .unwrap();
        std::fs::write(flows.join("guard.yaml"), GUARDED).unwrap();
        reg.load_dir(&flows, &cfg, &bus);
        assert_eq!(reg.get("guard").map(|f| f.signals.len()), Some(1));

        // La señal se estropea: el flujo pasa a inválido, dice por qué, y la versión vigente se respeta.
        std::fs::write(
            signals.join("root.yaml"),
            "type: rule
when: 'answers.x'
",
        )
        .unwrap();
        reg.load_dir(&flows, &cfg, &bus);
        let reason = reg.list()["invalid"]["guard"].as_str().unwrap_or_default().to_string();
        assert!(reason.contains("señal root: una heurística solo puede leer input"), "{reason}");
        assert!(reg.get("guard").is_some(), "la versión vigente sigue sirviendo");
    }
}
