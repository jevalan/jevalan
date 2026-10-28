//! Ventanilla del hub de decisiones: registro de capacidades, proveedores y `POST /v1/hub`.

mod exec;
pub mod execute;
mod provider;
mod registry;
pub mod route;
pub mod transport;

use jevalan_hub::capability::Capability;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, PoisonError, RwLock};

#[derive(Default)]
struct Inner {
    caps: BTreeMap<String, Arc<Capability>>,
    invalid: BTreeMap<String, String>,
    /// Sube en cada carga válida y sobrevive al borrado.
    versions: BTreeMap<String, u64>,
    /// YAML más esquemas resueltos con que se cargó cada capacidad vigente: una relectura idéntica es un no-op.
    texts: BTreeMap<String, String>,
}

/// Registro de capacidades; la carga desde disco y la recarga viven en `registry.rs`.
#[derive(Default)]
pub struct CapRegistry {
    inner: RwLock<Inner>,
    counter: AtomicU64,
}

impl CapRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, cap: Capability) {
        let mut inner = self.write();
        inner.invalid.remove(&cap.name);
        *inner.versions.entry(cap.name.clone()).or_insert(0) += 1;
        inner.caps.insert(cap.name.clone(), Arc::new(cap));
    }

    pub fn insert_invalid(&self, name: &str, reason: String) {
        let mut inner = self.write();
        inner.caps.remove(name);
        inner.invalid.insert(name.into(), reason);
    }

    /// Se clona el `Arc`: una recarga a mitad de petición no altera esa petición.
    pub fn get(&self, name: &str) -> Option<Arc<Capability>> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner).caps.get(name).cloned()
    }

    /// Resumen público de cada capacidad (jamás comando, entorno ni URL) y motivo de cada inválida.
    pub fn list(&self) -> Value {
        let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        let caps: BTreeMap<_, _> = inner.caps.iter().map(|(n, c)| (n, c.summary())).collect();
        json!({ "capabilities": caps, "invalid": inner.invalid })
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }
}
