//! Carga de capacidades desde disco: versiones, recarga en caliente y su rastro en el bus.

use super::CapRegistry;
use crate::bus::Bus;
use crate::config::Hub;
use crate::flows;
use jevalan_events::v2::{Event, EventKind, new_event};
use jevalan_hub::capability::{CapError, Capability, Provider, parse_capability_with};
use notify::{EventKind as FsKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

const SOURCE: &str = "daemon";

impl CapRegistry {
    pub fn load_dir(&self, dir: &Path, flows: &flows::Registry, hub: &Hub, bus: &Bus) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| cap_name(p).is_some()).collect();
        paths.sort();
        for path in paths {
            self.load_file(&path, dir, flows, hub, bus);
        }
    }

    /// Un fichero ilegible cuenta como inválido; la versión vigente se respeta.
    pub fn load_file(&self, path: &Path, dir: &Path, flows: &flows::Registry, hub: &Hub, bus: &Bus) {
        let Some(name) = cap_name(path) else { return };
        let read = std::fs::read_to_string(path).map_err(|e| format!("no se puede leer {}: {e}", path.display()));
        let parsed = read.and_then(|text| {
            // Primero el entorno del proceso; si falta, el `.env`. Sin `allow_exec` una variable ausente no invalida.
            let vars = |n: &str| std::env::var(n).ok().or_else(|| hub.dotenv.get(n).cloned());
            let cap = parse_capability_with(&name, &text, dir, &|f| flows.get(f).is_some(), &vars, hub.allow_exec)
                .map_err(|e| e.to_string())?;
            // Además del YAML cuentan los esquemas ya resueltos: un `$file` editado también es un cambio.
            let key = format!("{text}\n@@\n{}\n@@\n{}", cap.input_schema, cap.decision_schema);
            Ok((cap, key))
        });

        // Se publica con el cerrojo tomado: quien se suscribe antes de leer la instantánea no pierde nada.
        let mut inner = self.write();
        match parsed {
            Ok((cap, key)) => {
                inner.invalid.remove(&name);
                if inner.caps.contains_key(&name) && inner.texts.get(&name) == Some(&key) {
                    return;
                }
                let version = inner.versions.entry(name.clone()).or_insert(0);
                *version += 1;
                let event = loaded_event(&name, *version, &cap);
                inner.texts.insert(name.clone(), key);
                inner.caps.insert(name, Arc::new(cap));
                bus.publish(event);
            }
            Err(reason) => {
                if inner.invalid.get(&name) == Some(&reason) {
                    return;
                }
                inner.invalid.insert(name.clone(), reason.clone());
                bus.publish(self.invalid_event(name, reason));
            }
        }
    }

    pub fn remove(&self, name: &str, bus: &Bus) {
        let mut inner = self.write();
        inner.texts.remove(name);
        let was_loaded = inner.caps.remove(name).is_some();
        if inner.invalid.remove(name).is_some() || was_loaded {
            let trace_id = format!("capability:{name}:removed:{}", self.tick());
            bus.publish(new_event(&trace_id, 1, SOURCE, EventKind::CapabilityRemoved { capability: name.into() }));
        }
    }

    /// Cambió el conjunto de flujos: las capacidades `flow` huérfanas caen y las que recuperan su flujo vuelven.
    pub fn revalidate(&self, dir: &Path, flows: &flows::Registry, hub: &Hub, bus: &Bus) {
        {
            let mut inner = self.write();
            let orphans: Vec<(String, String)> = inner
                .caps
                .iter()
                .filter_map(|(name, cap)| match &cap.provider {
                    Provider::Flow { flow } if flows.get(flow).is_none() => Some((name.clone(), flow.clone())),
                    _ => None,
                })
                .collect();
            for (name, flow) in orphans {
                inner.caps.remove(&name);
                inner.texts.remove(&name);
                let reason = CapError::BadProvider(format!("flujo desconocido: {flow}")).to_string();
                inner.invalid.insert(name.clone(), reason.clone());
                // Ya no se sirve: tras explicar el motivo, el puerto debe desaparecer del tablero.
                let removed = format!("capability:{name}:removed:{}", self.tick());
                bus.publish(self.invalid_event(name.clone(), reason));
                bus.publish(new_event(&removed, 1, SOURCE, EventKind::CapabilityRemoved { capability: name }));
            }
        }
        self.load_dir(dir, flows, hub, bus);
    }

    /// Un `CAPABILITY_LOADED` por capacidad vigente, para quien acaba de conectarse.
    pub fn loaded_events(&self) -> Vec<Event> {
        let inner = self.read();
        inner.caps.iter().map(|(name, cap)| loaded_event(name, inner.versions[name], cap)).collect()
    }

    /// El vigilante deja de actuar al soltarse el valor devuelto.
    pub fn watch(
        self: Arc<Self>,
        dir: PathBuf,
        flows: Arc<flows::Registry>,
        hub: Hub,
        bus: Bus,
    ) -> notify::Result<RecommendedWatcher> {
        let base = dir.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, FsKind::Access(_)) {
                return;
            }
            for path in &event.paths {
                // Cualquier cambio bajo `schemas/` puede afectar a varias capacidades: se relee todo.
                if path.parent().is_some_and(|p| p != base) {
                    self.load_dir(&base, &flows, &hub, &bus);
                    continue;
                }
                let Some(name) = cap_name(path) else { continue };
                if matches!(event.kind, FsKind::Remove(_)) || !path.exists() {
                    self.remove(&name, &bus);
                } else {
                    self.load_file(path, &base, &flows, &hub, &bus);
                }
            }
        })?;
        watcher.watch(&dir, RecursiveMode::Recursive)?;
        Ok(watcher)
    }

    fn invalid_event(&self, name: String, reason: String) -> Event {
        let trace_id = format!("capability:{name}:invalid:{}", self.tick());
        new_event(&trace_id, 1, SOURCE, EventKind::CapabilityInvalid { capability: name, reason })
    }

    fn tick(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, super::Inner> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Nombre de la capacidad si `path` es un `*.yaml` / `*.yml`.
fn cap_name(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    if ext != "yaml" && ext != "yml" {
        return None;
    }
    path.file_stem()?.to_str().map(str::to_string)
}

/// Sin comando, entorno ni URL: solo el tipo y el identificador del proveedor.
fn loaded_event(name: &str, version: u64, cap: &Capability) -> Event {
    let kind = EventKind::CapabilityLoaded {
        capability: name.into(),
        version: cap.version.clone(),
        provider: cap.provider_id.clone(),
        provider_type: cap.provider.kind().into(),
        flow: match &cap.provider {
            Provider::Flow { flow } => Some(flow.clone()),
            _ => None,
        },
        description: cap.description.clone(),
        input_fields: fields(&cap.input_schema),
        decision_fields: fields(&cap.decision_schema),
    };
    new_event(&format!("capability:{name}:{version}"), 1, SOURCE, kind)
}

/// Claves de primer nivel de `properties`, ordenadas; vacío si el esquema no las declara.
fn fields(schema: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = schema["properties"].as_object().map_or_else(Vec::new, |p| p.keys().cloned().collect());
    keys.sort();
    keys
}
