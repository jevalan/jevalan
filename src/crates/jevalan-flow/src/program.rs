//! Cada expresión CEL se compila una sola vez: el analizador consume mucha pila y mucho tiempo.

use cel::Program;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

static CACHE: Mutex<BTreeMap<String, Arc<Program>>> = Mutex::new(BTreeMap::new());

/// Programa compilado de `expr`; solo se recuerdan los que compilan.
pub fn compiled(expr: &str) -> Result<Arc<Program>, String> {
    if let Some(p) = CACHE.lock().unwrap_or_else(PoisonError::into_inner).get(expr) {
        return Ok(p.clone());
    }
    let program = Arc::new(Program::compile(expr).map_err(|e| e.to_string())?);
    CACHE.lock().unwrap_or_else(PoisonError::into_inner).insert(expr.to_string(), program.clone());
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_once_per_expression() {
        let (a, b) = (compiled("1 + 1 == 2").unwrap(), compiled("1 + 1 == 2").unwrap());
        assert!(Arc::ptr_eq(&a, &b));
        assert!(compiled("1 +").is_err());
    }
}
