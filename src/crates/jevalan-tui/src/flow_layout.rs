//! Column layout of a flow's graph: one column per depth. Pure logic.

use jevalan_flow::Flow;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FlowNodeId {
    Node(String),
    Output(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub col: u16,
    pub row: u16,
}

fn id(flow: &Flow, name: &str) -> FlowNodeId {
    if flow.nodes.contains_key(name) { FlowNodeId::Node(name.into()) } else { FlowNodeId::Output(name.into()) }
}

/// Column = longest path from `start`, so every edge goes to the right.
pub fn columns(flow: &Flow) -> Vec<(FlowNodeId, Slot)> {
    fn dfs<'a>(n: &'a str, flow: &'a Flow, seen: &mut BTreeSet<&'a str>, post: &mut Vec<&'a str>) {
        if !seen.insert(n) {
            return;
        }
        if let Some(node) = flow.nodes.get(n) {
            for t in node.out.values() {
                dfs(t, flow, seen, post);
            }
        }
        post.push(n);
    }
    let mut post = Vec::new();
    dfs(&flow.start, flow, &mut BTreeSet::new(), &mut post);
    let alcanzables: Vec<&str> = post.into_iter().rev().collect();

    // An unreachable node does not come out of the traversal. A valid flow has none (`validate`
    // reports them), but the editor's draft does: a newly added node, still unwired, has to be seen
    // in order to wire it. It goes to a column of its own at the end, in name order.
    let vistos: BTreeSet<&str> = alcanzables.iter().copied().collect();
    let mut topo = alcanzables;
    topo.extend(flow.nodes.keys().map(String::as_str).filter(|n| !vistos.contains(n)));

    let mut depth: BTreeMap<&str, u16> = BTreeMap::new();
    depth.insert(flow.start.as_str(), 0);
    for n in &topo {
        // The unreachable one has no depth yet: it will be given the last one, as soon as it is known which that is.
        let Some(d) = depth.get(*n).copied() else { continue };
        if let Some(node) = flow.nodes.get(*n) {
            for t in node.out.values() {
                let e = depth.entry(t.as_str()).or_insert(0);
                *e = (*e).max(d + 1);
            }
        }
    }
    let ultima = depth.values().copied().max().map_or(0, |m| m + 1);
    for n in &topo {
        depth.entry(*n).or_insert(ultima);
    }

    let mut rows: BTreeMap<u16, u16> = BTreeMap::new();
    topo.iter()
        .map(|n| {
            let col = depth[n];
            let row = rows.entry(col).or_insert(0);
            let slot = Slot { col, row: *row };
            *row += 1;
            (id(flow, n), slot)
        })
        .collect()
}

/// Aristas `(origen, puerto, destino)` en orden estable.
pub fn edges(flow: &Flow) -> Vec<(FlowNodeId, String, FlowNodeId)> {
    flow.nodes
        .iter()
        .flat_map(|(name, node)| {
            node.out.iter().map(move |(port, target)| (FlowNodeId::Node(name.clone()), port.clone(), id(flow, target)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> Flow {
        jevalan_flow::parse(
            "agent-guard",
            include_str!("../../../../config/flows/agent-guard.yaml"),
            &["typesafe", "ollama"],
            "ollama",
        )
        .unwrap()
    }

    fn slot_of(cols: &[(FlowNodeId, Slot)], id: &FlowNodeId) -> Slot {
        cols.iter().find(|(n, _)| n == id).map(|(_, s)| *s).unwrap_or_else(|| panic!("{id:?} sin colocar"))
    }

    #[test]
    fn start_is_in_column_zero_and_depth_is_the_longest_path() {
        let cols = columns(&guard());
        assert_eq!(slot_of(&cols, &FlowNodeId::Node("destructive".into())), Slot { col: 0, row: 0 });
        assert_eq!(slot_of(&cols, &FlowNodeId::Node("risk".into())).col, 2);
        // allow hangs from kind (1) and from risk (2): the longest path rules.
        assert_eq!(slot_of(&cols, &FlowNodeId::Output("allow".into())).col, 3);
    }

    #[test]
    fn every_edge_goes_right() {
        let flow = guard();
        let cols = columns(&flow);
        for (from, port, to) in edges(&flow) {
            assert!(slot_of(&cols, &from).col < slot_of(&cols, &to).col, "{from:?} -{port}-> {to:?}");
        }
    }

    #[test]
    fn no_two_items_share_a_slot() {
        let cols = columns(&guard());
        let slots: BTreeSet<(u16, u16)> = cols.iter().map(|(_, s)| (s.col, s.row)).collect();
        assert_eq!(slots.len(), cols.len());
    }

    #[test]
    fn edges_name_their_port() {
        let e = edges(&guard());
        assert!(e.contains(&(
            FlowNodeId::Node("destructive".into()),
            "true".into(),
            FlowNodeId::Output("deny".into())
        )));
        assert_eq!(e.len(), 3 + 4 + 2);
    }

    #[test]
    fn layout_is_deterministic() {
        assert_eq!(columns(&guard()), columns(&guard()));
    }

    /// A drawing flow with a loose node, like the one the editor builds from a half-finished draft.
    fn con_suelto() -> Flow {
        let flow = guard();
        let mut nodes = flow.nodes.clone();
        let mut suelto = flow.nodes["destructive"].clone();
        suelto.out = [("true".to_string(), "allow".to_string())].into();
        nodes.insert("suelto".into(), suelto);
        Flow { nodes, ..flow }
    }

    #[test]
    fn un_nodo_inalcanzable_tiene_columna_propia() {
        let flow = con_suelto();
        let cols = columns(&flow);
        let suelto = slot_of(&cols, &FlowNodeId::Node("suelto".into()));
        let destructivo = slot_of(&cols, &FlowNodeId::Node("destructive".into()));
        assert!(suelto.col > destructivo.col, "el suelto va a una columna propia al final: {cols:?}");
        assert_eq!(cols.iter().filter(|(id, _)| *id == FlowNodeId::Node("suelto".into())).count(), 1);
        // And there is still a place for everything.
        let slots: BTreeSet<(u16, u16)> = cols.iter().map(|(_, s)| (s.col, s.row)).collect();
        assert_eq!(slots.len(), cols.len());
    }
}
