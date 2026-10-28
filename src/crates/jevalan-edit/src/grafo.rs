//! Grafo del editor: el flujo que se pinta, las paradas del cursor y la selección.

use jevalan_flow::{
    DEFAULT_MIN_CONFIDENCE, DEFAULT_THRESHOLD, Defaults, ERROR, Flow, FlowDef, LOW_CONFIDENCE, Node, NodeDef, NodeType,
    answer_ports,
};
use jevalan_tui::flow_layout::{FlowNodeId, columns};
use jevalan_tui::widgets::flow_graph;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Una parada del cursor: una caja o una arista del dibujo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parada {
    /// La caja de un nodo.
    Nodo(String),
    /// La caja de una salida.
    Salida(String),
    /// La arista de un puerto de un nodo.
    Puerto { nodo: String, puerto: String },
}

impl Parada {
    /// El nodo al que pertenece la parada, si es de un nodo.
    pub fn nodo(&self) -> Option<&str> {
        match self {
            Parada::Nodo(n) | Parada::Puerto { nodo: n, .. } => Some(n),
            Parada::Salida(_) => None,
        }
    }

    /// El identificador con el que el pintor resalta esta parada; un puerto sin cablear no tiene.
    pub fn resaltado(&self, def: &FlowDef) -> Option<String> {
        match self {
            Parada::Nodo(n) => Some(flow_graph::caja(&FlowNodeId::Node(n.clone()))),
            Parada::Salida(n) => Some(flow_graph::caja(&FlowNodeId::Output(n.clone()))),
            Parada::Puerto { nodo, puerto } => def
                .nodes
                .get(nodo)
                .filter(|d| d.out.contains_key(puerto))
                .map(|_| flow_graph::arista(&FlowNodeId::Node(nodo.clone()), puerto)),
        }
    }
}

/// El grafo del borrador: sus paradas en el orden del dibujo y la que está seleccionada.
pub struct Grafo {
    paradas: Vec<Parada>,
    cursor: usize,
    /// El flujo de dibujo, en un `Arc` estable: el pintor solo rehace su grafo si cambia el puntero.
    pub flujo: Arc<Flow>,
}

impl Default for Grafo {
    fn default() -> Self {
        Self::nuevo()
    }
}

impl Grafo {
    /// Un grafo sin nada que pintar.
    pub fn nuevo() -> Self {
        Self { paradas: Vec::new(), cursor: 0, flujo: Arc::new(flujo_vacio()) }
    }

    /// Rehace las paradas con el borrador y guarda su flujo de dibujo.
    pub fn rehacer(&mut self, nombre: &str, def: &FlowDef) {
        let antes = self.parada().cloned();
        self.paradas = paradas(def);
        self.flujo = Arc::new(dibujo(nombre, def));
        // Si la parada de antes sigue existiendo, el cursor se queda en ella: editar un campo no debe
        // mover al usuario de sitio.
        self.cursor = antes
            .and_then(|p| self.paradas.iter().position(|q| *q == p))
            .unwrap_or_else(|| self.cursor.min(self.paradas.len().saturating_sub(1)));
    }

    /// ¿No hay nada que pintar? Un YAML que no se pudo leer, o un flujo sin `start`.
    pub fn vacio(&self) -> bool {
        self.paradas.is_empty()
    }

    pub fn parada(&self) -> Option<&Parada> {
        self.paradas.get(self.cursor)
    }

    /// El nodo al que mira el inspector.
    pub fn nodo(&self) -> Option<&str> {
        self.parada().and_then(Parada::nodo)
    }

    /// El puerto seleccionado, si la parada es un puerto.
    pub fn puerto(&self) -> Option<(&str, &str)> {
        match self.parada()? {
            Parada::Puerto { nodo, puerto } => Some((nodo, puerto)),
            _ => None,
        }
    }

    pub fn siguiente(&mut self) {
        if !self.paradas.is_empty() {
            self.cursor = (self.cursor + 1).min(self.paradas.len() - 1);
        }
    }
    pub fn anterior(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Qué nodo ha de quedar marcado: el que el cursor señala, y si el cursor está en otro sitio, el
    /// nodo culpable del error de validación (spec §3: «el nodo culpable se señala»).
    pub fn marcado(&self, def: &FlowDef, culpable: Option<&str>) -> Option<String> {
        match culpable {
            Some(n) if self.nodo() != Some(n) && def.nodes.contains_key(n) => Some(n.to_string()),
            _ => self.nodo().map(str::to_string),
        }
    }

    /// Las paradas en el orden del dibujo; el editor las recorre con el cursor.
    pub fn paradas(&self) -> &[Parada] {
        &self.paradas
    }
}

/// El flujo que se pinta, armado desde el borrador **sin juzgarlo**: mientras el flujo no valide, el
/// grafo sigue enseñando lo que hay —y lo que se acaba de añadir—. Quien juzga es
/// `jevalan_flow::parse` (D6), no esto.
pub fn dibujo(nombre: &str, def: &FlowDef) -> Flow {
    Flow {
        name: nombre.to_string(),
        description: def.description.clone(),
        start: def.start.clone(),
        nodes: def.nodes.iter().map(|(n, d)| (n.clone(), nodo(d, &def.defaults))).collect(),
        outputs: def.outputs.iter().map(|(n, o)| (n.clone(), o.clone())).collect(),
        signals: BTreeMap::new(),
    }
}

/// Un `NodeDef` como nodo de dibujo, con los valores por defecto del flujo donde los haya.
fn nodo(def: &NodeDef, defaults: &Defaults) -> Node {
    Node {
        ty: def.ty,
        instructions: def.instructions.clone(),
        when: def.when.clone(),
        criteria: def.criteria.clone(),
        threshold: def.threshold.unwrap_or(DEFAULT_THRESHOLD),
        backend: def.backend.clone().or_else(|| defaults.backend.clone()).unwrap_or_default(),
        model: def.model.clone().or_else(|| defaults.model.clone()).unwrap_or_default(),
        min_confidence: def.min_confidence.or(defaults.min_confidence).unwrap_or(DEFAULT_MIN_CONFIDENCE),
        out: def.out.clone(),
        // Los rangos y las reglas de `route` los calcula `validate`; para pintar no hacen falta.
        ranges: Vec::new(),
        questions: def.questions.iter().map(|(q, d)| (q.clone(), nodo(d, defaults))).collect(),
        route: def.route.iter().map(|r| (r.when.clone(), r.to.clone())).collect(),
        else_branch: def.else_branch.clone(),
    }
}

/// Puertos que el editor ofrece cablear en un nodo: los de respuesta que su tipo admite —la misma
/// tabla que usa `validate`, D6—, más los comunes. Ni un `rule` ni un `batch` admiten `low-confidence`.
pub fn puertos(def: &NodeDef) -> Vec<String> {
    let mut puertos = answer_ports(def);
    puertos.push(ERROR.to_string());
    if !matches!(def.ty, NodeType::Rule | NodeType::Batch) {
        puertos.push(LOW_CONFIDENCE.to_string());
    }
    puertos
}

/// Las paradas, en el orden del dibujo: cada nodo con sus puertos detrás, y luego las salidas.
fn paradas(def: &FlowDef) -> Vec<Parada> {
    if def.start.is_empty() {
        return Vec::new();
    }
    let flow = dibujo("", def);
    let mut paradas = Vec::new();
    for (id, _) in columns(&flow) {
        match id {
            FlowNodeId::Node(nombre) => {
                paradas.push(Parada::Nodo(nombre.clone()));
                if let Some(d) = def.nodes.get(&nombre) {
                    paradas.extend(puertos(d).into_iter().map(|p| Parada::Puerto { nodo: nombre.clone(), puerto: p }));
                }
            }
            FlowNodeId::Output(nombre) => paradas.push(Parada::Salida(nombre)),
        }
    }
    paradas
}

/// Un flujo sin nada, para el grafo de un documento que no se pudo leer.
fn flujo_vacio() -> Flow {
    Flow {
        name: String::new(),
        description: String::new(),
        start: String::new(),
        nodes: BTreeMap::new(),
        outputs: BTreeMap::new(),
        signals: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevalan_flow::FlowDef;

    const LOCAL: &str = include_str!("../../jevalan-flow/fixtures/tool-risk-local-v1.yaml");

    fn def() -> FlowDef {
        serde_yaml::from_str(LOCAL).unwrap_or_else(|e| panic!("{e}"))
    }

    /// El borrador con un `noul` suelto, como el que añade el editor: sin cablear.
    fn con_suelto() -> FlowDef {
        let mut def = def();
        let mut nuevo = def.nodes["pipe_sh"].clone();
        nuevo.ty = NodeType::Noul;
        nuevo.when = None;
        nuevo.out.clear();
        def.nodes.insert("nuevo-1".into(), nuevo);
        def
    }

    #[test]
    fn las_paradas_recorren_nodos_y_puertos() {
        let paradas = paradas(&def());
        assert_eq!(paradas[0], Parada::Nodo("rm_root".into()));
        assert_eq!(paradas[1], Parada::Puerto { nodo: "rm_root".into(), puerto: "true".into() });
        assert_eq!(paradas[2], Parada::Puerto { nodo: "rm_root".into(), puerto: "false".into() });
        assert_eq!(paradas[3], Parada::Puerto { nodo: "rm_root".into(), puerto: "error".into() });
        // Un `rule` no admite `low-confidence`; un `batch` solo cablea su `error`.
        assert!(!paradas.contains(&Parada::Puerto { nodo: "rm_root".into(), puerto: "low-confidence".into() }));
        let del_batch: Vec<&Parada> = paradas.iter().filter(|p| p.nodo() == Some("risks")).collect();
        assert_eq!(del_batch.len(), 2, "el nodo y su puerto de error: {del_batch:?}");
        // Las salidas que cablea un `out` están; las que solo aparecen en un `route` de un `batch` no
        // son aristas del dibujo (el pintor solo cablea `out`) y se ven en el inspector.
        for salida in ["deny_static", "ask_static", "ask_error"] {
            assert!(paradas.contains(&Parada::Salida(salida.into())), "falta la salida {salida}");
        }
    }

    #[test]
    fn el_cursor_alcanza_el_puerto_sin_cablear() {
        let def = con_suelto();
        let mut grafo = Grafo::nuevo();
        grafo.rehacer("tool-risk-local", &def);
        assert!(!grafo.vacio());

        let sueltas: Vec<&Parada> = grafo.paradas().iter().filter(|p| p.nodo() == Some("nuevo-1")).collect();
        assert_eq!(sueltas.len(), 5, "el nodo, sus dos respuestas, su error y su low-confidence: {sueltas:?}");
        assert!(sueltas.iter().any(|p| matches!(p, Parada::Puerto { puerto, .. } if puerto == "true")));

        // Y el cursor llega hasta él con las flechas.
        let mut encontrado = false;
        for _ in 0..grafo.paradas().len() {
            encontrado |= grafo.nodo() == Some("nuevo-1");
            grafo.siguiente();
        }
        assert!(encontrado, "el nodo suelto ha de ser alcanzable");
    }

    #[test]
    fn el_nodo_culpable_se_marca_aunque_el_cursor_este_en_otro() {
        let def = con_suelto();
        let mut grafo = Grafo::nuevo();
        grafo.rehacer("tool-risk-local", &def);
        // El cursor, en el nodo suelto; y el culpable, `rm_root`.
        while grafo.nodo() != Some("nuevo-1") {
            grafo.siguiente();
        }
        assert_eq!(
            grafo.marcado(&def, Some("rm_root")),
            Some("rm_root".to_string()),
            "el culpable se marca aunque el cursor esté en otro"
        );
        assert_eq!(
            grafo.marcado(&def, Some("nuevo-1")),
            Some("nuevo-1".to_string()),
            "y si el cursor ya está en el culpable, manda el cursor"
        );
        assert_eq!(grafo.marcado(&def, None), Some("nuevo-1".to_string()), "sin culpable manda el cursor");
    }

    #[test]
    fn el_flujo_de_dibujo_no_juzga() {
        let def = con_suelto();
        let dibujo = dibujo("tool-risk-local", &def);
        assert_eq!(dibujo.name, "tool-risk-local");
        assert_eq!(dibujo.nodes.len(), 4);
        // El suelto viaja con el backend del flujo y sin que nadie lo juzgue: mientras tanto, la
        // banda inferior es la que dice que el borrador no vale.
        assert_eq!(dibujo.nodes["nuevo-1"].backend, "kev");
        assert!(dibujo.nodes["nuevo-1"].out.is_empty());
    }

    #[test]
    fn el_resaltado_solo_marca_lo_cableado() {
        let def = def();
        assert_eq!(Parada::Nodo("risks".into()).resaltado(&def).as_deref(), Some("n:risks"));
        assert_eq!(
            Parada::Puerto { nodo: "risks".into(), puerto: "error".into() }.resaltado(&def).as_deref(),
            Some("n:risks>error")
        );
        assert!(
            Parada::Puerto { nodo: "risks".into(), puerto: "true".into() }.resaltado(&def).is_none(),
            "un puerto sin cablear no tiene arista que resaltar"
        );
    }
}
