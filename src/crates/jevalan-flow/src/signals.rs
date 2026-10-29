//! Named signals: a catalogue (`config/signals/*.yaml`) that several flows reuse.
//!
//! A signal is the body of an unwired node. `type: rule` is a **heuristic**: CEL over `input`,
//! which the `when` expressions read as `signals.<name>`. `noul`/`choice`/`score` is a **learned** one: a question
//! that a `batch` includes with `signals: [<name>]`. The signal says what is asked; the flow, whom to ask.

use crate::model::{FlowDef, NodeDef, NodeType};
use crate::validate::{FlowError, cel_identifier};
use std::collections::{BTreeMap, BTreeSet};

/// Catalogue of already-validated signals, by name.
pub type Signals = BTreeMap<String, NodeDef>;

/// Validates a signal's file. `name` is the file stem and will be the name of the question or of
/// `signals.<name>`, so it must be a CEL identifier.
pub fn parse_signal(name: &str, yaml: &str) -> Result<NodeDef, FlowError> {
    let bad = |reason: &str| FlowError::BadSignal { signal: name.into(), reason: reason.into() };
    if !cel_identifier(name) {
        return Err(bad("el nombre debe ser un identificador CEL: [a-z_][a-z0-9_]*"));
    }
    let def: NodeDef = serde_yaml::from_str(yaml).map_err(|e| bad(&format!("YAML inválido: {e}")))?;
    // What is asked, never whom nor where it goes: that belongs to the flow that uses it.
    let wiring = !def.out.is_empty()
        || def.backend.is_some()
        || def.model.is_some()
        || def.min_confidence.is_some()
        || def.threshold.is_some()
        || !def.questions.is_empty()
        || !def.route.is_empty()
        || def.else_branch.is_some()
        || !def.signals.is_empty();
    if wiring {
        return Err(bad("una señal solo admite type, instructions, criteria y when"));
    }
    match def.ty {
        NodeType::Batch => Err(bad("una señal no puede ser batch")),
        NodeType::Rule => {
            let when = def.when.as_deref().ok_or_else(|| bad("una heurística necesita when"))?;
            let program = crate::program::compiled(when).map_err(|e| bad(&format!("CEL inválida: {e}")))?;
            // It only sees the input: this way it is valid at any point in the path and never depends on another signal.
            let refs = program.references();
            if refs.has_variable("answers") || refs.has_variable("signals") {
                return Err(bad("una heurística solo puede leer input"));
            }
            Ok(def)
        }
        _ if def.when.is_some() => Err(bad("when solo vale para una heurística (type: rule)")),
        // Without a question, the model would answer nothing and that answer could end up as `allow` (spec §3.1).
        _ if def.instructions.trim().is_empty() => Err(bad("una señal aprendida necesita instructions")),
        _ => Ok(def),
    }
}

/// Names that `expr` reads as `signals.<name>`, without repeats.
// ponytail: a text search, not an AST one; a literal containing "signals.x" counts extra, and it only
// costs evaluating that heuristic (or rejecting the flow if it does not exist). Move to `cel`'s AST if it starts to bother.
pub(crate) fn referenced(expr: &str) -> BTreeSet<String> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut names = BTreeSet::new();
    for (i, _) in expr.match_indices("signals.") {
        if expr[..i].chars().next_back().is_some_and(|c| ident(c) || c == '.') {
            continue;
        }
        let name: String = expr[i + "signals.".len()..].chars().take_while(|c| ident(*c)).collect();
        if !name.is_empty() {
            names.insert(name);
        }
    }
    names
}

/// Expands `def`'s signals with the catalogue: each `signals: [..]` of a `batch` becomes a question
/// named after the signal, and each `signals.<name>` of a CEL is looked up among the heuristics.
/// Returns the expanded `FlowDef` and the heuristics the flow reads (name → `when`).
pub(crate) fn resolve(mut def: FlowDef, catalog: &Signals) -> Result<(FlowDef, BTreeMap<String, String>), FlowError> {
    for (name, node) in def.nodes.iter_mut() {
        if node.signals.is_empty() {
            continue;
        }
        if node.ty != NodeType::Batch {
            return Err(FlowError::BadCriteria { node: name.clone(), reason: "signals solo vale para batch".into() });
        }
        for signal in std::mem::take(&mut node.signals) {
            let body = catalog
                .get(&signal)
                .ok_or_else(|| FlowError::UnknownSignal { node: name.clone(), signal: signal.clone() })?;
            if body.ty == NodeType::Rule {
                return Err(FlowError::BadCriteria {
                    node: format!("{name}.{signal}"),
                    reason: "una heurística no se pregunta: léela como signals.<nombre> en una CEL".into(),
                });
            }
            if node.questions.insert(signal.clone(), body.clone()).is_some() {
                return Err(FlowError::BadCriteria {
                    node: format!("{name}.{signal}"),
                    reason: "la señal choca con una pregunta del mismo nombre".into(),
                });
            }
        }
    }
    let mut heuristics = BTreeMap::new();
    for (name, node) in &def.nodes {
        let exprs = node.when.iter().chain(node.route.iter().map(|r| &r.when));
        for signal in exprs.flat_map(|e| referenced(e)) {
            match catalog.get(&signal) {
                Some(body) if body.ty == NodeType::Rule => {
                    heuristics.insert(signal, body.when.clone().unwrap_or_default());
                }
                Some(_) => {
                    return Err(FlowError::BadCriteria {
                        node: name.clone(),
                        reason: format!("signals.{signal} es aprendida: inclúyela en un batch con signals: [{signal}]"),
                    });
                }
                None => return Err(FlowError::UnknownSignal { node: name.clone(), signal }),
            }
        }
    }
    Ok((def, heuristics))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referenced_lee_signals_punto_nombre() {
        let r = referenced("signals.a && !signals.b_2 || x.signals.c || mysignals.d || signals.a");
        assert_eq!(r.into_iter().collect::<Vec<_>>(), ["a", "b_2"]);
    }
}
