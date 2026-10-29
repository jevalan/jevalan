//! Per-flow activity: traversals of each edge, last answer of each node and packets in flight. Pure logic.

use jevalan_events::{Event, EventKind};
use jevalan_flow::Flow;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Life of a packet on an edge.
pub const PACKET_TTL: Duration = Duration::from_millis(800);
/// Cap on simultaneous packets; once exceeded, the oldest are dropped.
const MAX_PACKETS: usize = 256;

#[derive(Debug, Default)]
struct NodeStats {
    ports: BTreeMap<String, u64>,
    last: Option<(Value, f64)>,
}

#[derive(Debug)]
struct Packet {
    flow: String,
    node: String,
    port: String,
    born: Instant,
}

#[derive(Debug, Default)]
pub struct FlowActivity {
    flows: BTreeMap<String, BTreeMap<String, NodeStats>>,
    packets: Vec<Packet>,
}

impl FlowActivity {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `NODE_END` (counter, last answer, packet) and forgets the flow on `FLOW_REMOVED`.
    /// Events of unknown flows or nodes are ignored.
    pub fn on_event(&mut self, flows: &BTreeMap<String, Arc<Flow>>, e: &Event, now: Instant) {
        match &e.kind {
            EventKind::NodeEnd { flow, node, answer, confidence, port } => {
                if !flows.get(flow).is_some_and(|f| f.nodes.contains_key(node)) {
                    return;
                }
                let stats = self.flows.entry(flow.clone()).or_default().entry(node.clone()).or_default();
                *stats.ports.entry(port.clone()).or_default() += 1;
                stats.last = Some((answer.clone(), *confidence));
                if self.packets.len() == MAX_PACKETS {
                    self.packets.remove(0);
                }
                self.packets.push(Packet { flow: flow.clone(), node: node.clone(), port: port.clone(), born: now });
            }
            EventKind::FlowRemoved { flow } => {
                self.flows.remove(flow);
                self.packets.retain(|p| p.flow != *flow);
            }
            _ => {}
        }
    }

    /// Forgets all per-flow state (on reconnecting, the daemon resends the current flows).
    pub fn clear_flows(&mut self) {
        self.flows.clear();
        self.packets.clear();
    }

    /// Removes the packets whose life has expired.
    pub fn tick(&mut self, now: Instant) {
        self.packets.retain(|p| now.saturating_duration_since(p.born) < PACKET_TTL);
    }

    pub fn has_packets(&self) -> bool {
        !self.packets.is_empty()
    }

    pub fn edge_count(&self, flow: &str, node: &str, port: &str) -> u64 {
        self.stats(flow, node).and_then(|s| s.ports.get(port)).copied().unwrap_or(0)
    }

    /// Times the node has finished, through any port.
    pub fn node_count(&self, flow: &str, node: &str) -> u64 {
        self.stats(flow, node).map_or(0, |s| s.ports.values().sum())
    }

    pub fn last_answer(&self, flow: &str, node: &str) -> Option<(&Value, f64)> {
        self.stats(flow, node)?.last.as_ref().map(|(a, c)| (a, *c))
    }

    /// Paquetes vivos del flujo: `(nodo, puerto, progreso 0..1)`.
    pub fn packets<'a>(&'a self, flow: &'a str, now: Instant) -> impl Iterator<Item = (&'a str, &'a str, f64)> + 'a {
        self.packets.iter().filter(move |p| p.flow == flow).filter_map(move |p| {
            let t = now.saturating_duration_since(p.born).as_secs_f64() / PACKET_TTL.as_secs_f64();
            (t < 1.0).then_some((p.node.as_str(), p.port.as_str(), t))
        })
    }

    fn stats(&self, flow: &str, node: &str) -> Option<&NodeStats> {
        self.flows.get(flow)?.get(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevalan_events::new_event;
    use serde_json::json;

    fn flows() -> BTreeMap<String, Arc<Flow>> {
        crate::fake::flows().into_iter().map(|f| (f.name.clone(), Arc::new(f))).collect()
    }

    fn node_end(flow: &str, node: &str, answer: Value, confidence: f64, port: &str) -> Event {
        let kind = EventKind::NodeEnd { flow: flow.into(), node: node.into(), answer, confidence, port: port.into() };
        new_event("t", 1, "test", kind)
    }

    #[test]
    fn node_end_counts_the_edge_and_launches_a_packet() {
        let (mut act, flows, t0) = (FlowActivity::new(), flows(), Instant::now());
        act.on_event(&flows, &node_end("agent-guard", "kind", json!("shell"), 0.9, "shell"), t0);
        act.on_event(&flows, &node_end("agent-guard", "kind", json!("shell"), 0.8, "shell"), t0);
        assert_eq!(act.edge_count("agent-guard", "kind", "shell"), 2);
        assert_eq!(act.edge_count("agent-guard", "kind", "research"), 0);
        assert_eq!(act.node_count("agent-guard", "kind"), 2);
        let half = t0 + PACKET_TTL / 2;
        let packets: Vec<_> = act.packets("agent-guard", half).collect();
        assert_eq!(packets.len(), 2);
        assert_eq!((packets[0].0, packets[0].1), ("kind", "shell"));
        assert!((packets[0].2 - 0.5).abs() < 1e-9);
        assert_eq!(act.packets("model-route", half).count(), 0);
    }

    #[test]
    fn packets_expire_after_ttl() {
        let (mut act, flows, t0) = (FlowActivity::new(), flows(), Instant::now());
        act.on_event(&flows, &node_end("agent-guard", "kind", json!("shell"), 0.9, "shell"), t0);
        act.tick(t0 + PACKET_TTL / 2);
        assert!(act.has_packets());
        assert_eq!(act.packets("agent-guard", t0 + PACKET_TTL).count(), 0);
        act.tick(t0 + PACKET_TTL);
        assert!(!act.has_packets());
        assert_eq!(act.edge_count("agent-guard", "kind", "shell"), 1, "el contador sobrevive al paquete");
    }

    #[test]
    fn last_answer_is_kept() {
        let (mut act, flows, t0) = (FlowActivity::new(), flows(), Instant::now());
        assert_eq!(act.last_answer("agent-guard", "risk"), None);
        act.on_event(&flows, &node_end("agent-guard", "risk", json!(0.5), 0.7, "0..1.5"), t0);
        act.on_event(&flows, &node_end("agent-guard", "risk", json!(2.0), 0.6, "1.5..3"), t0);
        assert_eq!(act.last_answer("agent-guard", "risk"), Some((&json!(2.0), 0.6)));
    }

    #[test]
    fn unknown_flow_is_ignored() {
        let (mut act, flows, t0) = (FlowActivity::new(), flows(), Instant::now());
        act.on_event(&flows, &node_end("fantasma", "kind", json!("x"), 0.9, "x"), t0);
        act.on_event(&flows, &node_end("agent-guard", "fantasma", json!("x"), 0.9, "x"), t0);
        assert!(!act.has_packets());
        assert_eq!(act.edge_count("fantasma", "kind", "x"), 0);
        assert_eq!(act.last_answer("agent-guard", "fantasma"), None);
    }

    #[test]
    fn flow_removed_clears_state() {
        let (mut act, flows, t0) = (FlowActivity::new(), flows(), Instant::now());
        act.on_event(&flows, &node_end("agent-guard", "kind", json!("shell"), 0.9, "shell"), t0);
        act.on_event(&flows, &node_end("model-route", "tier", json!(1.0), 0.9, "x"), t0);
        act.on_event(&flows, &new_event("t", 2, "test", EventKind::FlowRemoved { flow: "agent-guard".into() }), t0);
        assert_eq!(act.edge_count("agent-guard", "kind", "shell"), 0);
        assert_eq!(act.last_answer("agent-guard", "kind"), None);
        assert_eq!(act.packets("agent-guard", t0).count(), 0);
        assert!(act.has_packets(), "el otro flujo conserva su paquete");
    }
}
