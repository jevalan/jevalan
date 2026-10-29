//! Drawing of a flow's graph on rataflow: boxes per node and output, edges with counter and packets.

use crate::flow_activity::FlowActivity;
use crate::flow_layout::{FlowNodeId, columns, edges};
use crate::screen::Msg;
use crate::screens::trace::answer_text;
use crate::theme;
use crate::widgets::geometry::point_along;
use jevalan_flow::Flow;
use rataflow::{
    Edge, EdgeContent, EdgePathContext, EdgeRenderContext, EdgeStyle, Flow as Graph, Handle, HandlePosition, Node,
    NodeContent, NodeRenderContext, Path, Position, Viewport,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

/// Arrow step, in cells.
const PAN_STEP: f64 = 2.0;
pub const NODE_W: u16 = 16;
pub const NODE_H: u16 = 5;
pub const OUT_W: u16 = 12;
pub const OUT_H: u16 = 3;
const MIN_GAP: u16 = 4;
const MAX_GAP: u16 = 16;

fn key(id: &FlowNodeId) -> String {
    match id {
        FlowNodeId::Node(n) => format!("n:{n}"),
        FlowNodeId::Output(n) => format!("o:{n}"),
    }
}

fn edge_id(from: &FlowNodeId, port: &str) -> String {
    format!("{}>{port}", key(from))
}

/// Identifier of a box, as the painter uses it: `n:<node>` or `o:<output>`.
pub fn caja(id: &FlowNodeId) -> String {
    key(id)
}

/// Identifier of an edge: `n:<node>><port>`.
pub fn arista(from: &FlowNodeId, port: &str) -> String {
    edge_id(from, port)
}

/// Trims to `n` characters, with an ellipsis if it does not fit.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n.saturating_sub(1)).chain(['…']).collect() }
}

#[derive(Debug)]
struct NodeBox {
    title: Option<String>,
    lines: Vec<Line<'static>>,
    border: Style,
}

impl NodeContent for NodeBox {
    fn render(&self, ctx: &NodeRenderContext, buf: &mut Buffer) {
        let area = ctx.area.intersection(buf.area);
        if area.height < 3 || area.width < 3 {
            return;
        }
        let border = if ctx.selected { theme::accent() } else { self.border };
        let mut block = Block::bordered().border_style(border);
        if let Some(t) = &self.title {
            block = block.title(Span::styled(format!(" {t} "), theme::dim()));
        }
        let inner = block.inner(area);
        block.render(area, buf);
        Paragraph::new(self.lines.clone()).render(inner, buf);
    }
}

#[derive(Debug, Default)]
struct FlowEdge {
    label: String,
    /// Where the label is written (world) and how many cells it has free.
    at: (f64, f64),
    room: usize,
    hot: bool,
    packet: Style,
    packets: Vec<f64>,
}

impl EdgeContent for FlowEdge {
    fn compute_path(&self, ctx: &EdgePathContext) -> Path {
        Path::straight_with_direction(ctx.from, ctx.to, ctx.source_position, ctx.target_position)
    }

    fn render(&self, ctx: &EdgeRenderContext, buf: &mut Buffer) {
        let stroke = if self.hot { theme::text() } else { theme::dim() };
        ctx.render_path(&EdgeStyle::braille().with_no_markers().with_stroke_style(stroke), None, buf);
        let points: Vec<(f64, f64)> = ctx.path.points.iter().map(|p| (p.x, p.y)).collect();
        let put = |t: f64, s: &str, style: Style, buf: &mut Buffer| {
            let Some((wx, wy)) = point_along(&points, t) else { return };
            let (x, y) = ctx.world_to_terminal(Position::new(wx, wy));
            if ctx.is_in_bounds(x, y) && x >= 0 && y >= 0 && buf.area.contains((x as u16, y as u16).into()) {
                buf[(x as u16, y as u16)].set_symbol(s).set_style(style);
            }
        };
        for t in &self.packets {
            put(*t, "●", self.packet, buf);
        }
    }
}

/// Placed box: identity, position and size in the world.
type Placed = (FlowNodeId, (f64, f64), (f64, f64));

/// Position and size in the world of each box: one column per depth, centred in the area.
pub fn place(flow: &Flow, width: u16, height: u16) -> Vec<Placed> {
    let cols = columns(flow);
    let ncols = cols.iter().map(|(_, s)| s.col + 1).max().unwrap_or(0);
    let size = |id: &FlowNodeId| match id {
        FlowNodeId::Node(_) => (NODE_W, NODE_H),
        FlowNodeId::Output(_) => (OUT_W, OUT_H),
    };
    let col_w: Vec<u16> = (0..ncols)
        .map(|c| cols.iter().filter(|(_, s)| s.col == c).map(|(id, _)| size(id).0).max().unwrap_or(0))
        .collect();
    let gap =
        if ncols > 1 { (width.saturating_sub(col_w.iter().sum()) / (ncols - 1)).clamp(MIN_GAP, MAX_GAP) } else { 0 };
    let total_w = col_w.iter().sum::<u16>() + gap * ncols.saturating_sub(1);
    let col_rows = |c: u16| cols.iter().filter(|(_, s)| s.col == c).count() as u16;
    let col_h = |c: u16, vgap: u16| {
        let h: u16 = cols.iter().filter(|(_, s)| s.col == c).map(|(id, _)| size(id).1).sum();
        h + vgap * col_rows(c).saturating_sub(1)
    };
    let tallest = |vgap: u16| (0..ncols).map(|c| col_h(c, vgap)).max().unwrap_or(0);
    let vgap = if tallest(2) <= height { 2 } else { 1 };
    let (x0, y0) = (width.saturating_sub(total_w) / 2, height.saturating_sub(tallest(vgap)) / 2);

    let mut out = Vec::new();
    for c in 0..ncols {
        let x = x0 + col_w[..c as usize].iter().sum::<u16>() + gap * c;
        let mut y = y0 + (tallest(vgap) - col_h(c, vgap)) / 2;
        for (id, _) in cols.iter().filter(|(_, s)| s.col == c) {
            let (w, h) = size(id);
            out.push((id.clone(), (f64::from(x), f64::from(y)), (f64::from(w), f64::from(h))));
            y += h + vgap;
        }
    }
    out
}

/// Place for each edge label: in the gap following its origin, at the height of the stroke and without overlapping another.
fn label_spots(flow: &Flow, placed: &[Placed]) -> BTreeMap<String, ((f64, f64), usize)> {
    let find = |id: &FlowNodeId| placed.iter().find(|(n, ..)| n == id).map(|(_, p, s)| (*p, *s));
    let mut taken: Vec<(i32, i32, i32)> = Vec::new();
    let mut spots = BTreeMap::new();
    for (from, port, to) in edges(flow) {
        let (Some(((fx, fy), (fw, fh))), Some(((tx, ty), (_, th)))) = (find(&from), find(&to)) else { continue };
        let (sx, sy, ex, ey) = (fx + fw, fy + fh / 2.0, tx, ty + th / 2.0);
        let end = placed.iter().map(|(_, (x, _), _)| *x).filter(|x| *x > fx).fold(ex, f64::min);
        let room = (end - sx - 2.0).max(0.0) as usize;
        let x = sx + 1.0;
        let mid = x + room as f64 / 2.0;
        let line_y = if ex > sx { sy + (ey - sy) * (mid - sx) / (ex - sx) } else { sy };
        let base = line_y.round() as i32 - 1;
        let y = [0, 1, -1, 2, -2, 3, -3, 4, -4]
            .iter()
            .map(|d| base + d)
            .find(|y| !taken.iter().any(|(ty, a, b)| ty == y && *a < x as i32 + room as i32 && x as i32 <= *b))
            .unwrap_or(base);
        taken.push((y, x as i32, x as i32 + room as i32));
        spots.insert(edge_id(&from, &port), ((x, f64::from(y)), room));
    }
    spots
}

pub struct FlowGraph {
    graph: Graph<NodeBox, FlowEdge>,
    built: Option<(Arc<Flow>, u16, u16)>,
    drag_from: Option<(u16, u16)>,
    /// Identifier of the selected box or edge; rataflow paints it with the accent.
    selected: Option<String>,
}

impl Default for FlowGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl FlowGraph {
    pub fn new() -> Self {
        Self { graph: Graph::new(), built: None, drag_from: None, selected: None }
    }

    /// Marks the box or edge `id` (`n:<node>`, `o:<output>` or `n:<node>><port>`); `None` clears.
    /// It is applied immediately, because `build` only rebuilds the graph when the flow or the area changes.
    pub fn select(&mut self, id: Option<String>) {
        match &id {
            Some(id) if self.graph.node(id).is_some() => self.graph.select_node(id),
            Some(id) if self.graph.edge(id).is_some() => self.graph.select_edge(id),
            // An unwired port has no edge to mark: the previous one is cleared and that is all.
            _ => self.graph.clear_selection(),
        }
        self.selected = id;
    }

    /// Rebuilds the graph if the definition or the area's size changed, keeping pan and zoom.
    fn build(&mut self, flow: &Arc<Flow>, area: Rect) {
        let same =
            self.built.as_ref().is_some_and(|(f, w, h)| Arc::ptr_eq(f, flow) && *w == area.width && *h == area.height);
        if same {
            return;
        }
        let viewport = self.graph.viewport;
        self.graph = Graph::new();
        self.graph.viewport = viewport;
        let placed = place(flow, area.width, area.height);
        let spots = label_spots(flow, &placed);
        for (id, pos, size) in placed {
            let handles = vec![
                Handle::source(HandlePosition::Right).with_id("right"),
                Handle::target(HandlePosition::Left).with_id("left"),
            ]
            .into_iter()
            .map(|h| h.with_hidden(true).with_connectable(false))
            .collect();
            let content = NodeBox { title: None, lines: Vec::new(), border: theme::dim() };
            let node = Node::new(key(&id), pos, size, content)
                .with_handles(handles)
                .with_draggable(false)
                .with_deletable(false)
                .with_connectable(false)
                .with_resizable(false);
            let _ = self.graph.add_node(node);
        }
        for (from, port, to) in edges(flow) {
            let id = edge_id(&from, &port);
            let (at, room) = spots.get(&id).copied().unwrap_or_default();
            let edge = Edge::new(id, key(&from), key(&to))
                .with_source_side(HandlePosition::Right)
                .with_target_side(HandlePosition::Left)
                .with_selectable(false)
                .with_deletable(false);
            let _ = self.graph.add_edge(edge);
            if let Some(c) = self.graph.edge_content_mut(&edge_id(&from, &port)) {
                (c.at, c.room) = (at, room);
            }
        }
        // The selection is applied again: `build` has just rebuilt the whole graph.
        if let Some(id) = self.selected.clone() {
            if self.graph.node(&id).is_some() {
                self.graph.select_node(&id);
            } else if self.graph.edge(&id).is_some() {
                self.graph.select_edge(&id);
            }
        }
        self.built = Some((flow.clone(), area.width, area.height));
    }

    fn sync(&mut self, flow: &Flow, act: &FlowActivity, now: Instant) {
        for (name, node) in &flow.nodes {
            let k = key(&FlowNodeId::Node(name.clone()));
            let ran = act.node_count(&flow.name, name) > 0;
            let answer = match act.last_answer(&flow.name, name) {
                Some((a, c)) => {
                    Line::styled(clip(&format!("{} {c:.2}", answer_text(a)), NODE_W as usize - 2), theme::text())
                }
                None => Line::styled("—", theme::dim()),
            };
            if let Some(c) = self.graph.node_content_mut(&k) {
                c.border = if ran { theme::accent() } else { theme::dim() };
                c.lines = vec![
                    Line::styled(clip(&format!("{} · {name}", node.ty.as_str()), NODE_W as usize - 2), theme::title()),
                    Line::styled(
                        clip(&format!("{} · {}", node.backend, node.model), NODE_W as usize - 2),
                        theme::dim(),
                    ),
                    answer,
                ];
            }
        }
        for (name, out) in &flow.outputs {
            if let Some(c) = self.graph.node_content_mut(&key(&FlowNodeId::Output(name.clone()))) {
                let style = theme::outcome(&out.outcome);
                c.border = style;
                c.title = (name != &out.outcome).then(|| clip(name, OUT_W as usize - 4));
                c.lines = vec![Line::styled(clip(&out.outcome, OUT_W as usize - 2), style)];
            }
        }
        for (from, port, to) in edges(flow) {
            let FlowNodeId::Node(node) = &from else { continue };
            let n = act.edge_count(&flow.name, node, &port);
            let packet = match &to {
                FlowNodeId::Output(o) => flow.outputs.get(o).map_or(theme::accent(), |o| theme::outcome(&o.outcome)),
                FlowNodeId::Node(_) => theme::accent(),
            };
            let packets =
                act.packets(&flow.name, now).filter(|(nd, p, _)| *nd == node && *p == port).map(|(_, _, t)| t);
            if let Some(c) = self.graph.edge_content_mut(&edge_id(&from, &port)) {
                let count = format!(" ×{n}");
                let port_room = c.room.saturating_sub(count.chars().count());
                c.label =
                    if port_room >= 2 { format!("{}{count}", clip(&port, port_room)) } else { clip(&count, c.room) };
                c.hot = n > 0;
                c.packet = packet;
                c.packets = packets.collect();
            }
        }
    }

    pub fn render(&mut self, flow: &Arc<Flow>, act: &FlowActivity, now: Instant, area: Rect, buf: &mut Buffer) {
        self.build(flow, area);
        self.sync(flow, act, now);
        (&mut self.graph).render(area, buf);
        // Labels go in a second pass so that no stroke covers them.
        for edge in self.graph.edges() {
            let c = &edge.content;
            let (x, y) = self.graph.world_to_terminal(Position::new(c.at.0, c.at.1));
            let style = if c.hot { theme::accent() } else { theme::dim() };
            for (i, ch) in c.label.chars().enumerate() {
                let x = x + i as i32;
                if self.graph.is_in_bounds(x, y) && x >= 0 && y >= 0 && buf.area.contains((x as u16, y as u16).into()) {
                    buf[(x as u16, y as u16)].set_symbol(&ch.to_string()).set_style(style);
                }
            }
        }
    }

    /// Pan/zoom/fit with keyboard and mouse. Returns `true` if the view changed.
    pub fn handle(&mut self, msg: &Msg, area: Rect) -> bool {
        match msg {
            Msg::Key(k) if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) => self.handle_key(k.code),
            Msg::Mouse(m) => match m.kind {
                MouseEventKind::Down(MouseButton::Left) if area.contains((m.column, m.row).into()) => {
                    self.drag_from = Some((m.column, m.row));
                    false
                }
                MouseEventKind::Drag(MouseButton::Left) => match self.drag_from.replace((m.column, m.row)) {
                    Some((x, y)) => {
                        self.graph.pan(f64::from(m.column) - f64::from(x), f64::from(m.row) - f64::from(y));
                        true
                    }
                    None => false,
                },
                MouseEventKind::Up(MouseButton::Left) => {
                    self.drag_from = None;
                    false
                }
                MouseEventKind::ScrollUp if area.contains((m.column, m.row).into()) => {
                    self.graph.zoom_in();
                    true
                }
                MouseEventKind::ScrollDown if area.contains((m.column, m.row).into()) => {
                    self.graph.zoom_out();
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn handle_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Left | KeyCode::Char('h') => self.graph.pan(PAN_STEP, 0.0),
            KeyCode::Right | KeyCode::Char('l') => self.graph.pan(-PAN_STEP, 0.0),
            KeyCode::Up | KeyCode::Char('k') => self.graph.pan(0.0, 1.0),
            KeyCode::Down | KeyCode::Char('j') => self.graph.pan(0.0, -1.0),
            KeyCode::Char('+' | '=') => self.graph.zoom_in(),
            KeyCode::Char('-') => self.graph.zoom_out(),
            // The layout is already computed to fit the area: fitting is returning to the neutral view.
            KeyCode::Char('0') => self.graph.viewport = Viewport::default_view(),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> Arc<Flow> {
        Arc::new(crate::fake::flows().into_iter().find(|f| f.name == "agent-guard").unwrap())
    }

    #[test]
    fn every_box_fits_the_area_at_100x22() {
        for (id, (x, y), (w, h)) in place(&guard(), 98, 22) {
            assert!(x >= 0.0 && y >= 0.0 && x + w <= 98.0 && y + h <= 22.0, "{id:?} se sale: {x},{y} {w}x{h}");
        }
    }

    #[test]
    fn boxes_do_not_overlap() {
        let placed = place(&guard(), 98, 22);
        for (i, (a, (ax, ay), (aw, ah))) in placed.iter().enumerate() {
            for (b, (bx, by), (bw, bh)) in &placed[i + 1..] {
                let apart = ax + aw <= *bx || bx + bw <= *ax || ay + ah <= *by || by + bh <= *ay;
                assert!(apart, "{a:?} pisa a {b:?}");
            }
        }
    }

    #[test]
    fn clip_marks_the_cut() {
        assert_eq!(clip("corto", 8), "corto");
        assert_eq!(clip("demasiado largo", 8), "demasia…");
    }

    #[test]
    fn los_identificadores_son_los_del_pintor() {
        assert_eq!(caja(&FlowNodeId::Node("risks".into())), "n:risks");
        assert_eq!(caja(&FlowNodeId::Output("allow".into())), "o:allow");
        assert_eq!(arista(&FlowNodeId::Node("risks".into()), "error"), "n:risks>error");
    }

    #[test]
    fn el_pintor_marca_la_caja_y_la_arista_seleccionadas() {
        let flow = guard();
        let mut pintor = FlowGraph::new();
        let area = Rect::new(0, 0, 98, 22);
        pintor.render(&flow, &FlowActivity::new(), Instant::now(), area, &mut Buffer::empty(area));
        assert!(pintor.graph.first_selected_node_id().is_none(), "sin selección no hay nada marcado");

        // The selection is applied immediately: `build` only rebuilds the graph if the flow or the area changes.
        pintor.select(Some(caja(&FlowNodeId::Node("destructive".into()))));
        assert_eq!(pintor.graph.first_selected_node_id().as_deref(), Some("n:destructive"));

        // And it survives the rebuild an area change causes.
        let otra = Rect::new(0, 0, 99, 22);
        pintor.render(&flow, &FlowActivity::new(), Instant::now(), otra, &mut Buffer::empty(otra));
        assert_eq!(pintor.graph.first_selected_node_id().as_deref(), Some("n:destructive"));

        pintor.select(Some(arista(&FlowNodeId::Node("destructive".into()), "false")));
        assert_eq!(pintor.graph.first_selected_edge_id().as_deref(), Some("n:destructive>false"));

        // A port that does not exist marks nothing, and clears the previous one.
        pintor.select(Some(arista(&FlowNodeId::Node("destructive".into()), "inventado")));
        assert!(pintor.graph.first_selected_edge_id().is_none());
        assert!(pintor.graph.first_selected_node_id().is_none());
    }
}
