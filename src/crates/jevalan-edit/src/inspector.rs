//! Inspector: las filas editables del nodo seleccionado y el campo que se está escribiendo.

use jevalan_flow::{Criteria, NodeDef, NodeType};

/// Campo editable de un nodo, tal como lo recorre el inspector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Campo {
    Instructions,
    /// La expresión CEL de un `rule`.
    When,
    /// El umbral de un `noul`.
    Threshold,
    MinConfidence,
    Backend,
    Model,
    /// La `when` de la regla `route` número `indice`.
    RouteWhen(usize),
    /// El destino de la regla `route` número `indice`.
    RouteTo(usize),
    /// El destino del `else` de un `batch`.
    Else,
    /// La descripción de la opción `clave` de un `choice` (o de un criterio de un `noul`).
    Opcion(String),
    /// El nombre del nivel `indice` de un `score`.
    Nivel(usize),
}

/// Una fila del inspector: su campo, su etiqueta y el valor que hoy tiene.
#[derive(Debug, Clone, PartialEq)]
pub struct Fila {
    pub campo: Campo,
    pub etiqueta: String,
    pub valor: String,
}

/// Las filas que el inspector ofrece para `def`, en el orden en que se recorren.
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
    // Un `rule` no lleva backend, modelo ni umbral de confianza: los pone el motor.
    if !matches!(def.ty, NodeType::Rule) {
        filas.push(fila(Campo::MinConfidence, "min_confidence".into(), numero(def.min_confidence)));
        filas.push(fila(Campo::Backend, "backend".into(), def.backend.clone().unwrap_or_default()));
        filas.push(fila(Campo::Model, "model".into(), def.model.clone().unwrap_or_default()));
    }
    filas
}

/// Las filas de los criterios de un `choice` o de un `noul`.
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

/// Un número como texto; vacío si no lo hay.
fn numero(valor: Option<f64>) -> String {
    valor.map_or(String::new(), |v| v.to_string())
}

/// El cursor del inspector y el texto que se está escribiendo.
#[derive(Debug, Default)]
pub struct Inspector {
    /// Fila seleccionada.
    pub cursor: usize,
    /// Texto en curso del campo que se edita; `None` si no se está editando.
    pub edicion: Option<String>,
}

impl Inspector {
    pub fn nuevo() -> Self {
        Self::default()
    }

    /// Mantiene el cursor dentro de las filas que hay.
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

    /// Empieza a editar el valor que se le da, que es el de la fila seleccionada.
    pub fn empezar(&mut self, valor: &str) {
        self.edicion = Some(valor.to_string());
    }

    pub fn cancelar(&mut self) {
        self.edicion = None;
    }

    /// Escribe un carácter al final del texto en curso; sin edición, no hace nada.
    pub fn escribir(&mut self, c: char) {
        if let Some(texto) = &mut self.edicion {
            texto.push(c);
        }
    }

    /// Borra el último carácter del texto en curso.
    pub fn borrar(&mut self) {
        if let Some(texto) = &mut self.edicion {
            texto.pop();
        }
    }

    /// El texto en curso, si se está editando.
    pub fn texto(&self) -> Option<&str> {
        self.edicion.as_deref()
    }
}

// crates/jevalan-edit/src/inspector.rs — al final del fichero
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
        // Un `rule` no tiene backend ni modelo propios: los pone el motor.
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
        // Los criterios `true`/`false` son un `BTreeMap`: `false` va antes que `true`.
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
