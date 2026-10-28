//! Contrato de pantalla y contexto compartido.

use crate::flow_activity::FlowActivity;
use crate::fx::FxMode;
use crate::store::Store;
use crate::topology::{Cue, Topology};
use jevalan_events::Event;
use jevalan_flow::Flow;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone)]
pub enum Msg {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    Tick(Instant),
    Event(Box<Event>),
    Status(SourceStatus),
}

/// Lo que una fuente de eventos entrega a la TUI.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // viaja por el canal tal cual, sin acumularse
pub enum Feed {
    Event(Event),
    Status(SourceStatus),
}

/// Estado de la conexión con una fuente en vivo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceStatus {
    Connecting,
    Connected,
    Disconnected(String),
}

pub enum Nav {
    Push(Box<dyn Screen>),
    Pop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyHint {
    pub key: &'static str,
    pub desc: &'static str,
}

/// Interruptor de backends de la demo: recibe el nombre y devuelve si queda apagado; `None` si no lo conoce.
pub type BackendToggle = Box<dyn Fn(&str) -> Option<bool> + Send>;

/// Estado compartido por todas las pantallas.
pub struct Ctx {
    pub store: Store,
    pub topo: Topology,
    /// Flujos conocidos por `FLOW_LOADED`, por nombre.
    pub flows: BTreeMap<String, Arc<Flow>>,
    /// Recorridos de las aristas y paquetes en vuelo de cada flujo.
    pub activity: FlowActivity,
    pub fx: FxMode,
    pub now: Instant,
    pub paused: bool,
    pub source: &'static str,
    pub source_closed: bool,
    /// Estado de la conexión; `None` con la fuente ficticia.
    pub source_status: Option<SourceStatus>,
    /// Cues del último tick; caducan al terminar el tick.
    pub cues: Vec<Cue>,
    /// Solo existe en la demo con daemon propio.
    pub toggle: Option<BackendToggle>,
    /// Aviso efímero para la cabecera, con la hora en que nació.
    pub notice: Option<(String, Instant)>,
    /// Ruta local de `shadow.jsonl`, si se conoce; la vista del hook la sigue.
    pub shadow_log: Option<std::path::PathBuf>,
}

impl Ctx {
    pub fn new(fx: FxMode, now: Instant) -> Self {
        Self {
            store: Store::new(),
            topo: Topology::new(),
            flows: BTreeMap::new(),
            activity: FlowActivity::new(),
            fx,
            now,
            paused: false,
            source: "fake",
            source_closed: false,
            source_status: None,
            cues: Vec::new(),
            toggle: None,
            notice: None,
            shadow_log: None,
        }
    }
}

pub trait Screen {
    fn update(&mut self, ctx: &mut Ctx, msg: &Msg) -> Option<Nav>;
    fn draw(&mut self, ctx: &Ctx, f: &mut Frame, area: Rect);
    fn keys(&self) -> &'static [KeyHint];

    /// `true` mientras la pantalla tenga una animación propia en curso (efectos).
    fn animating(&self) -> bool {
        false
    }
}
