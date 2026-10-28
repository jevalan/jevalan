use super::model::{
    BackendState, CLIENT_GONE, CapabilityInfo, Cue, DOWN_AFTER, MAX_OPEN_TRACES, NodeId, PASSTHROUGH, Packet,
    PacketStatus, Route, Topology, progress,
};
use jevalan_events::{Endpoint, Event, EventKind};
use jevalan_flow::RULE_BACKEND;
use serde_json::Value;
use std::time::Instant;

impl Topology {
    /// Aplica un evento nuevo. Misma secuencia de (evento, now) → mismo modelo y mismos `Cue`.
    pub fn reduce(&mut self, e: &Event, now: Instant) -> Vec<Cue> {
        let mut cues = Vec::new();
        match &e.kind {
            EventKind::RequestReceived { client, endpoint, flow, backend } => {
                if self.clients.insert(client.clone(), now).is_none() {
                    cues.push(Cue::ClientJoined(client.clone()));
                }
                self.refresh_visible();
                let port = match (endpoint, flow) {
                    (Endpoint::Decide, Some(flow)) => flow.clone(),
                    _ => PASSTHROUGH.to_string(),
                };
                self.open_trace(&e.trace_id, client, port, false);
                if let Some(backend) = backend {
                    self.ensure_backend(backend);
                }
                self.send(e, NodeId::Client(client.clone()), NodeId::Hub, PacketStatus::Ok, now);
            }
            EventKind::BackendRequest { backend, .. } => {
                self.ensure_backend(backend);
                if let Some(port) = self.trace_port(&e.trace_id) {
                    self.port_backend_edges.insert((port.clone(), backend.clone()));
                    self.send(e, NodeId::Port(port), NodeId::Backend(backend.clone()), PacketStatus::Ok, now);
                }
            }
            EventKind::BackendResponse { backend, ok, .. } => {
                if let Some(port) = self.trace_port(&e.trace_id) {
                    let status = if *ok { PacketStatus::Ok } else { PacketStatus::Failed };
                    self.send(e, NodeId::Backend(backend.clone()), NodeId::Port(port), status, now);
                }
                self.ensure_backend(backend);
                if *ok {
                    self.failures.remove(backend);
                    self.backends.insert(backend.clone(), BackendState::Up);
                } else {
                    let fails = self.failures.entry(backend.clone()).or_default();
                    *fails += 1;
                    if *fails >= DOWN_AFTER
                        && self.backends.insert(backend.clone(), BackendState::Down) != Some(BackendState::Down)
                    {
                        cues.push(Cue::BackendDown(backend.clone()));
                    }
                }
            }
            // En una traza del hub la respuesta cierra la traza; el flujo solo delata a quién sirve.
            EventKind::DecisionMade { flow, .. } | EventKind::NodeStart { flow, .. }
                if self.is_hub_trace(&e.trace_id) =>
            {
                self.learn_served(&e.trace_id, flow);
            }
            EventKind::DecisionMade { flow, outcome, .. } => {
                if outcome == "deny" {
                    cues.push(Cue::Denied(flow.clone()));
                }
                self.last_status.insert(flow.clone(), PacketStatus::Outcome(outcome.clone()));
                self.finish(e, PacketStatus::Outcome(outcome.clone()), now);
            }
            EventKind::HubRequest { capability, client, .. } => {
                if self.clients.insert(client.clone(), now).is_none() {
                    cues.push(Cue::ClientJoined(client.clone()));
                }
                self.refresh_visible();
                self.open_trace(&e.trace_id, client, capability.clone(), true);
                self.send(e, NodeId::Client(client.clone()), NodeId::Hub, PacketStatus::Ok, now);
            }
            EventKind::HubResponse { capability, status, .. } => {
                let status = PacketStatus::Hub(status.clone());
                if self.capabilities.contains_key(capability) {
                    self.last_status.insert(capability.clone(), status.clone());
                }
                if status == PacketStatus::Hub("error".into()) {
                    cues.push(Cue::Error("hub".into()));
                }
                self.finish(e, status, now);
            }
            EventKind::CapabilityLoaded {
                capability,
                version,
                provider,
                provider_type,
                flow,
                description,
                input_fields,
                decision_fields,
            } => {
                if provider_type != "flow" {
                    self.ensure_backend(provider);
                }
                // El flujo anunciado manda; solo sin él se conserva lo aprendido del tráfico.
                match flow {
                    Some(f) => {
                        self.served.insert(capability.clone(), f.clone());
                    }
                    None if provider_type != "flow" => {
                        self.served.remove(capability);
                    }
                    None => {}
                }
                self.invalid_caps.remove(capability);
                let flow = self.served.get(capability).cloned();
                let info = CapabilityInfo {
                    version: version.clone(),
                    provider: provider.clone(),
                    kind: provider_type.clone(),
                    flow,
                    description: description.clone(),
                    input_fields: input_fields.clone(),
                    decision_fields: decision_fields.clone(),
                };
                if let Some(old) = self.capabilities.insert(capability.clone(), info) {
                    self.drop_orphan_provider(&old);
                }
                self.rebuild_ports();
            }
            EventKind::CapabilityRemoved { capability } => {
                if let Some(old) = self.capabilities.remove(capability) {
                    self.drop_orphan_provider(&old);
                }
                self.last_status.remove(capability);
                self.served.remove(capability);
                self.invalid_caps.remove(capability);
                self.rebuild_ports();
            }
            EventKind::CapabilityInvalid { capability, .. } => {
                if self.capabilities.contains_key(capability) {
                    self.invalid_caps.insert(capability.clone());
                }
            }
            EventKind::PassthroughDone { status, .. } => {
                let status = if (200..300).contains(status) { PacketStatus::Ok } else { PacketStatus::Failed };
                self.last_status.insert(PASSTHROUGH.to_string(), status.clone());
                self.finish(e, status, now);
            }
            EventKind::Error { stage, .. } => {
                cues.push(Cue::Error(stage.clone()));
                if !self.is_hub_trace(&e.trace_id) {
                    if let Some(port) = self.trace_port(&e.trace_id) {
                        self.last_status.insert(port, PacketStatus::Failed);
                    }
                    self.finish(e, PacketStatus::Failed, now);
                }
            }
            EventKind::Backends { backends } => backends.iter().for_each(|b| self.ensure_backend(b)),
            EventKind::FlowLoaded { flow, definition, .. } => {
                self.loaded_flows.insert(flow.clone());
                self.rebuild_ports();
                let nodes = definition.get("nodes").and_then(Value::as_object);
                for backend in nodes
                    .into_iter()
                    .flat_map(|n| n.values())
                    .filter_map(|n| n["backend"].as_str())
                    .filter(|b| *b != RULE_BACKEND)
                {
                    self.ensure_backend(backend);
                }
            }
            EventKind::FlowRemoved { flow } => {
                self.loaded_flows.remove(flow);
                if flow != PASSTHROUGH && !self.capabilities.contains_key(flow) {
                    self.last_status.remove(flow);
                }
                self.rebuild_ports();
            }
            EventKind::NodeStart { .. }
            | EventKind::NodeEnd { .. }
            | EventKind::NodeAudit { .. }
            | EventKind::FlowInvalid { .. }
            | EventKind::Lagged { .. } => {}
        }
        cues
    }

    /// Olvida los puertos de flujo; `passthrough` y las capacidades se quedan.
    pub fn clear_flow_ports(&mut self) {
        self.loaded_flows.clear();
        self.last_status.retain(|p, _| p == PASSTHROUGH || self.capabilities.contains_key(p));
        self.rebuild_ports();
    }

    /// Olvida las capacidades y los chips de sus especialistas; lo aprendido de a quién sirve cada flujo se conserva.
    pub fn clear_capabilities(&mut self) {
        self.invalid_caps.clear();
        for old in std::mem::take(&mut self.capabilities).values() {
            self.drop_orphan_provider(old);
        }
        self.last_status.retain(|p, _| p == PASSTHROUGH || self.loaded_flows.contains(p));
        self.rebuild_ports();
    }

    /// Caduca paquetes y despide a los clientes que llevan `CLIENT_GONE` sin hacer nada.
    pub fn tick(&mut self, now: Instant) -> Vec<Cue> {
        self.packets.retain(|p| progress(p, now) < 1.0);
        let gone: Vec<String> = self
            .clients
            .iter()
            .filter(|(_, last)| now.saturating_duration_since(**last) >= CLIENT_GONE)
            .map(|(n, _)| n.clone())
            .collect();
        for name in &gone {
            self.clients.remove(name);
        }
        if !gone.is_empty() {
            self.refresh_visible();
        }
        gone.into_iter().map(Cue::ClientLeft).collect()
    }

    fn ensure_backend(&mut self, name: &str) {
        self.backends.entry(name.to_string()).or_insert(BackendState::Up);
    }

    /// Registra la traza; si ya hay `MAX_OPEN_TRACES`, expulsa las más antiguas.
    fn open_trace(&mut self, trace_id: &str, client: &str, port: String, hub: bool) {
        if !self.trace_routes.contains_key(trace_id) {
            while self.trace_routes.len() >= MAX_OPEN_TRACES {
                let Some(old) = self.route_order.pop_front() else { break };
                self.trace_routes.remove(&old);
            }
            // ponytail: las trazas cerradas dejan su id en el orden; se purga en bloque, no en cada cierre
            if self.route_order.len() >= 2 * MAX_OPEN_TRACES {
                self.route_order.retain(|id| self.trace_routes.contains_key(id));
            }
            self.route_order.push_back(trace_id.to_string());
        }
        self.trace_routes.insert(trace_id.to_string(), Route { client: client.to_string(), port, hub });
    }

    fn trace_port(&self, trace_id: &str) -> Option<String> {
        self.trace_routes.get(trace_id).map(|r| r.port.clone())
    }

    fn is_hub_trace(&self, trace_id: &str) -> bool {
        self.trace_routes.get(trace_id).is_some_and(|r| r.hub)
    }

    /// La capacidad de una traza del hub la sirve `flow`: el flujo deja de tener puerto propio.
    fn learn_served(&mut self, trace_id: &str, flow: &str) {
        let Some(cap) = self.trace_port(trace_id) else { return };
        let Some(info) = self.capabilities.get_mut(&cap) else { return };
        if info.kind == "flow" && info.flow.as_deref() != Some(flow) {
            info.flow = Some(flow.to_string());
            self.served.insert(cap, flow.to_string());
            self.rebuild_ports();
        }
    }

    /// Retira el chip del especialista de una capacidad que se va, si nadie más lo usa.
    fn drop_orphan_provider(&mut self, old: &CapabilityInfo) {
        if old.kind == "flow" || self.is_provider(&old.provider) {
            return;
        }
        self.backends.remove(&old.provider);
        self.failures.remove(&old.provider);
        self.port_backend_edges.retain(|(_, b)| *b != old.provider);
    }

    /// Cierra la traza: devuelve el resultado al cliente que preguntó, si se le conoce.
    fn finish(&mut self, e: &Event, status: PacketStatus, now: Instant) {
        if let Some(route) = self.trace_routes.remove(&e.trace_id) {
            self.send(e, NodeId::Hub, NodeId::Client(route.client), status, now);
        }
    }

    fn send(&mut self, e: &Event, from: NodeId, to: NodeId, status: PacketStatus, now: Instant) {
        self.packets.push(Packet { trace_id: e.trace_id.clone(), from, to, started: now, status });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{CLIENT_IDLE, ClientState, MAX_OPEN_TRACES, MAX_VISIBLE_CLIENTS, PACKET_TTL};
    use jevalan_events::SCHEMA_VERSION;
    use serde_json::json;
    use std::time::Duration;

    fn ev(trace: &str, kind: EventKind) -> Event {
        Event {
            v: SCHEMA_VERSION,
            id: "x".into(),
            ts: String::new(),
            trace_id: trace.into(),
            seq: 1,
            source: "test".into(),
            kind,
        }
    }

    fn req(trace: &str, client: &str) -> Event {
        let kind = EventKind::RequestReceived {
            client: client.into(),
            endpoint: Endpoint::Decide,
            flow: Some("agent-guard".into()),
            backend: None,
        };
        ev(trace, kind)
    }

    fn backend_req(trace: &str, backend: &str) -> Event {
        ev(trace, EventKind::BackendRequest { backend: backend.into(), model: "m".into(), questions: 1 })
    }

    fn backend_resp(trace: &str, backend: &str, ok: bool) -> Event {
        let kind = EventKind::BackendResponse {
            backend: backend.into(),
            ok,
            status: if ok { 200 } else { 0 },
            duration_ms: 50,
            input_tokens: None,
        };
        ev(trace, kind)
    }

    fn decision(trace: &str, outcome: &str) -> Event {
        let kind = EventKind::DecisionMade {
            flow: "agent-guard".into(),
            outcome: outcome.into(),
            path_len: 1,
            duration_ms: 10,
        };
        ev(trace, kind)
    }

    fn loaded(flow: &str, backends: &[&str]) -> Event {
        let nodes: serde_json::Map<String, Value> =
            backends.iter().enumerate().map(|(i, b)| (format!("n{i}"), json!({ "backend": b }))).collect();
        let kind = EventKind::FlowLoaded {
            flow: flow.into(),
            nodes: backends.len() as u32,
            definition: json!({ "nodes": nodes }),
        };
        ev(&format!("flow:{flow}:1"), kind)
    }

    fn last(t: &Topology) -> &Packet {
        t.packets().last().expect("hay paquete")
    }

    fn route(p: &Packet) -> (NodeId, NodeId) {
        (p.from.clone(), p.to.clone())
    }

    #[test]
    fn backends_event_adds_chips() {
        let mut t = Topology::new();
        let kind = EventKind::Backends { backends: vec!["ollama".into(), "typesafe".into()] };
        assert!(t.reduce(&ev("backends:1", kind.clone()), Instant::now()).is_empty());
        let names: Vec<_> = t.backends().collect();
        assert_eq!(names, [("ollama", BackendState::Up), ("typesafe", BackendState::Up)]);
        t.reduce(&backend_resp("t", "ollama", false), Instant::now());
        t.reduce(&ev("backends:1", kind), Instant::now());
        assert_eq!(t.backends().count(), 2, "repetirlo no duplica");
    }

    #[test]
    fn request_spawns_client_and_inbound_packet() {
        let mut t = Topology::new();
        let now = Instant::now();
        let cues = t.reduce(&req("a", "crush"), now);
        assert_eq!(cues, vec![Cue::ClientJoined("crush".into())]);
        assert_eq!(t.visible_clients(), ["crush"]);
        assert_eq!(route(last(&t)), (NodeId::Client("crush".into()), NodeId::Hub));
        assert!(t.reduce(&req("b", "crush"), now).is_empty(), "un cliente conocido no vuelve a entrar");
    }

    #[test]
    fn request_then_backend_then_decision_moves_packets() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&loaded("agent-guard", &["typesafe"]), now);
        t.reduce(&req("a", "vscode"), now);
        t.reduce(&backend_req("a", "typesafe"), now);
        assert_eq!(route(last(&t)), (NodeId::Port("agent-guard".into()), NodeId::Backend("typesafe".into())));
        assert_eq!(t.port_backend_edges().collect::<Vec<_>>(), vec![("agent-guard", "typesafe")]);
        t.reduce(&backend_resp("a", "typesafe", true), now);
        assert_eq!(route(last(&t)), (NodeId::Backend("typesafe".into()), NodeId::Port("agent-guard".into())));
        assert_eq!(last(&t).status, PacketStatus::Ok);
        t.reduce(&decision("a", "allow"), now);
        assert_eq!(route(last(&t)), (NodeId::Hub, NodeId::Client("vscode".into())));
        assert_eq!(last(&t).status, PacketStatus::Outcome("allow".into()));
        let before = t.packets().len();
        t.reduce(&decision("a", "allow"), now);
        assert_eq!(t.packets().len(), before, "la traza ya está olvidada");
    }

    #[test]
    fn port_keeps_last_outcome() {
        let mut t = Topology::new();
        let now = Instant::now();
        assert_eq!(t.port_status("agent-guard"), None);
        t.reduce(&req("a", "crush"), now);
        t.reduce(&decision("a", "allow"), now);
        t.tick(now + PACKET_TTL * 2);
        assert_eq!(t.port_status("agent-guard"), Some(&PacketStatus::Outcome("allow".into())));
        t.reduce(&req("b", "crush"), now);
        t.reduce(&ev("b", EventKind::Error { stage: "backend".into(), message: "x".into() }), now);
        assert_eq!(t.port_status("agent-guard"), Some(&PacketStatus::Failed));
    }

    #[test]
    fn passthrough_port_keeps_last_status() {
        let mut t = Topology::new();
        let now = Instant::now();
        let r = EventKind::RequestReceived {
            client: "crush".into(),
            endpoint: Endpoint::Systemone,
            flow: None,
            backend: Some("ollama".into()),
        };
        let done =
            |status| EventKind::PassthroughDone { backend: "ollama".into(), model: "m".into(), status, duration_ms: 3 };
        t.reduce(&ev("p", r.clone()), now);
        t.reduce(&ev("p", done(200)), now);
        assert_eq!(t.port_status(PASSTHROUGH), Some(&PacketStatus::Ok));
        t.reduce(&ev("q", r), now);
        t.reduce(&ev("q", done(502)), now);
        assert_eq!(t.port_status(PASSTHROUGH), Some(&PacketStatus::Failed));
    }

    #[test]
    fn last_status_cleared_with_port() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&loaded("agent-guard", &[]), now);
        t.reduce(&req("a", "crush"), now);
        t.reduce(&decision("a", "deny"), now);
        t.reduce(&ev("f", EventKind::FlowRemoved { flow: "agent-guard".into() }), now);
        assert_eq!(t.port_status("agent-guard"), None);
        t.reduce(&decision("z", "allow"), now);
        t.clear_flow_ports();
        assert_eq!(t.port_status("agent-guard"), None);
    }

    #[test]
    fn passthrough_uses_its_port() {
        let mut t = Topology::new();
        let now = Instant::now();
        let r = EventKind::RequestReceived {
            client: "crush".into(),
            endpoint: Endpoint::Systemone,
            flow: None,
            backend: Some("ollama".into()),
        };
        t.reduce(&ev("p", r), now);
        assert!(t.backends().any(|(n, _)| n == "ollama"));
        t.reduce(&backend_req("p", "ollama"), now);
        assert_eq!(route(last(&t)), (NodeId::Port(PASSTHROUGH.into()), NodeId::Backend("ollama".into())));
        t.reduce(&backend_resp("p", "ollama", true), now);
        let done =
            EventKind::PassthroughDone { backend: "ollama".into(), model: "m".into(), status: 200, duration_ms: 9 };
        t.reduce(&ev("p", done), now);
        assert_eq!(route(last(&t)), (NodeId::Hub, NodeId::Client("crush".into())));
        assert_eq!(last(&t).status, PacketStatus::Ok);
    }

    #[test]
    fn three_failures_bring_a_backend_down_and_ok_brings_it_up() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&req("a", "c"), now);
        t.reduce(&backend_req("a", "ollama"), now);
        assert!(t.reduce(&backend_resp("a", "ollama", false), now).is_empty());
        assert_eq!(last(&t).status, PacketStatus::Failed);
        assert!(t.reduce(&backend_resp("a", "ollama", false), now).is_empty());
        assert_eq!(t.reduce(&backend_resp("a", "ollama", false), now), vec![Cue::BackendDown("ollama".into())]);
        assert!(t.reduce(&backend_resp("a", "ollama", false), now).is_empty(), "ya estaba caído");
        assert!(t.backends().any(|(n, s)| n == "ollama" && s == BackendState::Down));
        t.reduce(&backend_resp("a", "ollama", true), now);
        assert!(t.backends().any(|(n, s)| n == "ollama" && s == BackendState::Up));
        assert!(t.reduce(&backend_resp("a", "ollama", false), now).is_empty(), "el contador se reinició");
    }

    #[test]
    fn flow_loaded_adds_port_and_backends() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&loaded("model-route", &["ollama"]), now);
        t.reduce(&loaded("agent-guard", &["typesafe", "ollama"]), now);
        assert_eq!(t.ports(), ["agent-guard", "model-route", PASSTHROUGH]);
        let names: Vec<&str> = t.backends().map(|(n, _)| n).collect();
        assert_eq!(names, ["ollama", "typesafe"]);
        t.reduce(&loaded("agent-guard", &["typesafe"]), now);
        assert_eq!(t.ports().len(), 3, "recargar no duplica el puerto");
    }

    #[test]
    fn flow_removed_drops_the_port() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&loaded("agent-guard", &[]), now);
        t.reduce(&ev("r", EventKind::FlowRemoved { flow: "agent-guard".into() }), now);
        assert_eq!(t.ports(), [PASSTHROUGH]);
        t.reduce(&ev("r", EventKind::FlowRemoved { flow: PASSTHROUGH.into() }), now);
        assert_eq!(t.ports(), [PASSTHROUGH], "passthrough es fijo");
    }

    #[test]
    fn clear_flow_ports_keeps_passthrough() {
        let mut t = Topology::new();
        t.reduce(&loaded("agent-guard", &[]), Instant::now());
        t.clear_flow_ports();
        assert_eq!(t.ports(), [PASSTHROUGH]);
    }

    #[test]
    fn deny_outcome_cues_denied() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&req("a", "c"), now);
        assert_eq!(t.reduce(&decision("a", "deny"), now), vec![Cue::Denied("agent-guard".into())]);
        assert_eq!(last(&t).status, PacketStatus::Outcome("deny".into()));
        assert!(t.reduce(&decision("b", "allow"), now).is_empty());
    }

    #[test]
    fn error_is_cued_and_closes_the_trace() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&req("a", "c"), now);
        let cues = t.reduce(&ev("a", EventKind::Error { stage: "backend".into(), message: "m".into() }), now);
        assert_eq!(cues, vec![Cue::Error("backend".into())]);
        assert_eq!(route(last(&t)), (NodeId::Hub, NodeId::Client("c".into())));
        assert_eq!(last(&t).status, PacketStatus::Failed);
        let before = t.packets().len();
        t.reduce(&ev("zz", EventKind::Error { stage: "x".into(), message: "m".into() }), now);
        assert_eq!(t.packets().len(), before, "sin traza conocida no hay paquete");
    }

    #[test]
    fn packets_expire_after_ttl() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&req("a", "crush"), now);
        assert!((progress(last(&t), now + PACKET_TTL / 2) - 0.5).abs() < 1e-9);
        t.tick(now + PACKET_TTL / 2);
        assert!(t.has_live_packets());
        t.tick(now + PACKET_TTL);
        assert!(!t.has_live_packets());
    }

    #[test]
    fn idle_clients_grey_out_then_leave() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&req("a", "cron"), now);
        assert_eq!(t.client_state("cron", now), Some(ClientState::Active));
        assert_eq!(t.client_state("cron", now + CLIENT_IDLE), Some(ClientState::Idle));
        assert!(t.tick(now + CLIENT_IDLE).is_empty());
        assert_eq!(t.tick(now + CLIENT_GONE), vec![Cue::ClientLeft("cron".into())]);
        assert!(t.visible_clients().is_empty());
        assert_eq!(t.client_state("cron", now + CLIENT_GONE), None);
    }

    #[test]
    fn only_five_most_recent_clients_are_visible() {
        let mut t = Topology::new();
        let t0 = Instant::now();
        for i in 0..7u64 {
            t.reduce(&req(&format!("t{i}"), &format!("c{i}")), t0 + Duration::from_millis(i));
        }
        assert_eq!(t.visible_clients().len(), MAX_VISIBLE_CLIENTS);
        assert_eq!(t.overflow(), 2);
        assert_eq!(t.visible_clients()[0], "c6");
        assert_eq!(t.display(&NodeId::Client("c0".into())), NodeId::Overflow);
        assert_eq!(t.display(&NodeId::Client("c6".into())), NodeId::Client("c6".into()));
        assert_eq!(t.display(&NodeId::Hub), NodeId::Hub);
    }

    #[test]
    fn open_traces_are_bounded() {
        let mut t = Topology::new();
        let now = Instant::now();
        for i in 0..MAX_OPEN_TRACES + 10 {
            t.reduce(&req(&format!("t{i}"), "crush"), now);
        }
        assert!(t.trace_routes.len() <= MAX_OPEN_TRACES);
        assert!(!t.trace_routes.contains_key("t0"), "la traza más antigua se expulsa");
        assert!(t.trace_routes.contains_key(&format!("t{}", MAX_OPEN_TRACES + 9)));
    }

    fn cap_loaded(name: &str, provider: &str, kind: &str) -> Event {
        let kind = EventKind::CapabilityLoaded {
            capability: name.into(),
            version: "1".into(),
            provider: provider.into(),
            provider_type: kind.into(),
            flow: None,
            description: String::new(),
            input_fields: vec![],
            decision_fields: vec![],
        };
        ev(&format!("capability:{name}:1"), kind)
    }

    fn cap_served_by(name: &str, flow: &str) -> Event {
        let mut e = cap_loaded(name, "jevalan-flow", "flow");
        if let EventKind::CapabilityLoaded { flow: f, .. } = &mut e.kind {
            *f = Some(flow.into());
        }
        e
    }

    fn hub_req(trace: &str, capability: &str, provider: &str, client: &str) -> Event {
        let kind = EventKind::HubRequest {
            capability: capability.into(),
            capability_version: "1".into(),
            provider: provider.into(),
            client: client.into(),
        };
        ev(trace, kind)
    }

    fn hub_resp(trace: &str, capability: &str, provider: &str, status: &str) -> Event {
        let kind = EventKind::HubResponse {
            capability: capability.into(),
            provider: provider.into(),
            status: status.into(),
            code: (status == "error").then(|| "provider_failed".to_string()),
            elapsed_ms: 40,
        };
        ev(trace, kind)
    }

    fn node_start(trace: &str, flow: &str) -> Event {
        let kind = EventKind::NodeStart {
            flow: flow.into(),
            node: "n".into(),
            node_type: "noul".into(),
            backend: "typesafe".into(),
            model: "m".into(),
        };
        ev(trace, kind)
    }

    fn flow_decision(trace: &str, flow: &str) -> Event {
        let kind = EventKind::DecisionMade { flow: flow.into(), outcome: "ok".into(), path_len: 1, duration_ms: 9 };
        ev(trace, kind)
    }

    #[test]
    fn hub_request_and_response_move_packets() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&cap_loaded("tool.risk", "toolgate", "exec"), now);
        let cues = t.reduce(&hub_req("h", "tool.risk", "toolgate", "vscode"), now);
        assert_eq!(cues, vec![Cue::ClientJoined("vscode".into())]);
        assert_eq!(route(last(&t)), (NodeId::Client("vscode".into()), NodeId::Hub));
        t.reduce(&backend_req("h", "toolgate"), now);
        assert_eq!(route(last(&t)), (NodeId::Port("tool.risk".into()), NodeId::Backend("toolgate".into())));
        assert_eq!(t.port_backend_edges().collect::<Vec<_>>(), vec![("tool.risk", "toolgate")]);
        t.reduce(&backend_resp("h", "toolgate", true), now);
        assert_eq!(route(last(&t)), (NodeId::Backend("toolgate".into()), NodeId::Port("tool.risk".into())));
        for (i, status) in ["ok", "abstain", "error"].into_iter().enumerate() {
            let trace = format!("r{i}");
            t.reduce(&hub_req(&trace, "tool.risk", "toolgate", "vscode"), now);
            t.reduce(&hub_resp(&trace, "tool.risk", "toolgate", status), now);
            assert_eq!(route(last(&t)), (NodeId::Hub, NodeId::Client("vscode".into())));
            assert_eq!(last(&t).status, PacketStatus::Hub(status.into()));
            assert_eq!(t.port_status("tool.risk"), Some(&PacketStatus::Hub(status.into())), "el LED toma el status");
        }
        let before = t.packets().len();
        t.reduce(&hub_resp("r0", "tool.risk", "toolgate", "ok"), now);
        assert_eq!(t.packets().len(), before, "la traza ya está cerrada");
    }

    #[test]
    fn capability_events_are_state() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&loaded("agent-guard", &["typesafe"]), now);
        t.reduce(&loaded("agent-wakeup", &["typesafe"]), now);
        t.reduce(&cap_loaded("tool.risk", "toolgate", "exec"), now);
        t.reduce(&cap_loaded("agent.wakeup", "jevalan-flow", "flow"), now);
        t.reduce(&cap_loaded("agent.wakeup", "jevalan-flow", "flow"), now);
        assert_eq!(t.capabilities().count(), 2, "repetirlo no duplica");
        assert_eq!(t.ports(), ["agent.wakeup", "tool.risk", "agent-guard", "agent-wakeup", PASSTHROUGH]);
        // El primer tráfico revela qué flujo sirve a la capacidad: deja de tener puerto propio.
        t.reduce(&hub_req("h", "agent.wakeup", "jevalan-flow", "crush"), now);
        t.reduce(&node_start("h", "agent-wakeup"), now);
        assert_eq!(t.ports(), ["agent.wakeup", "tool.risk", "agent-guard", PASSTHROUGH]);
        assert_eq!(t.capability("agent.wakeup").and_then(|c| c.flow.as_deref()), Some("agent-wakeup"));
        // Recargar el flujo o la capacidad no le devuelve el puerto.
        t.reduce(&loaded("agent-wakeup", &["typesafe"]), now);
        t.reduce(&cap_loaded("agent.wakeup", "jevalan-flow", "flow"), now);
        assert_eq!(t.ports(), ["agent.wakeup", "tool.risk", "agent-guard", PASSTHROUGH]);
        t.reduce(&ev("c", EventKind::CapabilityRemoved { capability: "tool.risk".into() }), now);
        assert_eq!(t.ports(), ["agent.wakeup", "agent-guard", PASSTHROUGH]);
        assert!(t.backends().all(|(n, _)| n != "toolgate"), "su especialista se retira con ella");
        t.reduce(&ev("c", EventKind::CapabilityRemoved { capability: "agent.wakeup".into() }), now);
        assert_eq!(t.ports(), ["agent-guard", "agent-wakeup", PASSTHROUGH], "sin capacidad, el flujo vuelve");
        t.clear_capabilities();
        assert_eq!(t.capabilities().count(), 0);
    }

    #[test]
    fn invalid_capability_is_marked_until_reloaded_or_removed() {
        let mut t = Topology::new();
        let now = Instant::now();
        let invalid =
            |name: &str| ev("i", EventKind::CapabilityInvalid { capability: name.into(), reason: "x".into() });
        t.reduce(&cap_loaded("tool.risk", "toolgate", "exec"), now);
        t.reduce(&invalid("desconocida.nunca"), now);
        assert!(!t.is_invalid("desconocida.nunca"), "solo se marcan las conocidas");
        t.reduce(&invalid("tool.risk"), now);
        assert!(t.is_invalid("tool.risk"));
        t.reduce(&cap_loaded("tool.risk", "toolgate", "exec"), now);
        assert!(!t.is_invalid("tool.risk"));
        t.reduce(&invalid("tool.risk"), now);
        t.reduce(&ev("c", EventKind::CapabilityRemoved { capability: "tool.risk".into() }), now);
        assert!(!t.is_invalid("tool.risk") && t.ports() == [PASSTHROUGH]);
    }

    #[test]
    fn flow_backed_capability_hides_its_flow_port_from_the_start() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&loaded("agent-wakeup", &["typesafe"]), now);
        t.reduce(&cap_served_by("agent.wakeup", "agent-wakeup"), now);
        assert_eq!(t.ports(), ["agent.wakeup", PASSTHROUGH], "sin tráfico previo");
        assert_eq!(t.capability("agent.wakeup").and_then(|c| c.flow.as_deref()), Some("agent-wakeup"));
        // El orden de llegada no importa: el flujo puede cargarse después.
        let mut u = Topology::new();
        u.reduce(&cap_served_by("agent.wakeup", "agent-wakeup"), now);
        u.reduce(&loaded("agent-wakeup", &["typesafe"]), now);
        assert_eq!(u.ports(), ["agent.wakeup", PASSTHROUGH]);
    }

    #[test]
    fn flow_provider_trace_closes_on_the_hub_response() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&loaded("agent-wakeup", &["typesafe"]), now);
        t.reduce(&cap_loaded("agent.wakeup", "jevalan-flow", "flow"), now);
        t.reduce(&hub_req("h", "agent.wakeup", "jevalan-flow", "cron"), now);
        t.reduce(&node_start("h", "agent-wakeup"), now);
        t.reduce(&backend_req("h", "typesafe"), now);
        assert_eq!(route(last(&t)), (NodeId::Port("agent.wakeup".into()), NodeId::Backend("typesafe".into())));
        t.reduce(&backend_resp("h", "typesafe", true), now);
        let before = t.packets().len();
        assert!(t.reduce(&flow_decision("h", "agent-wakeup"), now).is_empty());
        assert_eq!(t.packets().len(), before, "el desenlace del flujo no cierra la traza del hub");
        assert_eq!(t.port_status("agent-wakeup"), None);
        t.reduce(&hub_resp("h", "agent.wakeup", "jevalan-flow", "ok"), now);
        assert_eq!(route(last(&t)), (NodeId::Hub, NodeId::Client("cron".into())));
        assert_eq!(t.port_status("agent.wakeup"), Some(&PacketStatus::Hub("ok".into())));
    }

    #[test]
    fn exec_provider_gets_its_own_chip_style() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&cap_loaded("tool.risk", "toolgate", "exec"), now);
        t.reduce(&cap_loaded("agent.wakeup", "jevalan-flow", "flow"), now);
        assert!(t.is_provider("toolgate"));
        assert!(!t.is_provider("jevalan-flow"), "un flujo no es un especialista");
        let chips: Vec<_> = t.backends().collect();
        assert_eq!(chips, [("toolgate", BackendState::Up)], "el chip existe antes del primer tráfico");
        t.reduce(&hub_req("a", "tool.risk", "toolgate", "c"), now);
        t.reduce(&backend_req("a", "toolgate"), now);
        for _ in 0..DOWN_AFTER {
            t.reduce(&backend_resp("a", "toolgate", false), now);
        }
        assert!(t.backends().any(|(n, s)| n == "toolgate" && s == BackendState::Down), "cuenta fallos como cualquiera");
        t.clear_capabilities();
        assert_eq!(t.backends().count(), 0);
        assert!(!t.is_provider("toolgate"));
    }

    #[test]
    fn rule_nodes_do_not_create_a_chip() {
        let mut t = Topology::new();
        t.reduce(&loaded("agent-wakeup", &["typesafe", "-"]), Instant::now());
        assert_eq!(t.backends().map(|(n, _)| n).collect::<Vec<_>>(), ["typesafe"]);
    }

    #[test]
    fn unknown_capability_does_not_grow_the_model() {
        let mut t = Topology::new();
        let now = Instant::now();
        t.reduce(&hub_req("h", "fantasma", "-", "crush"), now);
        t.reduce(&hub_resp("h", "fantasma", "-", "error"), now);
        assert_eq!(t.port_status("fantasma"), None);
        assert_eq!(t.ports(), [PASSTHROUGH]);
        assert_eq!(route(last(&t)), (NodeId::Hub, NodeId::Client("crush".into())));
    }

    #[test]
    fn same_inputs_same_model() {
        let t0 = Instant::now();
        let events = vec![
            loaded("agent-guard", &["typesafe"]),
            req("a", "crush"),
            backend_req("a", "typesafe"),
            backend_resp("a", "typesafe", false),
            decision("a", "deny"),
        ];
        let run = || {
            let mut t = Topology::new();
            let mut cues = Vec::new();
            for (i, e) in events.iter().enumerate() {
                let now = t0 + Duration::from_millis(100 * i as u64);
                cues.extend(t.reduce(e, now));
                cues.extend(t.tick(now));
            }
            (t, cues)
        };
        assert_eq!(run(), run());
    }
}
