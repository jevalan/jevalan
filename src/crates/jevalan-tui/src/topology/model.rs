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

/// Puerto fijo del tráfico que no pasa por ningún flujo.
pub const PASSTHROUGH: &str = "passthrough";
/// Fallos consecutivos de un backend para darlo por caído.
pub const DOWN_AFTER: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketStatus {
    Ok,
    Failed,
    Outcome(String),
    /// Resultado del hub: `ok`, `abstain` o `error`.
    Hub(String),
}

/// Lo que se sabe de una capacidad por su `CAPABILITY_LOADED`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityInfo {
    pub version: String,
    pub provider: String,
    /// `flow`, `exec` o `http`.
    pub kind: String,
    /// Flujo que la sirve: lo anuncia el evento de carga; si faltara, se aprende del tráfico.
    pub flow: Option<String>,
    pub description: String,
    pub input_fields: Vec<String>,
    pub decision_fields: Vec<String>,
}

/// Camino de una traza abierta: quién preguntó, por qué puerto entró y si es del hub.
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

/// Algo digno de mención para la capa de efectos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cue {
    ClientJoined(String),
    ClientLeft(String),
    Denied(String),
    BackendDown(String),
    Error(String),
}

/// Progreso de un paquete en [0, 1]; se calcula, no se almacena.
pub fn progress(p: &Packet, now: Instant) -> f64 {
    (now.saturating_duration_since(p.started).as_secs_f64() / PACKET_TTL.as_secs_f64()).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Topology {
    pub(super) ports: Vec<String>,
    /// Cliente → última actividad.
    pub(super) clients: BTreeMap<String, Instant>,
    pub(super) visible: Vec<String>,
    pub(super) backends: BTreeMap<String, BackendState>,
    /// Fallos consecutivos por backend.
    pub(super) failures: HashMap<String, u32>,
    /// Pares (puerto, backend) vistos en algún BACKEND_REQUEST.
    pub(super) port_backend_edges: BTreeSet<(String, String)>,
    /// trace_id → camino, para devolver la decisión a quien preguntó. Acotado por `MAX_OPEN_TRACES`.
    pub(super) trace_routes: HashMap<String, Route>,
    /// Orden de llegada de las trazas; puede contener ids ya cerrados.
    pub(super) route_order: VecDeque<String>,
    pub(super) packets: Vec<Packet>,
    /// Color del último resultado de cada puerto; el LED lo conserva hasta el siguiente.
    pub(super) last_status: BTreeMap<String, PacketStatus>,
    /// Flujos cargados; los que sirven a una capacidad no tienen puerto propio.
    pub(super) loaded_flows: BTreeSet<String>,
    pub(super) capabilities: BTreeMap<String, CapabilityInfo>,
    /// Capacidades conocidas cuya última edición no valió: se siguen sirviendo, pero se pintan apagadas.
    pub(super) invalid_caps: BTreeSet<String>,
    /// Capacidad → flujo que la sirve; sobrevive a recargas y reconexiones.
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

    /// Puertos: capacidades, flujos que no sirven a ninguna (ambos por orden alfabético) y `passthrough` al final.
    pub fn ports(&self) -> &[String] {
        &self.ports
    }

    /// Clientes visibles, de la actividad más reciente a la más antigua.
    pub fn visible_clients(&self) -> &[String] {
        &self.visible
    }

    /// Clientes presentes que no caben (el nodo `+N`).
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

    /// Último resultado visto en el puerto, si lo hubo.
    pub fn port_status(&self, port: &str) -> Option<&PacketStatus> {
        self.last_status.get(port)
    }

    pub fn packets(&self) -> &[Packet] {
        &self.packets
    }

    pub fn has_live_packets(&self) -> bool {
        !self.packets.is_empty()
    }

    /// Nodo donde se dibuja `id`: un cliente que no cabe se dibuja en `Overflow`.
    pub fn display(&self, id: &NodeId) -> NodeId {
        match id {
            NodeId::Client(name) if !self.visible.contains(name) => NodeId::Overflow,
            other => other.clone(),
        }
    }

    pub fn capabilities(&self) -> impl Iterator<Item = (&str, &CapabilityInfo)> {
        self.capabilities.iter().map(|(n, c)| (n.as_str(), c))
    }

    /// La última edición de la capacidad fue inválida (el daemon sigue sirviendo la versión anterior).
    pub fn is_invalid(&self, name: &str) -> bool {
        self.invalid_caps.contains(name)
    }

    pub fn capability(&self, name: &str) -> Option<&CapabilityInfo> {
        self.capabilities.get(name)
    }

    /// `true` si `backend` es el especialista `exec`/`http` de alguna capacidad.
    pub fn is_provider(&self, backend: &str) -> bool {
        self.capabilities.values().any(|c| c.kind != "flow" && c.provider == backend)
    }

    /// Recalcula los puertos: capacidades, flujos libres y `passthrough`.
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
