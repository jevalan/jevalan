//! Inspector: the editable rows of the selected node and the field being typed.

use jevalan_flow::{Criteria, NodeDef, NodeType};

/// Editable field of a node, as the inspector walks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Campo {
    Instructions,
    /// A `rule`'s CEL expression.
    When,
    /// A `noul`'s threshold.
    Threshold,
    MinConfidence,
    Backend,
    Model,
    /// The `when` of `route` rule number `indice`.
    RouteWhen(usize),
    /// The destination of `route` rule number `indice`.
    RouteTo(usize),
    /// The destination of a `batch`'s `else`.
    Else,
    /// The description of option `clave` of a `choice` (or of a criterion of a `noul`).
    Opcion(String),
    /// The name of level `indice` of a `score`.
    Nivel(usize),
}

/// An inspector row: its field, its label and the value it has today.
#[derive(Debug, Clone, PartialEq)]
pub struct Fila {
    pub campo: Campo,
    pub etiqueta: String,
    pub valor: String,
}

/// The rows the inspector offers for `def`, in the order they are walked.
pub fn filas(def: &NodeDef) -> Vec<Fila> {
    let fila = |campo, etiqueta: String, valor: String| Fila { campo, etiqueta, valor };
    let mut filas = vec![fila(Campo::Instructions, "instructions".into(), def.instructions.clone())];
    match def.ty {
        NodeType::Rule => filas.push(fila(Campo::When, "when".into(), def.when.clone().unwrap_or_default())),
        NodeType::Noul => {
            filas.push(fila(Campo::Threshold, "threshold".into(), numero(def.threshold)));
            filas.extend(criterios(&def.criteria));
        }
        NodeType::Choice => filas.extend(criterios(&def.criteria)),
        NodeType::Score => {
            if let Some(Criteria::Levels(niveles)) = &def.criteria {
                for (i, nivel) in niveles.iter().enumerate() {
                    filas.push(fila(Campo::Nivel(i), format!("nivel {i}"), nivel.clone()));
                }
            }
        }
        NodeType::Batch => {
            for (i, regla) in def.route.iter().enumerate() {
                filas.push(fila(Campo::RouteWhen(i), format!("route {i} when"), regla.when.clone()));
                filas.push(fila(Campo::RouteTo(i), format!("route {i} to"), regla.to.clone()));
            }
            filas.push(fila(Campo::Else, "else".into(), def.else_branch.clone().unwrap_or_default()));
        }
    }
    // A `rule` carries no backend, model or confidence threshold: the engine sets them.
    if !matches!(def.ty, NodeType::Rule) {
        filas.push(fila(Campo::MinConfidence, "min_confidence".into(), numero(def.min_confidence)));
        filas.push(fila(Campo::Backend, "backend".into(), def.backend.clone().unwrap_or_default()));
        filas.push(fila(Campo::Model, "model".into(), def.model.clone().unwrap_or_default()));
    }
    filas
}

/// The rows for the criteria of a `choice` or a `noul`.
fn criterios(criteria: &Option<Criteria>) -> Vec<Fila> {
    match criteria {
        Some(Criteria::Options(m)) => m
            .iter()
            .map(|(clave, texto)| Fila {
                campo: Campo::Opcion(clave.clone()),
                etiqueta: format!("criterio {clave}"),
                valor: texto.clone().unwrap_or_default(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A number as text; empty if there is none.
fn numero(valor: Option<f64>) -> String {
    valor.map_or(String::new(), |v| v.to_string())
}

/// The inspector's cursor and the text being typed.
#[derive(Debug, Default)]
pub struct Inspector {
    /// Fila seleccionada.
    pub cursor: usize,
    /// Text in progress of the field being edited; `None` if nothing is being edited.
    pub edicion: Option<String>,
}

impl Inspector {
    pub fn nuevo() -> Self {
        Self::default()
    }

    /// Keeps the cursor within the rows that exist.
    pub fn ajustar(&mut self, filas: usize) {
        self.cursor = self.cursor.min(filas.saturating_sub(1));
    }

    pub fn bajar(&mut self, filas: usize) {
        if filas > 0 {
            self.cursor = (self.cursor + 1).min(filas - 1);
        }
    }

    pub fn subir(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Starts editing the value it is given, which is that of the selected row.
    pub fn empezar(&mut self, valor: &str) {
        self.edicion = Some(valor.to_string());
    }

    pub fn cancelar(&mut self) {
        self.edicion = None;
    }

    /// Writes a character at the end of the text in progress; without an edit, it does nothing.
    pub fn escribir(&mut self, c: char) {
        if let Some(texto) = &mut self.edicion {
            texto.push(c);
        }
    }

    /// Deletes the last character of the text in progress.
    pub fn borrar(&mut self) {
        if let Some(texto) = &mut self.edicion {
            texto.pop();
        }
    }

    /// The text in progress, if editing.
    pub fn texto(&self) -> Option<&str> {
        self.edicion.as_deref()
    }
}

// crates/jevalan-edit/src/inspector.rs — at the end of the file
#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: &str = include_str!("../../jevalan-flow/fixtures/tool-risk-local-v1.yaml");

    fn defs() -> jevalan_flow::FlowDef {
        serde_yaml::from_str(LOCAL).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn las_filas_de_una_regla_llevan_su_when_y_ni_backend_ni_modelo() {
        let filas = filas(&defs().nodes["rm_root"]);
        assert_eq!(filas[0].campo, Campo::Instructions);
        assert_eq!(filas[1].campo, Campo::When);
        // A `rule` has no backend or model of its own: the engine sets them.
        assert!(!filas.iter().any(|f| f.campo == Campo::Backend), "{filas:?}");
        assert_eq!(filas.len(), 2);
    }

    #[test]
    fn las_filas_de_un_batch_llevan_sus_reglas_y_su_else() {
        let filas = filas(&defs().nodes["risks"]);
        let etiquetas: Vec<&str> = filas.iter().map(|f| f.etiqueta.as_str()).collect();
        assert_eq!(
            etiquetas,
            [
                "instructions",
                "route 0 when",
                "route 0 to",
                "route 1 when",
                "route 1 to",
                "else",
                "min_confidence",
                "backend",
                "model"
            ]
        );
        assert_eq!(filas[5].valor, "allow", "el `else` del flujo local");
    }

    #[test]
    fn las_filas_de_un_noul_llevan_su_umbral_y_sus_criterios() {
        let filas = filas(&defs().nodes["risks"].questions["privilege"]);
        let etiquetas: Vec<&str> = filas.iter().map(|f| f.etiqueta.as_str()).collect();
        // The `true`/`false` criteria are a `BTreeMap`: `false` comes before `true`.
        assert_eq!(
            etiquetas,
            ["instructions", "threshold", "criterio false", "criterio true", "min_confidence", "backend", "model"]
        );
    }

    #[test]
    fn el_cursor_y_el_texto_en_edicion_no_se_salen() {
        let mut inspector = Inspector::nuevo();
        inspector.ajustar(0);
        assert_eq!(inspector.cursor, 0);
        inspector.ajustar(2);
        inspector.bajar(2);
        assert_eq!(inspector.cursor, 1);
        inspector.subir();
        inspector.subir();
        assert_eq!(inspector.cursor, 0);

        inspector.empezar("0.55");
        inspector.escribir('6');
        assert_eq!(inspector.texto(), Some("0.556"));
        inspector.borrar();
        inspector.borrar();
        assert_eq!(inspector.texto(), Some("0.5"));
        inspector.cancelar();
        assert_eq!(inspector.texto(), None);
    }
}
