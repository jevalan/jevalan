use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, Instant};

pub const PACKET_TTL: Duration = Duration::from_millis(600);
pub const CLIENT_IDLE: Duration = Duration::from_secs(10);
pub const CLIENT_GONE: Duration = Duration::from_secs(30);
pub const MAX_VISIBLE_CLIENTS: usize = 5;
pub const MAX_OPEN_TRACES: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeId {
    Client(String),
    Overflow,
    Hub,
    Port(String),
    Backend(String),
}

/// Fixed port for traffic that goes through no flow.
pub const PASSTHROUGH: &str = "passthrough";
/// Consecutive failures of a backend to consider it down.
pub const DOWN_AFTER: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketStatus {
    Ok,
    Failed,
    Outcome(String),
    /// Hub result: `ok`, `abstain` or `error`.
    Hub(String),
}

/// What is known about a capability from its `CAPABILITY_LOADED`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityInfo {
    pub version: String,
    pub provider: String,
    /// `flow`, `exec` o `http`.
    pub kind: String,
    /// Flow that serves it: the load event announces it; if missing, it is learned from traffic.
    pub flow: Option<String>,
    pub description: String,
    pub input_fields: Vec<String>,
    pub decision_fields: Vec<String>,
}

/// Path of an open trace: who asked, through which port it entered and whether it belongs to the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Route {
    pub client: String,
    pub port: String,
    pub hub: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub trace_id: String,
    pub from: NodeId,
    pub to: NodeId,
    pub started: Instant,
    pub status: PacketStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    Active,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendState {
    Up,
    Down,
}

/// Something worth mentioning for the effects layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cue {
    ClientJoined(String),
    ClientLeft(String),
    Denied(String),
    BackendDown(String),
    Error(String),
}

/// Progress of a packet in [0, 1]; it is computed, not stored.
pub fn progress(p: &Packet, now: Instant) -> f64 {
    (now.saturating_duration_since(p.started).as_secs_f64() / PACKET_TTL.as_secs_f64()).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Topology {
    pub(super) ports: Vec<String>,
    /// Client → last activity.
    pub(super) clients: BTreeMap<String, Instant>,
    pub(super) visible: Vec<String>,
    pub(super) backends: BTreeMap<String, BackendState>,
    /// Backends the daemon says do not live on this machine (a subset of `backends`).
    pub(super) remote: BTreeSet<String>,
    /// Fallos consecutivos por backend.
    pub(super) failures: HashMap<String, u32>,
    /// (port, backend) pairs seen in some BACKEND_REQUEST.
    pub(super) port_backend_edges: BTreeSet<(String, String)>,
    /// trace_id → path, to return the decision to whoever asked. Bounded by `MAX_OPEN_TRACES`.
    pub(super) trace_routes: HashMap<String, Route>,
    /// Order of arrival of the traces; may contain already-closed ids.
    pub(super) route_order: VecDeque<String>,
    pub(super) packets: Vec<Packet>,
    /// Colour of each port's last result; the LED keeps it until the next one.
    pub(super) last_status: BTreeMap<String, PacketStatus>,
    /// Loaded flows; those that serve a capability have no port of their own.
    pub(super) loaded_flows: BTreeSet<String>,
    pub(super) capabilities: BTreeMap<String, CapabilityInfo>,
    /// Known capabilities whose last edit was not valid: they are still served, but painted dimmed.
    pub(super) invalid_caps: BTreeSet<String>,
    /// Capability → flow that serves it; survives reloads and reconnections.
    pub(super) served: BTreeMap<String, String>,
}

impl Default for Topology {
    fn default() -> Self {
        Self::new()
    }
}

impl Topology {
    pub fn new() -> Self {
        Self {
            ports: vec![PASSTHROUGH.to_string()],
            clients: BTreeMap::new(),
            visible: Vec::new(),
            backends: BTreeMap::new(),
            remote: BTreeSet::new(),
            failures: HashMap::new(),
            port_backend_edges: BTreeSet::new(),
            trace_routes: HashMap::new(),
            route_order: VecDeque::new(),
            packets: Vec::new(),
            last_status: BTreeMap::new(),
            loaded_flows: BTreeSet::new(),
            capabilities: BTreeMap::new(),
            served: BTreeMap::new(),
            invalid_caps: BTreeSet::new(),
        }
    }

    /// Ports: capabilities, flows that serve none (both in alphabetical order) and `passthrough` at the end.
    pub fn ports(&self) -> &[String] {
        &self.ports
    }

    /// Visible clients, from the most recent activity to the oldest.
    pub fn visible_clients(&self) -> &[String] {
        &self.visible
    }

    /// Present clients that do not fit (the `+N` node).
    pub fn overflow(&self) -> usize {
        self.clients.len() - self.visible.len()
    }

    pub fn client_state(&self, name: &str, now: Instant) -> Option<ClientState> {
        self.clients.get(name).map(|last| {
            if now.saturating_duration_since(*last) >= CLIENT_IDLE { ClientState::Idle } else { ClientState::Active }
        })
    }

    pub fn backends(&self) -> impl Iterator<Item = (&str, BackendState)> {
        self.backends.iter().map(|(n, s)| (n.as_str(), *s))
    }

    pub fn port_backend_edges(&self) -> impl Iterator<Item = (&str, &str)> {
        self.port_backend_edges.iter().map(|(p, b)| (p.as_str(), b.as_str()))
    }

    /// Last result seen on the port, if any.
    pub fn port_status(&self, port: &str) -> Option<&PacketStatus> {
        self.last_status.get(port)
    }

    pub fn packets(&self) -> &[Packet] {
        &self.packets
    }

    pub fn has_live_packets(&self) -> bool {
        !self.packets.is_empty()
    }

    /// Node where `id` is drawn: a client that does not fit is drawn in `Overflow`.
    pub fn display(&self, id: &NodeId) -> NodeId {
        match id {
            NodeId::Client(name) if !self.visible.contains(name) => NodeId::Overflow,
            other => other.clone(),
        }
    }

    pub fn capabilities(&self) -> impl Iterator<Item = (&str, &CapabilityInfo)> {
        self.capabilities.iter().map(|(n, c)| (n.as_str(), c))
    }

    /// The capability's last edit was invalid (the daemon keeps serving the previous version).
    pub fn is_invalid(&self, name: &str) -> bool {
        self.invalid_caps.contains(name)
    }

    pub fn capability(&self, name: &str) -> Option<&CapabilityInfo> {
        self.capabilities.get(name)
    }

    /// `true` if the daemon announced `backend` as living outside this machine.
    pub fn is_remote(&self, backend: &str) -> bool {
        self.remote.contains(backend)
    }

    /// `true` if `backend` is the `exec`/`http` specialist of some capability.
    pub fn is_provider(&self, backend: &str) -> bool {
        self.capabilities.values().any(|c| c.kind != "flow" && c.provider == backend)
    }

    /// Recomputes the ports: capabilities, free flows and `passthrough`.
    pub(super) fn rebuild_ports(&mut self) {
        let serving: BTreeSet<&str> =
            self.served.iter().filter(|(c, _)| self.capabilities.contains_key(*c)).map(|(_, f)| f.as_str()).collect();
        let mut ports: Vec<String> = self.capabilities.keys().cloned().collect();
        ports.extend(self.loaded_flows.iter().filter(|f| !serving.contains(f.as_str())).cloned());
        ports.retain(|p| p != PASSTHROUGH);
        ports.push(PASSTHROUGH.to_string());
        self.ports = ports;
    }

    pub(super) fn refresh_visible(&mut self) {
        let mut all: Vec<(&String, &Instant)> = self.clients.iter().collect();
        all.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        self.visible = all.into_iter().take(MAX_VISIBLE_CLIENTS).map(|(n, _)| n.clone()).collect();
    }
}
