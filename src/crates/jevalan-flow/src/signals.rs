//! Señales con nombre: un catálogo (`config/signals/*.yaml`) que varios flujos reutilizan.
//!
//! Una señal es el cuerpo de un nodo sin cablear. `type: rule` es una **heurística**: CEL sobre `input`,
//! que los `when` leen como `signals.<nombre>`. `noul`/`choice`/`score` es una **aprendida**: una pregunta
//! que un `batch` incluye con `signals: [<nombre>]`. La señal dice qué se pregunta; el flujo, a quién.

use crate::model::{FlowDef, NodeDef, NodeType};
use crate::validate::{FlowError, cel_identifier};
use std::collections::{BTreeMap, BTreeSet};

/// Catálogo de señales ya validadas, por nombre.
pub type Signals = BTreeMap<String, NodeDef>;

/// Valida el fichero de una señal. `name` es el tallo del fichero y será el nombre de la pregunta o de
/// `signals.<name>`, así que ha de ser un identificador CEL.
pub fn parse_signal(name: &str, yaml: &str) -> Result<NodeDef, FlowError> {
    let bad = |reason: &str| FlowError::BadSignal { signal: name.into(), reason: reason.into() };
    if !cel_identifier(name) {
        return Err(bad("el nombre debe ser un identificador CEL: [a-z_][a-z0-9_]*"));
    }
    let def: NodeDef = serde_yaml::from_str(yaml).map_err(|e| bad(&format!("YAML inválido: {e}")))?;
    // Qué se pregunta, nunca a quién ni adónde se va: eso es del flujo que la usa.
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
            // Solo ve la entrada: así vale en cualquier punto del camino y nunca depende de otra señal.
            let refs = program.references();
            if refs.has_variable("answers") || refs.has_variable("signals") {
                return Err(bad("una heurística solo puede leer input"));
            }
            Ok(def)
        }
        _ if def.when.is_some() => Err(bad("when solo vale para una heurística (type: rule)")),
        // Sin pregunta, el modelo contestaría a nada y esa respuesta podría acabar en `allow` (spec §3.1).
        _ if def.instructions.trim().is_empty() => Err(bad("una señal aprendida necesita instructions")),
        _ => Ok(def),
    }
}

/// Nombres que `expr` lee como `signals.<nombre>`, sin repetir.
// ponytail: búsqueda en el texto, no en el AST; un literal que contenga «signals.x» cuenta de más, y solo
// cuesta evaluar esa heurística (o rechazar el flujo si no existe). Pasar al AST de `cel` si llega a molestar.
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

/// Expande las señales de `def` con el catálogo: cada `signals: [..]` de un `batch` se vuelve una pregunta
/// con el nombre de la señal, y cada `signals.<nombre>` de una CEL se busca entre las heurísticas.
/// Devuelve el `FlowDef` expandido y las heurísticas que el flujo lee (nombre → `when`).
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
