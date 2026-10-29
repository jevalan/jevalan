use super::model::{NodeId, Topology};

/// A node's box in cells, relative to the topology area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

const CLIENT_W: u16 = 14;
/// Cap on the clients column: wider leaves the hub with no room.
const CLIENT_MAX_W: u16 = 24;
const ENGINE_W: u16 = 20;
/// Natural maximum and minimum width of a port, the switch's inner margin and minimum air after the clients.
const PORT_W: u16 = 15;
const PORT_MIN_W: u16 = 8;
const PAD: u16 = 2;
const MIN_GAP: u16 = 6;
const HUB_H: u16 = 5;
const ENGINE_GAP: u16 = 2;
/// Minimum air between adjacent chips.
const CHIP_GAP: u16 = 4;
const BOX_H: u16 = 3;

/// Height of the hub drawn as a switch: frame, three rows of ports, frame and legs.
pub const SWITCH_H: u16 = 6;
/// Height of an engine drawn as a DIP chip: pins, three-row body, pins.
pub const CHIP_H: u16 = 5;
/// Row of the latency history, just under the chip; only reserved if height is to spare.
pub const SPARK_H: u16 = 1;
/// Columns reserved to the left of the switch for the PWR/ACT LEDs.
pub const LED_PANEL_W: u16 = 8;

/// Fixed positions: clients in a column on the left, hub with the hooks in a row,
/// engines below the hub. The clients column widens to the longest name and,
/// if it still does not fit, the box gains a second line. Designed for areas of at least
/// 60×10; smaller ones do not panic, but may overlap.
pub fn layout(t: &Topology, width: u16, height: u16) -> Vec<(NodeId, Placed)> {
    let mut out = Vec::new();

    let mut slots: Vec<NodeId> = t.visible_clients().iter().map(|c| NodeId::Client(c.clone())).collect();
    if t.overflow() > 0 {
        slots.push(NodeId::Overflow);
    }
    let longest = t.visible_clients().iter().map(|c| c.chars().count() as u16).max().unwrap_or(0);
    let client_w = client_width(longest, width);
    let n = slots.len() as u16;
    if n > 0 {
        // If the name does not fit across and there is height to spare, a second line is reserved for it.
        let want_h = if longest + 2 <= client_w { BOX_H } else { BOX_H + 1 };
        let h = if n * want_h <= height {
            want_h
        } else if n * BOX_H <= height {
            BOX_H
        } else {
            1
        };
        let gap = height.saturating_sub(n * h) / (n + 1);
        for (i, id) in slots.into_iter().enumerate() {
            let y = gap + i as u16 * (h + gap);
            out.push((id, Placed { x: 0, y, w: client_w.min(width), h }));
        }
    }

    let hardware_mode = height >= SWITCH_H + ENGINE_GAP + CHIP_H;
    let hooks = t.ports();
    let k = hooks.len() as u16;
    let m = t.backends().count() as u16;

    // The switch measures what its ports do; never more than 4/5 of the gap.
    let region_x = client_w + MIN_GAP;
    let avail = width.saturating_sub(region_x);
    let led = if hardware_mode { LED_PANEL_W } else { 0 };
    let content = 2 + led + k * PORT_W + 2 * k.saturating_sub(1) + 2 * PAD;
    let hub_w = content.min(avail * 9 / 10);
    let hub_x = region_x + (avail - hub_w) / 2;

    // ponytail: below 13 rows switch and chips do not fit; falls back to plain boxes.
    let (hub_h, engine_h) = if hardware_mode { (SWITCH_H, CHIP_H) } else { (HUB_H, BOX_H) };

    // With height to spare, the gap is split in thirds: margin, air between switch and chips, margin.
    let (hub_y, engine_gap) = if hardware_mode {
        let spark = u16::from(height > SWITCH_H + ENGINE_GAP + CHIP_H);
        let free = height - SWITCH_H - CHIP_H - spark * SPARK_H;
        let gap = (free / 3).max(ENGINE_GAP);
        (free.saturating_sub(gap) / 2, gap)
    } else {
        (height.saturating_sub(HUB_H + ENGINE_GAP + BOX_H) / 2, ENGINE_GAP)
    };
    out.push((NodeId::Hub, Placed { x: hub_x, y: hub_y, w: hub_w, h: hub_h }));

    if k > 0 {
        let inner = hub_w.saturating_sub(2 + led + 2 * PAD);
        let widths = fit_widths(hooks, inner.saturating_sub(2 * (k - 1)));
        let used = widths.iter().sum::<u16>() + 2 * (k - 1);
        let mut x = hub_x + 1 + led + (hub_w.saturating_sub(2 + led)).saturating_sub(used) / 2;
        for (hook, w) in hooks.iter().zip(widths) {
            out.push((NodeId::Port(hook.clone()), Placed { x, y: hub_y + 1, w, h: BOX_H }));
            x += w + 2;
        }
    }

    // Chips are spread across the whole region; if they do not fit with air, they are crowded together, centred.
    let engines: Vec<&str> = t.backends().map(|(name, _)| name).collect();
    let engine_y = hub_y + hub_h + engine_gap;
    let spread = m > 0 && avail / (m + 1) > ENGINE_W + CHIP_GAP;
    let step = if spread {
        0
    } else if m * ENGINE_W + m.saturating_sub(1) * CHIP_GAP <= avail {
        ENGINE_W + CHIP_GAP
    } else {
        ENGINE_W
    };
    let packed = (m * ENGINE_W + m.saturating_sub(1) * (step - ENGINE_W.min(step))).min(avail);
    for (i, name) in engines.into_iter().enumerate() {
        let i = i as u16;
        let x = if spread {
            (region_x + avail * (i + 1) / (m + 1)).saturating_sub(ENGINE_W / 2)
        } else {
            region_x + (avail - packed) / 2 + i * step
        };
        let x = x.min(width.saturating_sub(ENGINE_W));
        out.push((NodeId::Backend(name.to_string()), Placed { x, y: engine_y, w: ENGINE_W, h: engine_h }));
    }

    out
}

/// Width of the clients column: whatever the longest name asks for (plus the two borders),
/// never less than `CLIENT_W` nor more than a third of the area, as the hub needs the rest.
fn client_width(longest: u16, width: u16) -> u16 {
    let cap = (width / 3).clamp(CLIENT_W, CLIENT_MAX_W);
    (longest + 2).clamp(CLIENT_W, cap).min(width)
}

/// Port widths: `PORT_W` each; if they do not fit in `room`, the widest are trimmed one at a time,
/// first those with room to spare over their label and then, with no remedy left, any.
fn fit_widths(names: &[String], room: u16) -> Vec<u16> {
    let need: Vec<u16> = names.iter().map(|n| (n.chars().count() as u16 + 2).max(PORT_MIN_W)).collect();
    let mut widths = vec![PORT_W; names.len()];
    while widths.iter().sum::<u16>() > room {
        let slack = widths.iter().zip(&need).enumerate().filter(|(_, (w, n))| w > n).max_by_key(|(_, (w, _))| **w);
        let any = widths.iter().enumerate().filter(|(_, w)| **w > 1).max_by_key(|(_, w)| **w);
        match slack.map(|(i, _)| i).or(any.map(|(i, _)| i)) {
            Some(i) => widths[i] -= 1,
            None => break,
        }
    }
    widths
}

/// Sparkline row under a chip, if the chip is hardware and there is room in `height`.
pub fn sparkline_row(chip: Placed, height: u16) -> Option<Placed> {
    let y = chip.y + chip.h;
    (chip.h == CHIP_H && y + SPARK_H <= height).then_some(Placed { x: chip.x, y, w: chip.w, h: SPARK_H })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevalan_events::{Endpoint, Event, EventKind, SCHEMA_VERSION};
    use std::time::{Duration, Instant};

    fn topo_with_clients(n: usize) -> Topology {
        let mut t = Topology::new();
        let t0 = Instant::now();
        for (flow, backend) in [("agent-guard", "typesafe"), ("model-route", "ollama")] {
            let definition = serde_json::json!({ "nodes": { "n": { "backend": backend } } });
            let kind = EventKind::FlowLoaded { flow: flow.into(), nodes: 1, definition };
            let e = Event {
                v: SCHEMA_VERSION,
                id: "x".into(),
                ts: String::new(),
                trace_id: format!("flow:{flow}:1"),
                seq: 1,
                source: "test".into(),
                kind,
            };
            t.reduce(&e, t0);
        }
        for i in 0..n {
            let e = Event {
                v: SCHEMA_VERSION,
                id: "x".into(),
                ts: String::new(),
                trace_id: format!("t{i}"),
                seq: 1,
                source: "test".into(),
                kind: EventKind::RequestReceived {
                    client: format!("client-{i}"),
                    endpoint: Endpoint::Decide,
                    flow: None,
                    backend: None,
                },
            };
            t.reduce(&e, t0 + Duration::from_millis(i as u64));
        }
        t
    }

    fn get(placed: &[(NodeId, Placed)], id: &NodeId) -> Placed {
        placed.iter().find(|(n, _)| n == id).map(|(_, p)| *p).expect("nodo colocado")
    }

    /// Topology with a single client with a custom name: it is the name that rules the width.
    fn topo_with_named_client(name: &str) -> Topology {
        let mut t = Topology::new();
        let e = Event {
            v: SCHEMA_VERSION,
            id: "x".into(),
            ts: String::new(),
            trace_id: "t".into(),
            seq: 1,
            source: "test".into(),
            kind: EventKind::RequestReceived {
                client: name.into(),
                endpoint: Endpoint::Decide,
                flow: None,
                backend: None,
            },
        };
        t.reduce(&e, Instant::now());
        t
    }

    fn overlaps(a: Placed, b: Placed) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn everything_fits_inside_the_area() {
        for clients in 0..=8 {
            let t = topo_with_clients(clients);
            for (w, h) in [(60, 10), (80, 13), (120, 30), (200, 60)] {
                let placed = layout(&t, w, h);
                for (id, p) in &placed {
                    assert!(p.x + p.w <= w && p.y + p.h <= h, "{id:?} {p:?} fuera de {w}x{h} con {clients} clientes");
                }
            }
        }
    }

    #[test]
    fn places_every_node_once() {
        let t = topo_with_clients(7);
        let placed = layout(&t, 120, 30);
        let ids: Vec<&NodeId> = placed.iter().map(|(n, _)| n).collect();
        assert_eq!(ids.iter().filter(|n| matches!(n, NodeId::Client(_))).count(), 5);
        assert!(ids.contains(&&NodeId::Overflow));
        assert!(ids.contains(&&NodeId::Hub));
        assert_eq!(ids.iter().filter(|n| matches!(n, NodeId::Port(_))).count(), 3);
        assert_eq!(ids.iter().filter(|n| matches!(n, NodeId::Backend(_))).count(), 2);
    }

    #[test]
    fn ports_sit_inside_the_hub_and_nothing_else_overlaps() {
        let t = topo_with_clients(8);
        for (w, h) in [(80, 13), (120, 30)] {
            let placed = layout(&t, w, h);
            let hub = get(&placed, &NodeId::Hub);
            for (id, p) in &placed {
                match id {
                    NodeId::Port(_) => {
                        assert!(p.x > hub.x && p.x + p.w < hub.x + hub.w && p.y > hub.y && p.y + p.h < hub.y + hub.h)
                    }
                    NodeId::Hub => {}
                    _ => assert!(!overlaps(*p, hub), "{id:?} pisa el hub en {w}x{h}"),
                }
            }
            let others: Vec<Placed> =
                placed.iter().filter(|(n, _)| !matches!(n, NodeId::Hub)).map(|(_, p)| *p).collect();
            for (i, a) in others.iter().enumerate() {
                for b in &others[i + 1..] {
                    assert!(!overlaps(*a, *b), "{a:?} y {b:?} se solapan en {w}x{h}");
                }
            }
        }
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        let t = topo_with_clients(8);
        for (w, h) in [(0, 0), (1, 1), (10, 3)] {
            let _ = layout(&t, w, h);
        }
    }

    #[test]
    fn the_client_column_grows_with_the_longest_name() {
        let name = "app/desktop:ab12";
        let t = topo_with_named_client(name);
        for w in [80u16, 120, 200] {
            let placed = layout(&t, w, 30);
            let c = get(&placed, &NodeId::Client(name.into()));
            assert_eq!(c.w, name.chars().count() as u16 + 2, "el nombre debe caber entero a {w} columnas");
            assert!(c.w <= CLIENT_MAX_W, "la columna no puede comerse al hub: {} a {w}", c.w);
            assert!(get(&placed, &NodeId::Hub).x >= c.w + MIN_GAP, "sin aire tras los clientes a {w}");
        }
    }

    #[test]
    fn short_names_keep_the_column_at_its_minimum() {
        let t = topo_with_named_client("crush");
        assert_eq!(get(&layout(&t, 200, 30), &NodeId::Client("crush".into())).w, CLIENT_W);
    }

    #[test]
    fn a_name_too_wide_even_at_the_cap_takes_a_second_line() {
        let name = "claude-code/mi-proyecto-largo:1a2b";
        let t = topo_with_named_client(name);
        let c = get(&layout(&t, 200, 30), &NodeId::Client(name.into()));
        assert_eq!(c.w, CLIENT_MAX_W, "el ancho se queda en el tope");
        assert_eq!(c.h, BOX_H + 1, "sin ancho para el nombre, se le da alto");
    }

    #[test]
    fn without_room_for_a_second_line_the_box_stays_flat() {
        let name = "app/desktop:ab12";
        let t = topo_with_named_client(name);
        assert_eq!(get(&layout(&t, 80, 3), &NodeId::Client(name.into())).h, BOX_H);
        assert_eq!(get(&layout(&t, 80, 1), &NodeId::Client(name.into())).h, 1);
    }

    #[test]
    fn layout_is_stable_when_nothing_changes() {
        let t = topo_with_clients(3);
        assert_eq!(layout(&t, 100, 20), layout(&t, 100, 20));
    }

    #[test]
    fn hardware_mode_uses_switch_and_chip_heights() {
        let t = topo_with_clients(2);
        let placed = layout(&t, 120, 30);
        let hub = get(&placed, &NodeId::Hub);
        assert_eq!(hub.h, SWITCH_H, "hub debe usar SWITCH_H en hardware mode");

        for (id, p) in &placed {
            if matches!(id, NodeId::Backend(_)) {
                assert_eq!(p.h, CHIP_H, "motor debe usar CHIP_H en hardware mode");
                assert!(p.y >= hub.y + hub.h, "motor no debe solapar con hub");
            }
            if matches!(id, NodeId::Port(_)) {
                assert!(p.x >= hub.x + 1 + LED_PANEL_W, "hook debe estar después del LED panel");
            }
        }
    }

    #[test]
    fn short_areas_fall_back_to_flat_boxes() {
        let t = topo_with_clients(2);
        let placed = layout(&t, 120, 12);
        let hub = get(&placed, &NodeId::Hub);
        assert_eq!(hub.h, 5, "hub debe usar HUB_H en modo plano");

        for (id, p) in &placed {
            if matches!(id, NodeId::Backend(_)) {
                assert_eq!(p.h, 3, "motor debe usar BOX_H en modo plano");
            }
        }
    }

    fn ports_n(n: usize) -> Topology {
        let mut t = Topology::new();
        let t0 = Instant::now();
        for i in 0..n {
            let definition = serde_json::json!({ "nodes": { "n": { "backend": "ollama" } } });
            let kind = EventKind::FlowLoaded { flow: format!("f{i}"), nodes: 1, definition };
            let e = Event {
                v: SCHEMA_VERSION,
                id: "x".into(),
                ts: String::new(),
                trace_id: format!("flow:f{i}:1"),
                seq: 1,
                source: "test".into(),
                kind,
            };
            t.reduce(&e, t0);
        }
        t
    }

    #[test]
    fn hub_is_sized_to_its_ports_not_the_area() {
        let placed = layout(&topo_with_clients(2), 200, 30);
        assert!(get(&placed, &NodeId::Hub).w < 100);
    }

    #[test]
    fn hub_leaves_air_after_the_clients() {
        for w in [100, 160] {
            let placed = layout(&topo_with_clients(2), w, 30);
            assert!(get(&placed, &NodeId::Hub).x >= CLIENT_W + MIN_GAP, "sin aire a {w} columnas");
        }
    }

    #[test]
    fn hub_is_centred() {
        let t = topo_with_clients(2);
        for w in [100u16, 160, 200] {
            let hub = get(&layout(&t, w, 30), &NodeId::Hub);
            let left = hub.x - (CLIENT_W + MIN_GAP);
            let right = w - (hub.x + hub.w);
            assert!(left.abs_diff(right) <= 1, "descentrado a {w}: {left} vs {right}");
        }
    }

    #[test]
    fn many_ports_still_fit() {
        let t = ports_n(8);
        let placed = layout(&t, 100, 30);
        let hub = get(&placed, &NodeId::Hub);
        let ports: Vec<Placed> = placed.iter().filter(|(n, _)| matches!(n, NodeId::Port(_))).map(|(_, p)| *p).collect();
        assert_eq!(ports.len(), 9);
        for (i, p) in ports.iter().enumerate() {
            assert!(p.x > hub.x && p.x + p.w < hub.x + hub.w, "puerto fuera del hub");
            for q in &ports[i + 1..] {
                assert!(!overlaps(*p, *q));
            }
        }
    }

    fn hub_and_chips(t: &Topology, w: u16, h: u16) -> (Placed, Vec<Placed>) {
        let placed = layout(t, w, h);
        let chips = placed.iter().filter(|(n, _)| matches!(n, NodeId::Backend(_))).map(|(_, p)| *p).collect();
        (get(&placed, &NodeId::Hub), chips)
    }

    #[test]
    fn chip_gap_grows_with_height() {
        let t = topo_with_clients(2);
        let gap = |h| {
            let (hub, chips) = hub_and_chips(&t, 120, h);
            chips[0].y - (hub.y + hub.h)
        };
        assert!(gap(30) > gap(15), "{} vs {}", gap(30), gap(15));
        assert!(gap(15) >= ENGINE_GAP);
    }

    #[test]
    fn chips_are_spread_with_air() {
        let (_, mut chips) = hub_and_chips(&topo_with_clients(2), 120, 30);
        chips.sort_by_key(|c| c.x);
        assert_eq!(chips.len(), 2);
        assert!(chips[1].x - (chips[0].x + chips[0].w) >= CHIP_GAP);
    }

    #[test]
    fn chips_never_overlap_hub_or_clients() {
        for engines in 1..=4 {
            let mut t = topo_with_clients(4);
            for e in 0..engines {
                let definition = serde_json::json!({ "nodes": { "n": { "backend": format!("be{e}") } } });
                let kind = EventKind::FlowLoaded { flow: format!("g{e}"), nodes: 1, definition };
                let ev = Event {
                    v: SCHEMA_VERSION,
                    id: "x".into(),
                    ts: String::new(),
                    trace_id: format!("flow:g{e}:1"),
                    seq: 1,
                    source: "test".into(),
                    kind,
                };
                t.reduce(&ev, Instant::now());
            }
            for (w, h) in [(100, 20), (120, 30), (160, 40), (200, 60)] {
                let m = t.backends().count() as u16;
                if m * ENGINE_W > w - CLIENT_W - MIN_GAP {
                    continue;
                }
                let placed = layout(&t, w, h);
                let chips: Vec<Placed> =
                    placed.iter().filter(|(n, _)| matches!(n, NodeId::Backend(_))).map(|(_, p)| *p).collect();
                for (i, c) in chips.iter().enumerate() {
                    assert!(c.x + c.w <= w && c.y + c.h <= h);
                    for (id, p) in &placed {
                        if !matches!(id, NodeId::Backend(_)) && !matches!(id, NodeId::Port(_)) {
                            assert!(!overlaps(*c, *p), "{id:?} pisa un chip en {w}x{h} con {engines} motores");
                        }
                    }
                    for d in &chips[i + 1..] {
                        assert!(!overlaps(*c, *d), "chips solapados en {w}x{h}");
                    }
                }
            }
        }
    }

    #[test]
    fn hub_no_longer_depends_on_chips() {
        let t = topo_with_clients(2);
        let k = t.ports().len() as u16;
        let content = 2 + LED_PANEL_W + k * PORT_W + 2 * (k - 1) + 2 * PAD;
        assert_eq!(get(&layout(&t, 160, 30), &NodeId::Hub).w, content);
    }

    #[test]
    fn sparkline_row_only_when_there_is_room() {
        let t = topo_with_clients(2);
        for (w, h, room) in [(120, 30, true), (120, 14, true), (120, 13, false), (120, 12, false)] {
            let placed = layout(&t, w, h);
            for (id, p) in &placed {
                if !matches!(id, NodeId::Backend(_)) {
                    continue;
                }
                match sparkline_row(*p, h) {
                    Some(r) => {
                        assert!(room, "no debería haber fila a {h}");
                        assert_eq!((r.x, r.y, r.w, r.h), (p.x, p.y + p.h, p.w, 1));
                        assert!(r.y < h);
                    }
                    None => assert!(!room, "faltó la fila a {h}"),
                }
            }
        }
    }
}
