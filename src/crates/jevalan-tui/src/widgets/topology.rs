//! Drawing of the topology on rataflow.

use crate::fx::FxMode;
use crate::screen::Msg;
use crate::store::Store;
use crate::theme;
use crate::topology::{
    BackendState, CHIP_H, ClientState, NodeId, Packet, PacketStatus, Placed, SWITCH_H, Topology, layout, progress,
    sparkline_row,
};
use crate::widgets::geometry::point_along;
use rataflow::{
    Edge, EdgeContent, EdgePathContext, EdgeRenderContext, EdgeStyle, Flow, Handle, HandlePosition, Node, NodeContent,
    NodeRenderContext, Path, Position, Viewport,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Paragraph, Widget};
use std::collections::BTreeMap;
use std::time::Instant;

/// Arrow step, in cells.
const PAN_STEP: f64 = 2.0;
/// Spacing between the points of the trail, as a fraction of the edge.
const TRAIL_STEP: f64 = 0.05;

/// Stable key of a node in rataflow: "client:crush", "overflow", "hub", "port:x", "backend:x".
fn key(id: &NodeId) -> String {
    match id {
        NodeId::Client(n) => format!("client:{n}"),
        NodeId::Overflow => "overflow".into(),
        NodeId::Hub => "hub".into(),
        NodeId::Port(n) => format!("port:{n}"),
        NodeId::Backend(n) => format!("backend:{n}"),
    }
}

fn edge_id(from: &NodeId, to: &NodeId) -> String {
    format!("{}->{}", key(from), key(to))
}

/// Look of a node: plain box, framed box with title, rack switch, DIP chip or port with port LED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Plain,
    /// The flat hub carries the label as the frame's title, so as not to overlap its ports.
    Titled,
    Switch {
        act: bool,
    },
    Chip,
    /// `exec`/`http` specialist of a capability: chip with the notch on the right.
    ProviderChip,
    Port,
}

#[derive(Debug)]
struct NodeBox {
    label: String,
    style: Style,
    kind: NodeKind,
    /// A port's LED: `Some` if a live packet touches it.
    led: Option<Style>,
    /// A client's name is split into lines instead of being trimmed.
    wrap: bool,
}

/// Writes `s` at (x, y) only if the cell falls inside `area` and the buffer.
fn put(buf: &mut Buffer, area: Rect, x: u16, y: u16, s: &str, style: Style) {
    if area.contains((x, y).into()) && buf.area.contains((x, y).into()) {
        buf[(x, y)].set_symbol(s).set_style(style);
    }
}

/// Splits a client name into pieces of at most `width` columns, preferring to cut
/// right after `/` or `:` so as not to split the client or the project; if there is no separator
/// at hand, it cuts across.
fn wrap_name(name: &str, width: u16) -> Vec<String> {
    let width = usize::from(width).max(1);
    let chars: Vec<char> = name.chars().collect();
    let mut out = Vec::new();
    let mut from = 0;
    while chars.len() - from > width {
        let head = &chars[from..from + width];
        let cut = head.iter().rposition(|c| *c == '/' || *c == ':').map_or(width, |i| i + 1);
        out.push(chars[from..from + cut].iter().collect());
        from += cut;
    }
    out.push(chars[from..].iter().collect());
    out
}

impl NodeBox {
    fn plain(&self, border: Style, area: Rect, buf: &mut Buffer) {
        let titled = self.kind == NodeKind::Titled || matches!(self.kind, NodeKind::Switch { .. });
        let mut block = Block::bordered().border_style(border);
        if titled {
            block = block.title(Span::styled(self.label.as_str(), self.style));
        }
        let inner = block.inner(area);
        block.render(area, buf);
        if !titled {
            let lines: Vec<Line> = if self.wrap {
                wrap_name(&self.label, inner.width)
                    .into_iter()
                    .map(|l| Line::from(Span::styled(l, self.style)))
                    .collect()
            } else {
                vec![Line::from(Span::styled(self.label.as_str(), self.style))]
            };
            Paragraph::new(Text::from(lines)).render(inner, buf);
        }
    }

    fn port(&self, border: Style, area: Rect, buf: &mut Buffer) {
        self.plain(border, area, buf);
        let (led, style) = match self.led {
            Some(style) => ("[●]", style),
            None => ("[○]", theme::led_off()),
        };
        for (i, c) in led.chars().enumerate() {
            put(buf, area, area.x + 1 + i as u16, area.y, &c.to_string(), style);
        }
    }

    fn switch(&self, act: bool, border: Style, area: Rect, buf: &mut Buffer) {
        let frame = Rect { height: SWITCH_H - 1, ..area };
        let title = format!("═[ {} ]", self.label.to_uppercase());
        Block::bordered()
            .border_type(BorderType::Double)
            .border_style(border)
            .title(Span::styled(title, border))
            .render(frame, buf);
        let act_led = if act { theme::accent() } else { theme::led_off() };
        for (row, (name, led)) in [("PWR", theme::packet(&PacketStatus::Ok)), ("ACT", act_led)].into_iter().enumerate()
        {
            let y = area.y + 1 + row as u16;
            put(buf, area, area.x + 2, y, "▣", led);
            for (i, c) in name.chars().enumerate() {
                put(buf, area, area.x + 4 + i as u16, y, &c.to_string(), theme::text());
            }
        }
        for x in [area.x + 2, area.right().saturating_sub(3)] {
            put(buf, area, x, area.y + SWITCH_H - 2, "╤", border);
            put(buf, area, x, area.y + SWITCH_H - 1, "╧", border);
        }
    }

    fn chip(&self, provider: bool, border: Style, area: Rect, buf: &mut Buffer) {
        let body = Rect { y: area.y + 1, height: CHIP_H - 2, ..area };
        let block = Block::bordered().border_type(BorderType::Rounded).border_style(border);
        let inner = block.inner(body);
        block.render(body, buf);
        if provider {
            Paragraph::new(Span::styled(format!(" {}", self.label), self.style)).render(inner, buf);
            put(buf, area, inner.right().saturating_sub(1), inner.y, "◗", border);
        } else {
            Paragraph::new(Span::styled(format!("◖ {}", self.label), self.style)).render(inner, buf);
        }
        for x in (area.x + 1..area.right().saturating_sub(1)).step_by(2) {
            put(buf, area, x, area.y, "╷", border);
            put(buf, area, x, area.y + 1, "┴", border);
            put(buf, area, x, area.y + CHIP_H - 2, "┬", border);
            put(buf, area, x, area.y + CHIP_H - 1, "╵", border);
        }
    }
}

impl NodeContent for NodeBox {
    fn render(&self, ctx: &NodeRenderContext, buf: &mut Buffer) {
        let area = ctx.area.intersection(buf.area);
        let border = if ctx.selected { theme::accent().add_modifier(Modifier::BOLD) } else { self.style };
        if area.height < 3 || area.width < 3 {
            Paragraph::new(Span::styled(self.label.as_str(), self.style)).render(area, buf);
            return;
        }
        // If the drawn rect is not enough for the complete apparatus (zoom, clipping), it falls back to the plain box.
        let fits = area == ctx.area;
        match self.kind {
            NodeKind::Switch { act } if fits && area.height >= SWITCH_H && area.width >= 12 => {
                self.switch(act, border, area, buf);
            }
            NodeKind::Chip | NodeKind::ProviderChip if fits && area.height >= CHIP_H && area.width >= 5 => {
                self.chip(self.kind == NodeKind::ProviderChip, border, area, buf);
            }
            NodeKind::Port => self.port(border, area, buf),
            _ => self.plain(border, area, buf),
        }
    }
}

#[derive(Debug, Default)]
struct PacketEdge {
    /// (progress 0..=1 in the edge's direction, +1 if it travels with it and -1 if against, style)
    packets: Vec<(f64, f64, Style)>,
    trail: bool,
}

impl PacketEdge {
    fn put(ctx: &EdgeRenderContext, points: &[(f64, f64)], t: f64, symbol: &str, style: Style, buf: &mut Buffer) {
        let Some((wx, wy)) = point_along(points, t) else { return };
        let (x, y) = ctx.world_to_terminal(Position::new(wx, wy));
        if ctx.is_in_bounds(x, y) && x >= 0 && y >= 0 && buf.area.contains((x as u16, y as u16).into()) {
            buf[(x as u16, y as u16)].set_symbol(symbol).set_style(style);
        }
    }
}

impl EdgeContent for PacketEdge {
    fn compute_path(&self, ctx: &EdgePathContext) -> Path {
        Path::straight_with_direction(ctx.from, ctx.to, ctx.source_position, ctx.target_position)
    }

    fn render(&self, ctx: &EdgeRenderContext, buf: &mut Buffer) {
        ctx.render_path(&EdgeStyle::braille().with_no_markers().with_stroke_style(theme::dim()), None, buf);
        let points: Vec<(f64, f64)> = ctx.path.points.iter().map(|p| (p.x, p.y)).collect();
        if self.trail {
            for (t, dir, style) in &self.packets {
                for k in 1..=2 {
                    let behind = (t - dir * TRAIL_STEP * f64::from(k)).clamp(0.0, 1.0);
                    Self::put(ctx, &points, behind, "·", style.add_modifier(Modifier::DIM), buf);
                }
            }
        }
        for (t, _, style) in &self.packets {
            Self::put(ctx, &points, *t, "●", *style, buf);
        }
    }
}

/// Position of a node in rataflow's world: the ports are relative to the hub.
fn world_position(id: &NodeId, p: Placed, hub: Option<Placed>) -> (f64, f64) {
    match (id, hub) {
        (NodeId::Port(_), Some(h)) => (f64::from(p.x) - f64::from(h.x), f64::from(p.y) - f64::from(h.y)),
        _ => (f64::from(p.x), f64::from(p.y)),
    }
}

/// A node with no visible or interactive handles: nothing is connected or dragged here.
fn new_node(id: &NodeId, p: Placed, hub: Option<Placed>) -> Node<NodeBox> {
    let handles = vec![
        Handle::source(HandlePosition::Right).with_id("right"),
        Handle::source(HandlePosition::Bottom).with_id("bottom"),
        Handle::target(HandlePosition::Left).with_id("left"),
        Handle::target(HandlePosition::Top).with_id("top"),
    ]
    .into_iter()
    .map(|h| h.with_hidden(true).with_connectable(false))
    .collect();
    let content = NodeBox { label: String::new(), style: theme::text(), kind: NodeKind::Plain, led: None, wrap: false };
    let mut node = Node::new(key(id), world_position(id, p, hub), (f64::from(p.w), f64::from(p.h)), content)
        .with_handles(handles)
        .with_draggable(false)
        .with_deletable(false)
        .with_connectable(false)
        .with_resizable(false);
    if matches!(id, NodeId::Port(_)) {
        node = node.with_parent("hub");
    }
    if *id == NodeId::Hub {
        node = node.with_opaque(false);
    }
    node
}

pub struct TopologyView {
    flow: Flow<NodeBox, PacketEdge>,
    selected: Option<NodeId>,
    boxes: BTreeMap<NodeId, Rect>,
    last_layout: Vec<(NodeId, Placed)>,
    drag_from: Option<(u16, u16)>,
    /// Last position set for each node, and the relocations in flight.
    at: BTreeMap<NodeId, (f64, f64)>,
    moves: BTreeMap<NodeId, Move>,
}

/// How long a relocation lasts, in milliseconds.
const MOVE_MS: f64 = 240.0;

/// A node being relocated: where it comes from, where it is going and since when.
struct Move {
    from: (f64, f64),
    to: (f64, f64),
    start: Instant,
}

/// Starts fast and brakes on arrival; in cells it shows more than a straight line.
fn eased(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

impl Default for TopologyView {
    fn default() -> Self {
        Self::new()
    }
}

impl TopologyView {
    pub fn new() -> Self {
        Self {
            flow: Flow::new(),
            selected: None,
            boxes: BTreeMap::new(),
            last_layout: Vec::new(),
            drag_from: None,
            at: BTreeMap::new(),
            moves: BTreeMap::new(),
        }
    }

    /// `true` while some node is on its way to its place.
    pub fn is_moving(&self) -> bool {
        !self.moves.is_empty()
    }

    pub fn selected(&self) -> Option<&NodeId> {
        self.selected.as_ref()
    }

    /// A node's box on screen according to the last `render`.
    pub fn node_area(&self, id: &NodeId) -> Option<Rect> {
        self.boxes.get(id).copied()
    }

    /// Draws the topology in `area`. With `fx == FxMode::Full`, each packet leaves a trail of two faint `·`.
    pub fn render(&mut self, topo: &Topology, now: Instant, fx: FxMode, area: Rect, buf: &mut Buffer) {
        let placed = layout(topo, area.width, area.height);
        self.sync_nodes(&placed, now);
        self.sync_content(topo, now);
        self.sync_edges(topo, &placed);
        self.sync_packets(topo, now, fx);
        if let Some(sel) = self.selected.clone() {
            if self.last_layout.iter().any(|(id, _)| *id == sel) {
                self.flow.select_node(&key(&sel));
            } else {
                self.selected = None;
                self.flow.clear_selection();
            }
        }
        (&mut self.flow).render(area, buf);
        self.boxes =
            self.last_layout.iter().filter_map(|(id, _)| Some((id.clone(), self.screen_box(id, area)?))).collect();
    }

    /// Under each chip, the backend's latency history: sparkline scaled to the maximum and the last measurement.
    /// Buckets with a failure (and without a good response) are marked with a red `×`.
    pub fn render_latency(&self, store: &Store, now: Instant, area: Rect, buf: &mut Buffer) {
        const GLYPHS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
        for (id, p) in &self.last_layout {
            let NodeId::Backend(name) = id else { continue };
            let Some(b) = self.boxes.get(id) else { continue };
            if sparkline_row(*p, area.height).is_none() || b.height < CHIP_H || b.bottom() >= area.bottom() {
                continue;
            }
            let y = b.bottom();
            let tail = store.last_latency(name).map_or_else(|| " —".to_string(), |ms| format!(" {ms}ms"));
            let tail_w = tail.chars().count() as u16;
            let Some(spark_w) = b.width.checked_sub(tail_w).filter(|w| *w >= 4) else { continue };
            let series = store.latency_series(name, now);
            let failed = store.latency_failures(name, now);
            let from = series.len() - usize::from(spark_w).min(series.len());
            let max = series.iter().flatten().copied().max().unwrap_or(0).max(1);
            for (i, (v, f)) in series[from..].iter().zip(&failed[from..]).enumerate() {
                let (glyph, style) = match v {
                    Some(v) => (GLYPHS[(v * 7 / max).min(7) as usize], theme::spark()),
                    None if *f => ("×", theme::error()),
                    None => (" ", theme::dim()),
                };
                // Right-aligned: if there are fewer buckets than width, there is air on the left.
                put(buf, area, b.x + spark_w - (series.len() - from) as u16 + i as u16, y, glyph, style);
            }
            for (i, c) in tail.chars().enumerate() {
                put(buf, area, b.x + spark_w + i as u16, y, &c.to_string(), theme::text());
            }
        }
    }

    fn screen_box(&self, id: &NodeId, area: Rect) -> Option<Rect> {
        let (l, t, r, b) = self.flow.node_terminal_rect(&key(id))?;
        let (l, t) = (l.max(i32::from(area.left())), t.max(i32::from(area.top())));
        let (r, b) = (r.min(i32::from(area.right())), b.min(i32::from(area.bottom())));
        (r > l && b > t).then(|| Rect::new(l as u16, t as u16, (r - l) as u16, (b - t) as u16))
    }

    fn sync_nodes(&mut self, placed: &[(NodeId, Placed)], now: Instant) {
        for (id, _) in &self.last_layout {
            if !placed.iter().any(|(n, _)| n == id) {
                self.flow.remove_node(&key(id));
                self.at.remove(id);
                self.moves.remove(id);
            }
        }
        let hub = placed.iter().find(|(id, _)| *id == NodeId::Hub).map(|(_, p)| *p);
        for (id, p) in placed {
            if self.flow.node(&key(id)).is_none() {
                let _ = self.flow.add_node(new_node(id, *p, hub));
                self.at.insert(id.clone(), world_position(id, *p, hub));
                self.moves.remove(id);
            }
        }
        if self.last_layout != placed {
            for (id, p) in placed {
                let k = key(id);
                self.flow.set_node_dimensions(&k, f64::from(p.w), f64::from(p.h));
                let to = world_position(id, *p, hub);
                match self.at.get(id) {
                    // It really moves: it is given a trip from where it is now to its new place.
                    Some(from) if *from != to => {
                        let from = *from;
                        self.moves.insert(id.clone(), Move { from, to, start: now });
                    }
                    _ => {
                        self.flow.set_node_positions([(k, to)]);
                        self.at.insert(id.clone(), to);
                        self.moves.remove(id);
                    }
                }
            }
            self.last_layout = placed.to_vec();
        }
        // On each frame, those on their way advance towards their place.
        let mut en_vuelo: Vec<(NodeId, (f64, f64), bool)> = Vec::with_capacity(self.moves.len());
        for (id, m) in &self.moves {
            let t = (now.saturating_duration_since(m.start).as_secs_f64() * 1000.0 / MOVE_MS).clamp(0.0, 1.0);
            let e = eased(t);
            let p = (m.from.0 + (m.to.0 - m.from.0) * e, m.from.1 + (m.to.1 - m.from.1) * e);
            en_vuelo.push((id.clone(), if t >= 1.0 { m.to } else { p }, t >= 1.0));
        }
        for (id, p, llego) in en_vuelo {
            self.flow.set_node_positions([(key(&id), p)]);
            self.at.insert(id.clone(), p);
            if llego {
                self.moves.remove(&id);
            }
        }
    }

    fn sync_content(&mut self, topo: &Topology, now: Instant) {
        let backends: BTreeMap<&str, BackendState> = topo.backends().collect();
        let hardware = self.last_layout.iter().any(|(id, p)| *id == NodeId::Hub && p.h == SWITCH_H);
        let port_led = |name: &str| {
            let port = NodeId::Port(name.into());
            let touches = |p: &&Packet| topo.display(&p.from) == port || topo.display(&p.to) == port;
            // The packet in flight flashes; if there is none, the port's last result rules.
            let live = topo.packets().iter().filter(touches).rfind(|p| progress(p, now) < 1.0).map(|p| &p.status);
            live.or_else(|| topo.port_status(name)).map(theme::packet)
        };
        let port_text = |name: &str| if topo.is_invalid(name) { theme::dim() } else { theme::text() };
        for (id, p) in &self.last_layout {
            let mut led = None;
            let (label, style, kind) = match id {
                NodeId::Client(name) => {
                    let idle = topo.client_state(name, now) == Some(ClientState::Idle);
                    (name.clone(), if idle { theme::dim() } else { theme::text() }, NodeKind::Plain)
                }
                NodeId::Overflow => (format!("+{}", topo.overflow()), theme::text(), NodeKind::Plain),
                NodeId::Hub if p.h == SWITCH_H => {
                    ("jevalan".into(), theme::text(), NodeKind::Switch { act: topo.has_live_packets() })
                }
                NodeId::Hub => ("jevalan".into(), theme::text(), NodeKind::Titled),
                NodeId::Port(name) if hardware => {
                    led = port_led(name);
                    (name.clone(), port_text(name), NodeKind::Port)
                }
                NodeId::Port(name) => (name.clone(), port_text(name), NodeKind::Plain),
                NodeId::Backend(name) => {
                    let provider = topo.is_provider(name);
                    let kind = match (p.h == CHIP_H, provider) {
                        (true, true) => NodeKind::ProviderChip,
                        (true, false) => NodeKind::Chip,
                        _ => NodeKind::Plain,
                    };
                    let up = if provider { theme::provider() } else { theme::text() };
                    match backends.get(name.as_str()) {
                        Some(BackendState::Down) => (format!("{name} ✖ down"), theme::error(), kind),
                        _ => (format!("{name} ● up"), up, kind),
                    }
                }
            };
            if let Some(c) = self.flow.node_content_mut(&key(id)) {
                *c = NodeBox { label, style, kind, led, wrap: matches!(id, NodeId::Client(_)) };
            }
        }
    }

    fn sync_edges(&mut self, topo: &Topology, placed: &[(NodeId, Placed)]) {
        use HandlePosition::{Bottom, Left, Right, Top};
        let has = |id: &NodeId| placed.iter().any(|(n, _)| n == id);
        let mut wanted: Vec<(NodeId, NodeId, HandlePosition, HandlePosition)> = Vec::new();
        for (id, _) in placed {
            if matches!(id, NodeId::Client(_) | NodeId::Overflow) {
                wanted.push((id.clone(), NodeId::Hub, Right, Left));
            }
        }
        for (port, backend) in topo.port_backend_edges() {
            wanted.push((NodeId::Port(port.into()), NodeId::Backend(backend.into()), Bottom, Top));
        }
        wanted.retain(|(a, b, ..)| has(a) && has(b));
        let wanted_ids: Vec<String> = wanted.iter().map(|(a, b, ..)| edge_id(a, b)).collect();
        let stale: Vec<String> =
            self.flow.edges().iter().map(|e| e.id.clone()).filter(|id| !wanted_ids.contains(id)).collect();
        for id in stale {
            self.flow.remove_edge(&id);
        }
        for (a, b, from_side, to_side) in wanted {
            let id = edge_id(&a, &b);
            if self.flow.edge(&id).is_none() {
                let edge = Edge::new(id, key(&a), key(&b))
                    .with_source_side(from_side)
                    .with_target_side(to_side)
                    .with_selectable(false)
                    .with_deletable(false);
                let _ = self.flow.add_edge(edge);
            }
        }
    }

    fn sync_packets(&mut self, topo: &Topology, now: Instant, fx: FxMode) {
        for (_, content) in self.flow.edges_content_mut() {
            content.packets.clear();
            content.trail = fx == FxMode::Full;
        }
        for p in topo.packets() {
            let (from, to) = (topo.display(&p.from), topo.display(&p.to));
            let t = progress(p, now);
            let style = theme::packet(&p.status);
            let (id, t, dir) = if self.flow.edge(&edge_id(&from, &to)).is_some() {
                (edge_id(&from, &to), t, 1.0)
            } else if self.flow.edge(&edge_id(&to, &from)).is_some() {
                (edge_id(&to, &from), 1.0 - t, -1.0)
            } else {
                continue;
            };
            if let Some(c) = self.flow.edge_content_mut(&id) {
                c.packets.push((t, dir, style));
            }
        }
    }

    /// Pan/zoom/fit/select. Returns `true` if the view changed.
    pub fn handle(&mut self, msg: &Msg, area: Rect) -> bool {
        match msg {
            Msg::Key(k) if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) => self.handle_key(k.code),
            Msg::Mouse(m) => {
                let at = (m.column, m.row).into();
                let inside = area.contains(at);
                match m.kind {
                    MouseEventKind::Down(MouseButton::Left) if inside => {
                        self.drag_from = Some((m.column, m.row));
                        let hit = self
                            .last_layout
                            .iter()
                            .rev()
                            .map(|(id, _)| id)
                            .find(|id| self.boxes.get(id).is_some_and(|b| b.contains(at)))
                            .cloned();
                        self.select(hit)
                    }
                    MouseEventKind::Drag(MouseButton::Left) => match self.drag_from.replace((m.column, m.row)) {
                        Some((x, y)) => {
                            self.flow.pan(f64::from(m.column) - f64::from(x), f64::from(m.row) - f64::from(y));
                            true
                        }
                        None => false,
                    },
                    MouseEventKind::Up(MouseButton::Left) => {
                        self.drag_from = None;
                        false
                    }
                    MouseEventKind::ScrollUp if inside => {
                        self.flow.zoom_in();
                        true
                    }
                    MouseEventKind::ScrollDown if inside => {
                        self.flow.zoom_out();
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn handle_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Left | KeyCode::Char('h') => self.flow.pan(PAN_STEP, 0.0),
            KeyCode::Right | KeyCode::Char('l') => self.flow.pan(-PAN_STEP, 0.0),
            KeyCode::Up | KeyCode::Char('k') => self.flow.pan(0.0, 1.0),
            KeyCode::Down | KeyCode::Char('j') => self.flow.pan(0.0, -1.0),
            KeyCode::Char('+' | '=') => self.flow.zoom_in(),
            KeyCode::Char('-') => self.flow.zoom_out(),
            // The layout is already computed to fit the area: fitting is returning to the neutral view.
            KeyCode::Char('0') => self.flow.viewport = Viewport::default_view(),
            KeyCode::Char('n') => return self.step_selection(1),
            KeyCode::Char('N') => return self.step_selection(-1),
            _ => return false,
        }
        true
    }

    fn step_selection(&mut self, delta: isize) -> bool {
        let n = self.last_layout.len() as isize;
        if n == 0 {
            return false;
        }
        let current = self.selected.as_ref().and_then(|s| self.last_layout.iter().position(|(id, _)| id == s));
        let next = match current {
            Some(i) => (i as isize + delta).rem_euclid(n),
            None if delta > 0 => 0,
            None => n - 1,
        };
        let id = self.last_layout[next as usize].0.clone();
        self.select(Some(id))
    }

    fn select(&mut self, id: Option<NodeId>) -> bool {
        match &id {
            Some(id) => self.flow.select_node(&key(id)),
            None => self.flow.clear_selection(),
        }
        let changed = self.selected != id;
        self.selected = id;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{BACKENDS, flows, loaded_events};
    use crate::topology::{NodeId, PACKET_TTL, Topology};
    use jevalan_events::{Endpoint, Event, EventKind, SCHEMA_VERSION};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;
    use std::time::{Duration, Instant};

    fn req(trace: &str, client: &str) -> Event {
        Event {
            v: SCHEMA_VERSION,
            id: trace.into(),
            ts: String::new(),
            trace_id: trace.into(),
            seq: 1,
            source: "test".into(),
            kind: EventKind::RequestReceived {
                client: client.into(),
                endpoint: Endpoint::Decide,
                flow: Some("agent-guard".into()),
                backend: None,
            },
        }
    }

    /// Topology with the example flows loaded: three ports and two chips.
    fn loaded_topo(now: Instant) -> Topology {
        let mut topo = Topology::new();
        for e in loaded_events(&flows()) {
            topo.reduce(&e, now);
        }
        topo
    }

    /// Like `loaded_topo`, plus the `agent.wakeup` flow and the two example capabilities, already with a first
    /// traffic that reveals which flow serves `agent.wakeup` (and takes its own port away).
    fn hub_topo(now: Instant) -> Topology {
        let mut topo = loaded_topo(now);
        for e in loaded_events(&[crate::fake::wakeup_flow()]).into_iter().chain(crate::fake::capability_events()) {
            topo.reduce(&e, now);
        }
        let request = EventKind::HubRequest {
            capability: "agent.wakeup".into(),
            capability_version: "1".into(),
            provider: "jevalan-flow".into(),
            client: "crush".into(),
        };
        let start = EventKind::NodeStart {
            flow: "agent-wakeup".into(),
            node: "user".into(),
            node_type: "rule".into(),
            backend: "typesafe".into(),
            model: "m".into(),
        };
        topo.reduce(&Event { kind: request, ..req("first", "crush") }, now);
        topo.reduce(&Event { kind: start, seq: 2, ..req("first", "crush") }, now);
        topo.tick(now + PACKET_TTL * 2);
        topo
    }

    fn text(buf: &Buffer, area: Rect) -> String {
        let mut s = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn draws_nodes_and_a_packet_in_flight() {
        let now = Instant::now();
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", "crush"), now);
        let area = Rect::new(0, 0, 100, 20);
        let mut buf = Buffer::empty(area);
        let mut view = TopologyView::new();
        view.render(&topo, now + PACKET_TTL / 2, FxMode::Events, area, &mut buf);
        let out = text(&buf, area);
        for label in
            ["crush", "JEVALAN", "agent-guard", "model-route", "passthrough", "typesafe ● up", "ollama ● up", "●"]
        {
            assert!(out.contains(label), "falta {label:?} en\n{out}");
        }
        let boxes: Vec<Rect> = [
            NodeId::Client("crush".into()),
            NodeId::Hub,
            NodeId::Backend("typesafe".into()),
            NodeId::Backend("ollama".into()),
        ]
        .iter()
        .map(|id| view.node_area(id).expect("caja del nodo"))
        .collect();
        let packet_cells = (area.top()..area.bottom())
            .flat_map(|y| (area.left()..area.right()).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                buf[(x, y)].symbol() == "●"
                    && buf[(x, y)].fg == crate::theme::ALLOW
                    && !boxes.iter().any(|b| b.contains((x, y).into()))
            })
            .count();
        assert!(packet_cells >= 1, "ningún ● verde fuera de las cajas en\n{out}");
    }

    #[test]
    fn long_names_never_draw_outside_the_area() {
        let now = Instant::now();
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", &"x".repeat(48)), now);
        let full = Rect::new(0, 0, 120, 30);
        let area = Rect::new(10, 5, 90, 16);
        let mut buf = Buffer::empty(full);
        TopologyView::new().render(&topo, now, FxMode::Events, area, &mut buf);
        for y in full.top()..full.bottom() {
            for x in full.left()..full.right() {
                if !area.contains((x, y).into()) {
                    assert_eq!(buf[(x, y)].symbol(), " ", "celda pintada fuera del área en ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn packet_to_a_departed_client_does_not_panic() {
        let now = Instant::now();
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", "cron"), now);
        topo.tick(now + crate::topology::CLIENT_GONE);
        let area = Rect::new(0, 0, 100, 20);
        let mut buf = Buffer::empty(area);
        TopologyView::new().render(&topo, now + crate::topology::CLIENT_GONE, FxMode::Events, area, &mut buf);
    }

    #[test]
    fn n_cycles_selection_in_layout_order() {
        let now = Instant::now();
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", "crush"), now);
        let area = Rect::new(0, 0, 100, 20);
        let mut view = TopologyView::new();
        let mut buf = Buffer::empty(area);
        view.render(&topo, now, FxMode::Events, area, &mut buf);
        let n = Msg::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(view.handle(&n, area));
        assert_eq!(view.selected(), Some(&NodeId::Client("crush".into())));
        view.handle(&n, area);
        assert_eq!(view.selected(), Some(&NodeId::Hub));
        assert!(view.node_area(&NodeId::Hub).is_some());
    }

    #[test]
    fn snapshot_with_traffic() {
        let t0 = Instant::now();
        let mut topo = loaded_topo(t0);
        let flows = flows();
        let mut rng = crate::fake::Rng::new(3);
        for (i, client) in ["crush", "vscode", "cli"].iter().enumerate() {
            for (offset, e) in crate::fake::trace(&mut rng, &format!("t{i}"), client, &flows, None) {
                topo.reduce(&e, t0 + offset);
            }
        }
        let area = Rect::new(0, 0, 100, 18);
        let mut buf = Buffer::empty(area);
        TopologyView::new().render(&topo, t0 + PACKET_TTL / 3, FxMode::Events, area, &mut buf);
        insta::assert_snapshot!(text(&buf, area));
    }

    #[test]
    fn snapshot_with_hub_traffic() {
        let t0 = Instant::now();
        let mut topo = hub_topo(t0);
        let wakeup = crate::fake::wakeup_flow();
        let mut rng = crate::fake::Rng::new(11);
        for (i, client) in ["crush", "vscode", "cli", "cron", "ci-bot", "script-a"].iter().enumerate() {
            for (offset, e) in crate::fake::hub_trace(&mut rng, &format!("h{i}"), client, &wakeup, None) {
                topo.reduce(&e, t0 + offset);
            }
        }
        let area = Rect::new(0, 0, 120, 20);
        let mut buf = Buffer::empty(area);
        TopologyView::new().render(&topo, t0 + PACKET_TTL / 3, FxMode::Events, area, &mut buf);
        insta::assert_snapshot!(text(&buf, area));
    }

    #[test]
    fn provider_chip_has_a_notch_on_the_right_and_backend_chips_do_not() {
        let now = Instant::now();
        let topo = hub_topo(now);
        let area = Rect::new(0, 0, 120, 20);
        let mut buf = Buffer::empty(area);
        let mut view = TopologyView::new();
        view.render(&topo, now, FxMode::Events, area, &mut buf);
        let provider = view.node_area(&NodeId::Backend("toolgate".into())).expect("chip del especialista");
        let out = text(&buf, provider);
        assert!(out.contains("toolgate ● up") && out.contains('◗') && !out.contains('◖'), "{out}");
        let backend = view.node_area(&NodeId::Backend("typesafe".into())).expect("chip del backend");
        let out = text(&buf, backend);
        assert!(out.contains('◖') && !out.contains('◗'), "{out}");
    }

    #[test]
    fn capability_ports_carry_their_full_names() {
        let now = Instant::now();
        let topo = hub_topo(now);
        let area = Rect::new(0, 0, 120, 20);
        let mut buf = Buffer::empty(area);
        TopologyView::new().render(&topo, now, FxMode::Events, area, &mut buf);
        let out = text(&buf, area);
        for label in ["agent.wakeup", "tool.risk", "agent-guard", "model-route", "passthrough"] {
            assert!(out.contains(label), "falta {label:?} en\n{out}");
        }
        assert!(!out.contains("agent-wakeup"), "el flujo que sirve a una capacidad no tiene puerto propio en\n{out}");
    }

    #[test]
    fn port_led_stays_lit_with_the_last_result() {
        let now = Instant::now();
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", "crush"), now);
        let done =
            EventKind::DecisionMade { flow: "agent-guard".into(), outcome: "deny".into(), path_len: 1, duration_ms: 1 };
        topo.reduce(&Event { kind: done, seq: 2, ..req("a", "crush") }, now);
        let later = now + PACKET_TTL * 2;
        topo.tick(later);
        let area = Rect::new(0, 0, 100, 20);
        let mut buf = Buffer::empty(area);
        TopologyView::new().render(&topo, later, FxMode::Events, area, &mut buf);
        let out = text(&buf, area);
        assert_eq!(out.matches("[●]").count(), 1, "{out}");
        assert!(out.contains("[○]"));
    }

    #[test]
    fn idle_topology_draws_no_connection_handles() {
        let now = Instant::now();
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", "crush"), now);
        let area = Rect::new(0, 0, 100, 20);
        let mut buf = Buffer::empty(area);
        TopologyView::new().render(&topo, now + PACKET_TTL * 2, FxMode::Events, area, &mut buf);
        let dots = text(&buf, area).matches('●').count();
        assert_eq!(dots, BACKENDS.len(), "solo deben quedar los ● de las etiquetas de los backends");
    }

    #[test]
    fn switch_shows_title_power_and_port_leds() {
        let now = Instant::now();
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", "crush"), now);
        let call = EventKind::BackendRequest { backend: "typesafe".into(), model: "m".into(), questions: 1 };
        topo.reduce(&Event { kind: call, seq: 2, ..req("a", "crush") }, now);
        let area = Rect::new(0, 0, 100, 20);
        let mut buf = Buffer::empty(area);
        let mut view = TopologyView::new();
        view.render(&topo, now + PACKET_TTL / 2, FxMode::Events, area, &mut buf);
        let out = text(&buf, area);
        for label in ["[ JEVALAN ]", "PWR", "ACT", "[○]"] {
            assert!(out.contains(label), "falta {label:?} en\n{out}");
        }
        let lit = ["agent-guard", "model-route", "passthrough"].iter().any(|p| {
            let b = view.node_area(&NodeId::Port((*p).into())).expect("caja del puerto");
            (b.left()..b.right()).any(|x| buf[(x, b.y)].symbol() == "●" && buf[(x, b.y)].fg == crate::theme::ALLOW)
        });
        assert!(lit, "ningún [●] verde en un puerto en\n{out}");
    }

    #[test]
    fn backends_are_drawn_as_dip_chips() {
        let now = Instant::now();
        let topo = loaded_topo(now);
        let area = Rect::new(0, 0, 100, 20);
        let mut buf = Buffer::empty(area);
        let mut view = TopologyView::new();
        view.render(&topo, now, FxMode::Events, area, &mut buf);
        let b = view.node_area(&NodeId::Backend("typesafe".into())).expect("caja del backend");
        let out = text(&buf, b);
        for sym in ["┴", "┬", "◖"] {
            assert!(out.contains(sym), "falta {sym:?} en\n{out}");
        }
    }

    #[test]
    fn short_area_draws_flat_boxes() {
        let now = Instant::now();
        let topo = loaded_topo(now);
        let area = Rect::new(0, 0, 100, 12);
        let mut buf = Buffer::empty(area);
        TopologyView::new().render(&topo, now, FxMode::Events, area, &mut buf);
        let out = text(&buf, area);
        assert!(!out.contains('╔') && !out.contains('◖'), "se dibujó hardware en\n{out}");
    }

    #[test]
    fn full_fx_leaves_a_trail_behind_packets() {
        let now = Instant::now();
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", "crush"), now);
        let area = Rect::new(0, 0, 100, 20);
        let at = now + PACKET_TTL / 2;
        let count = |fx| {
            let mut buf = Buffer::empty(area);
            TopologyView::new().render(&topo, at, fx, area, &mut buf);
            text(&buf, area).matches('·').count()
        };
        assert_eq!(count(FxMode::Events), 0);
        assert!(count(FxMode::Full) >= 1);
    }

    #[test]
    fn click_selects_the_node_under_the_pointer() {
        use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let now = Instant::now();
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", "crush"), now);
        let area = Rect::new(0, 0, 100, 20);
        let mut view = TopologyView::new();
        view.render(&topo, now, FxMode::Events, area, &mut Buffer::empty(area));
        let b = view.node_area(&NodeId::Port("agent-guard".into())).expect("caja");
        let click = Msg::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: b.x + 1,
            row: b.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(view.handle(&click, area));
        assert_eq!(view.selected(), Some(&NodeId::Port("agent-guard".into())));
    }

    #[test]
    fn chip_shows_a_sparkline_and_last_latency() {
        let t0 = Instant::now();
        let topo = loaded_topo(t0);
        let mut store = Store::new();
        for (i, (ok, ms)) in [(true, 40), (true, 120), (false, 900), (true, 80)].into_iter().enumerate() {
            let kind = EventKind::BackendResponse {
                backend: "typesafe".into(),
                ok,
                status: if ok { 200 } else { 0 },
                duration_ms: ms,
                input_tokens: None,
            };
            store.ingest(Event { kind, seq: i as u64 + 2, ..req("a", "crush") }, t0 + Duration::from_secs(i as u64));
        }
        let now = t0 + Duration::from_secs(4);
        let area = Rect::new(0, 0, 100, 20);
        let mut buf = Buffer::empty(area);
        let mut view = TopologyView::new();
        view.render(&topo, now, FxMode::Events, area, &mut buf);
        view.render_latency(&store, now, area, &mut buf);
        let chip = view.node_area(&NodeId::Backend("typesafe".into())).expect("chip");
        let row = text(&buf, Rect::new(chip.x, chip.bottom(), chip.width, 1));
        assert!(row.contains("80ms"), "{row:?}");
        assert!(row.chars().any(|c| "▁▂▃▄▅▆▇█".contains(c)), "{row:?}");
        assert!(row.contains('×'), "el fallo debe marcarse: {row:?}");
        let other = view.node_area(&NodeId::Backend("ollama".into())).expect("chip");
        let row = text(&buf, Rect::new(other.x, other.bottom(), other.width, 1));
        assert!(row.contains('—'), "sin datos: {row:?}");
    }

    /// The hook's name is `<client>/<project>:<session>`: it must be possible to read it in full.
    #[test]
    fn a_long_client_name_is_drawn_whole_when_the_window_allows() {
        let now = Instant::now();
        let name = "app/desktop:ab12";
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", name), now);
        let area = Rect::new(0, 0, 120, 30);
        let mut buf = Buffer::empty(area);
        TopologyView::new().render(&topo, now, FxMode::Events, area, &mut buf);
        let out = text(&buf, area);
        assert!(out.contains(name), "el nombre sale cortado en\n{out}");
    }

    #[test]
    fn a_client_name_wider_than_the_cap_gets_a_second_line_instead_of_being_cut() {
        let now = Instant::now();
        let name = "claude-code/mi-proyecto-largo:1a2b";
        let mut topo = loaded_topo(now);
        topo.reduce(&req("a", name), now);
        let area = Rect::new(0, 0, 120, 30);
        let mut buf = Buffer::empty(area);
        TopologyView::new().render(&topo, now, FxMode::Events, area, &mut buf);
        let out = text(&buf, area);
        // On a single 22-column line the two halves do not fit: their appearing proves the cut.
        assert!(out.contains("claude-code/") && out.contains("mi-proyecto-largo:1a2b"), "{out}");
    }

    #[test]
    fn wrap_name_cuts_after_the_separators() {
        let got = |s: &str, w: u16| -> Vec<String> { wrap_name(s, w) };
        assert_eq!(got("crush", 12), vec!["crush"]);
        assert_eq!(got("app/desktop:ab12", 12), vec!["app/desktop:", "ab12"]);
        assert_eq!(got("claude-code/mi-proyecto-largo:1a2b", 24), vec!["claude-code/", "mi-proyecto-largo:1a2b"]);
        assert_eq!(got("sinseparadores", 6), vec!["sinsep", "arador", "es"]);
    }
}
