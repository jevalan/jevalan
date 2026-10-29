//! Summary metrics bar and detail according to the selection.

use crate::screen::Ctx;
use crate::theme;
use crate::topology::NodeId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Sparkline, Widget};

/// Line of aggregated metrics: `allow 70% · deny 10% · err 0 · typesafe 120ms · ollama 40ms`
pub fn summary(ctx: &Ctx) -> Line<'static> {
    let mut top: Vec<(&String, &u64)> = ctx.store.outcomes().iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

    let mut spans = Vec::new();
    for (name, _) in top.into_iter().take(4) {
        spans.push(Span::styled(format!("{name} "), theme::outcome(name)));
        spans.push(Span::styled(format!("{}% · ", ctx.store.outcome_pct(name)), theme::text()));
    }
    spans.push(Span::styled("err ", theme::text()));
    spans.push(Span::raw(format!("{}", ctx.store.errors())));
    for name in ctx.topo.backends().map(|(n, _)| n) {
        spans.push(Span::styled(format!(" · {name} "), theme::text()));
        spans.push(Span::raw(match ctx.store.backend(name).and_then(|b| b.last_ms) {
            Some(ms) => format!("{ms}ms"),
            None => "—".into(),
        }));
    }
    Line::from(spans)
}

/// Metrics line for a specific node.
pub fn detail(ctx: &Ctx, id: &NodeId) -> Line<'static> {
    match id {
        NodeId::Client(name) => {
            let stats = ctx.store.client(name);
            let ago = stats.map(|s| ctx.now.saturating_duration_since(s.last_seen).as_secs()).unwrap_or(0);
            Line::from(vec![
                Span::raw(format!("{} ", name)),
                Span::styled("· ", theme::text()),
                Span::raw(format!("{} peticiones ", stats.map(|s| s.requests).unwrap_or(0))),
                Span::styled("· visto hace ", theme::text()),
                Span::raw(format!("{}s", ago)),
            ])
        }
        NodeId::Port(name) => {
            let mut spans = vec![Span::raw(format!("{name} "))];
            let outcomes = ctx.store.flow(name);
            for (outcome, n) in outcomes.into_iter().flatten() {
                spans.push(Span::styled("· ", theme::text()));
                spans.push(Span::styled(format!("{outcome} "), theme::outcome(outcome)));
                spans.push(Span::raw(format!("{n} ")));
            }
            if outcomes.is_none() {
                spans.push(Span::styled("· sin decisiones", theme::text()));
            }
            Line::from(spans)
        }
        NodeId::Backend(name) => {
            let stats = ctx.store.backend(name);
            let (calls, failures, avg_str) = match stats {
                Some(s) => (s.calls, s.failures, s.avg_ms().map(|a| format!("{:.0}ms", a)).unwrap_or("—".into())),
                None => (0, 0, "—".into()),
            };
            Line::from(vec![
                Span::raw(format!("{} ", name)),
                Span::styled("· ", theme::text()),
                Span::raw(format!("{} llamadas ", calls)),
                Span::styled("· ", theme::text()),
                Span::raw(format!("{} fallos ", failures)),
                Span::styled("· media ", theme::text()),
                Span::raw(avg_str),
            ])
        }
        NodeId::Hub => {
            let count = ctx.store.decisions().count();
            Line::from(vec![
                Span::raw("jevalan "),
                Span::styled("· ", theme::text()),
                Span::raw(format!("{} decisiones", count)),
            ])
        }
        NodeId::Overflow => {
            let n = ctx.topo.overflow();
            Line::from(vec![Span::raw(format!("+{} clientes más", n))])
        }
    }
}

/// Draws the metrics panel: sparkline on the left, detail/summary on the right.
pub fn render(ctx: &Ctx, selected: Option<&NodeId>, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    // Sparkline a la izquierda: 30 columnas.
    let sparkline_width = 30.min(area.width as usize);
    if sparkline_width > 0 {
        let data = ctx.store.sparkline(ctx.now);
        // Take the last 30 (or as many as fit).
        let start = data.len().saturating_sub(sparkline_width);
        let sparkline_data: Vec<u64> = data[start..].to_vec();

        let sparkline = Sparkline::default()
            .data(&sparkline_data)
            .style(theme::accent())
            .max(sparkline_data.iter().max().copied().unwrap_or(1));

        let sparkline_area =
            Rect { x: area.x, y: area.y, width: sparkline_width.min(u16::MAX as usize) as u16, height: area.height };
        sparkline.render(sparkline_area, buf);
    }

    // Texto a la derecha.
    let text_area = Rect {
        x: area.x + sparkline_width.min(area.width as usize) as u16,
        y: area.y,
        width: area.width.saturating_sub(sparkline_width.min(area.width as usize) as u16),
        height: area.height,
    };

    if text_area.width > 0 {
        let line = match selected {
            Some(id) => detail(ctx, id),
            None => summary(ctx),
        };
        line.render(text_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxMode;
    use crate::screen::Ctx;
    use crate::topology::NodeId;
    use jevalan_events::{Endpoint, Event, EventKind, SCHEMA_VERSION};
    use std::time::{Duration, Instant};

    fn ev(trace: &str, seq: u64, kind: EventKind) -> Event {
        Event {
            v: SCHEMA_VERSION,
            id: "x".into(),
            ts: String::new(),
            trace_id: trace.into(),
            seq,
            source: "t".into(),
            kind,
        }
    }

    fn plain(l: Line) -> String {
        l.spans.iter().map(|s| s.content.to_string()).collect()
    }

    fn ctx() -> Ctx {
        let now = Instant::now();
        let mut c = Ctx::new(FxMode::Events, now);
        let kinds = [
            EventKind::RequestReceived {
                client: "crush".into(),
                endpoint: Endpoint::Decide,
                flow: Some("agent-guard".into()),
                backend: None,
            },
            EventKind::BackendRequest { backend: "typesafe".into(), model: "m".into(), questions: 1 },
            EventKind::BackendResponse {
                backend: "typesafe".into(),
                ok: true,
                status: 200,
                duration_ms: 120,
                input_tokens: None,
            },
            EventKind::DecisionMade {
                flow: "agent-guard".into(),
                outcome: "deny".into(),
                path_len: 1,
                duration_ms: 130,
            },
        ];
        for (i, kind) in kinds.into_iter().enumerate() {
            let e = ev("a", i as u64 + 1, kind);
            c.topo.reduce(&e, now);
            c.store.ingest(e, now);
        }
        c.now = now + Duration::from_secs(3);
        c
    }

    #[test]
    fn summary_line() {
        assert_eq!(plain(summary(&ctx())), "deny 100% · err 0 · typesafe 120ms");
    }

    #[test]
    fn detail_lines() {
        let c = ctx();
        assert_eq!(plain(detail(&c, &NodeId::Client("crush".into()))), "crush · 1 peticiones · visto hace 3s");
        assert_eq!(plain(detail(&c, &NodeId::Port("agent-guard".into()))), "agent-guard · deny 1 ");
        assert_eq!(plain(detail(&c, &NodeId::Port("passthrough".into()))), "passthrough · sin decisiones");
        assert_eq!(
            plain(detail(&c, &NodeId::Backend("typesafe".into()))),
            "typesafe · 1 llamadas · 0 fallos · media 120ms"
        );
        assert_eq!(plain(detail(&c, &NodeId::Backend("ollama".into()))), "ollama · 0 llamadas · 0 fallos · media —");
        assert_eq!(plain(detail(&c, &NodeId::Hub)), "jevalan · 1 decisiones");
    }
}
