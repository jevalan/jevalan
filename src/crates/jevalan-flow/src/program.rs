//! Each CEL expression is compiled only once: the parser uses a lot of stack and a lot of time.

use cel::Program;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

static CACHE: Mutex<BTreeMap<String, Arc<Program>>> = Mutex::new(BTreeMap::new());

/// Compiled program of `expr`; only those that compile are remembered.
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
