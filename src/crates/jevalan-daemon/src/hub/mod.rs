//! Hub of decisions front desk: capability registry, providers and `POST /v1/hub`.

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
    /// Increases on every valid load and survives deletion.
    versions: BTreeMap<String, u64>,
    /// YAML plus resolved schemas with which each current capability was loaded: an identical re-read is a no-op.
    texts: BTreeMap<String, String>,
}

/// Capability registry; loading from disk and reloading live in `registry.rs`.
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

    /// The `Arc` is cloned: a reload halfway through a request does not alter that request.
    pub fn get(&self, name: &str) -> Option<Arc<Capability>> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner).caps.get(name).cloned()
    }

    /// Public summary of each capability (never command, environment or URL) and the reason for each invalid one.
    pub fn list(&self) -> Value {
        let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        let caps: BTreeMap<_, _> = inner.caps.iter().map(|(n, c)| (n, c.summary())).collect();
        json!({ "capabilities": caps, "invalid": inner.invalid })
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }
}
