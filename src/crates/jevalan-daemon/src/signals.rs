//! Catálogo de señales con nombre: `config/signals/*.yaml`, leído del disco cada vez que se carga un flujo.

use crate::flows::flow_name;
use jevalan_flow::{Signals, parse_signal};
use std::path::Path;

/// Las señales válidas de `dir` y, aparte, el motivo de cada fichero que no vale. Sin directorio, catálogo vacío.
// ponytail: se relee entero en cada carga de flujo; son unos pocos ficheros pequeños. Cachear si deja de serlo.
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
