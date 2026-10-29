//! Editor graph: the flow being painted, the cursor stops and the selection.

use jevalan_flow::{
    DEFAULT_MIN_CONFIDENCE, DEFAULT_THRESHOLD, Defaults, ERROR, Flow, FlowDef, LOW_CONFIDENCE, Node, NodeDef, NodeType,
    answer_ports,
};
use jevalan_tui::flow_layout::{FlowNodeId, columns};
use jevalan_tui::widgets::flow_graph;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A cursor stop: a box or an edge of the drawing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parada {
    /// A node's box.
    Nodo(String),
    /// An output's box.
    Salida(String),
    /// The edge of a node's port.
    Puerto { nodo: String, puerto: String },
}

impl Parada {
    /// The node the stop belongs to, if it belongs to a node.
    pub fn nodo(&self) -> Option<&str> {
        match self {
            Parada::Nodo(n) | Parada::Puerto { nodo: n, .. } => Some(n),
            Parada::Salida(_) => None,
        }
    }

    /// The identifier with which the painter highlights this stop; an unwired port has none.
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

/// The draft's graph: its stops in drawing order and the one that is selected.
pub struct Grafo {
    paradas: Vec<Parada>,
    cursor: usize,
    /// The drawing flow, in a stable `Arc`: the painter only rebuilds its graph if the pointer changes.
    pub flujo: Arc<Flow>,
}

impl Default for Grafo {
    fn default() -> Self {
        Self::nuevo()
    }
}

impl Grafo {
    /// A graph with nothing to paint.
    pub fn nuevo() -> Self {
        Self { paradas: Vec::new(), cursor: 0, flujo: Arc::new(flujo_vacio()) }
    }

    /// Rebuilds the stops from the draft and stores its drawing flow.
    pub fn rehacer(&mut self, nombre: &str, def: &FlowDef) {
        let antes = self.parada().cloned();
        self.paradas = paradas(def);
        self.flujo = Arc::new(dibujo(nombre, def));
        // If the previous stop still exists, the cursor stays on it: editing a field must not
        // move the user around.
        self.cursor = antes
            .and_then(|p| self.paradas.iter().position(|q| *q == p))
            .unwrap_or_else(|| self.cursor.min(self.paradas.len().saturating_sub(1)));
    }

    /// Nothing to paint? A YAML that could not be read, or a flow without `start`.
    pub fn vacio(&self) -> bool {
        self.paradas.is_empty()
    }

    pub fn parada(&self) -> Option<&Parada> {
        self.paradas.get(self.cursor)
    }

    /// The node the inspector looks at.
    pub fn nodo(&self) -> Option<&str> {
        self.parada().and_then(Parada::nodo)
    }

    /// The selected port, if the stop is a port.
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

    /// Which node must be marked: the one the cursor points at, and if the cursor is elsewhere, the
    /// node to blame for the validation error (spec §3: "the guilty node is pointed out").
    pub fn marcado(&self, def: &FlowDef, culpable: Option<&str>) -> Option<String> {
        match culpable {
            Some(n) if self.nodo() != Some(n) && def.nodes.contains_key(n) => Some(n.to_string()),
            _ => self.nodo().map(str::to_string),
        }
    }

    /// The stops in drawing order; the editor walks them with the cursor.
    pub fn paradas(&self) -> &[Parada] {
        &self.paradas
    }
}

/// The flow that is painted, built from the draft **without judging it**: while the flow does not validate, the
/// graph keeps showing what is there (and what has just been added). The judge is
/// `jevalan_flow::parse` (D6), not this.
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

/// A `NodeDef` as a drawing node, with the flow's defaults where there are any.
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
        // The ranges and `route` rules are computed by `validate`; painting does not need them.
        ranges: Vec::new(),
        questions: def.questions.iter().map(|(q, d)| (q.clone(), nodo(d, defaults))).collect(),
        route: def.route.iter().map(|r| (r.when.clone(), r.to.clone())).collect(),
        else_branch: def.else_branch.clone(),
    }
}

/// Ports the editor offers to wire on a node: the answer ones its type admits (the same
/// table `validate` uses, D6), plus the common ones. Neither a `rule` nor a `batch` admits `low-confidence`.
pub fn puertos(def: &NodeDef) -> Vec<String> {
    let mut puertos = answer_ports(def);
    puertos.push(ERROR.to_string());
    if !matches!(def.ty, NodeType::Rule | NodeType::Batch) {
        puertos.push(LOW_CONFIDENCE.to_string());
    }
    puertos
}

/// The stops, in drawing order: each node with its ports behind it, and then the outputs.
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

/// A flow with nothing, for the graph of a document that could not be read.
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

    /// The draft with a loose `noul`, like the one the editor adds: unwired.
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
        // A `rule` does not admit `low-confidence`; a `batch` only wires its `error`.
        assert!(!paradas.contains(&Parada::Puerto { nodo: "rm_root".into(), puerto: "low-confidence".into() }));
        let del_batch: Vec<&Parada> = paradas.iter().filter(|p| p.nodo() == Some("risks")).collect();
        assert_eq!(del_batch.len(), 2, "el nodo y su puerto de error: {del_batch:?}");
        // The outputs wired by an `out` are there; those that only appear in a `batch`'s `route` are not
        // drawing edges (the painter only wires `out`) and are seen in the inspector.
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

        // And the cursor reaches it with the arrows.
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
        // The cursor, on the loose node; and the culprit, `rm_root`.
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
        // The loose one travels with the flow's backend and without anyone judging it: meanwhile, the
        // bottom band is the one that says the draft is not valid.
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
