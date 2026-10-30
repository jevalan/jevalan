//! Screen contract and shared context.

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

/// What an event source hands to the TUI.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // travels through the channel as is, without accumulating
pub enum Feed {
    Event(Event),
    Status(SourceStatus),
}

/// State of the connection to a live source.
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

/// Demo backend switch: receives the name and returns whether it ends up off; `None` if it does not know it.
pub type BackendToggle = Box<dyn Fn(&str) -> Option<bool> + Send>;

/// State shared by all the screens.
pub struct Ctx {
    pub store: Store,
    pub topo: Topology,
    /// Flows known through `FLOW_LOADED`, by name.
    pub flows: BTreeMap<String, Arc<Flow>>,
    /// Edge traversals and in-flight packets of each flow.
    pub activity: FlowActivity,
    pub fx: FxMode,
    pub now: Instant,
    pub paused: bool,
    pub source: &'static str,
    pub source_closed: bool,
    /// Connection state; `None` with the fake source.
    pub source_status: Option<SourceStatus>,
    /// Cues of the last tick; they expire when the tick ends.
    pub cues: Vec<Cue>,
    /// Only exists in the demo with its own daemon.
    pub toggle: Option<BackendToggle>,
    /// Ephemeral notice for the header, with the time it was born.
    pub notice: Option<(String, Instant)>,
    /// Local path of `shadow.jsonl`, if known; the hook view follows it.
    pub shadow_log: Option<std::path::PathBuf>,
    /// Local directory of the session log, if known; the sessions view reads it.
    pub sessions_dir: Option<std::path::PathBuf>,
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
            sessions_dir: None,
        }
    }
}

pub trait Screen {
    fn update(&mut self, ctx: &mut Ctx, msg: &Msg) -> Option<Nav>;
    fn draw(&mut self, ctx: &Ctx, f: &mut Frame, area: Rect);
    fn keys(&self) -> &'static [KeyHint];

    /// `true` while the screen is taking text (a filter being typed): the global keys (`q`, `f`, `p`, `esc`...) then
    /// go to the screen as ordinary characters.
    fn captures_text(&self) -> bool {
        false
    }

    /// `true` while the screen has an animation of its own in progress (effects).
    fn animating(&self) -> bool {
        false
    }
}
