//! Catalogue of named signals: `config/signals/*.yaml`, read from disk every time a flow is loaded.

use crate::flows::flow_name;
use jevalan_flow::{Signals, parse_signal};
use std::path::Path;

/// The valid signals in `dir` and, separately, the reason for each file that is not valid. Without a directory, an empty catalogue.
// ponytail: it is re-read in full on every flow load; they are a few small files. Cache it if that stops being true.
pub fn load_dir(dir: &Path) -> (Signals, Vec<(String, String)>) {
    let (mut signals, mut invalid) = (Signals::new(), Vec::new());
    let Ok(entries) = std::fs::read_dir(dir) else { return (signals, invalid) };
    for path in entries.flatten().map(|e| e.path()) {
        let Some(name) = flow_name(&path) else { continue };
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| format!("no se puede leer {}: {e}", path.display()))
            .and_then(|text| parse_signal(&name, &text).map_err(|e| e.to_string()));
        match parsed {
            Ok(def) => {
                signals.insert(name, def);
            }
            Err(reason) => invalid.push((name, reason)),
        }
    }
    (signals, invalid)
}
