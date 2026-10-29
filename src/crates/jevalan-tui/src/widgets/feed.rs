//! Feed of recent decisions, selectable and navigable.

use crate::screen::Ctx;
use crate::store::Decision;
use crate::theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Widget};

/// A decision's line: `{hh:mm:ss} {client:<10} {flow:<14} {outcome:<9} {confidence:.2}`.
/// A hub response carries the capability, the `status` and the milliseconds in place of the confidence.
pub fn row(d: &Decision) -> Line<'static> {
    let ts_str = if d.ts.len() >= 19 { d.ts[11..19].to_string() } else { "--:--:--".to_string() };
    let (style, tail) = match d.hub_ms {
        Some(ms) => (theme::hub_status(&d.outcome), format!(" {ms}ms")),
        None => (theme::outcome(&d.outcome), format!(" {:.2}", d.confidence)),
    };
    Line::from(vec![
        Span::raw(format!("{ts_str} {:<10.10} {:<14.14} ", d.client, d.flow)),
        Span::styled(format!("{:<9}", d.outcome), style),
        Span::raw(tail),
    ])
}

/// Draws the feed panel with the recent decisions.
pub fn render(ctx: &Ctx, selected: usize, focused: bool, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let decisions: Vec<&Decision> = ctx.store.decisions().collect();

    if decisions.is_empty() {
        return;
    }

    let items: Vec<ListItem> = decisions.iter().map(|d| ListItem::new(row(d))).collect();

    let _state = ListState::default().with_selected(Some(selected));
    let list = if focused { List::new(items).style(theme::accent()) } else { List::new(items) };

    list.render(area, buf);

    // Apply inverted highlight if focused.
    if focused && selected < decisions.len() {
        for x in area.left()..area.right() {
            if let Some(y) = area.top().checked_add(selected as u16)
                && y < area.bottom()
                && buf.area.contains((x, y).into())
            {
                let cell = &mut buf[(x, y)];
                cell.set_style(theme::accent());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Decision;
    use std::time::Instant;

    fn plain(l: Line) -> String {
        l.spans.iter().map(|s| s.content.to_string()).collect()
    }

    fn decision(ts: &str, client: &str) -> Decision {
        Decision {
            trace_id: "t".into(),
            ts: ts.into(),
            at: Instant::now(),
            outcome: "ask-user".into(),
            client: client.into(),
            flow: "agent-guard".into(),
            confidence: 0.876,
            hub_ms: None,
        }
    }

    #[test]
    fn row_format() {
        assert_eq!(
            plain(row(&decision("2026-10-07T14:02:11Z", "crush"))),
            "14:02:11 crush      agent-guard    ask-user  0.88"
        );
        assert!(plain(row(&decision("", "a-very-long-client-name"))).starts_with("--:--:-- a-very-lon "));
    }

    #[test]
    fn hub_rows_show_capability_status_and_milliseconds() {
        let d = Decision {
            outcome: "abstain".into(),
            flow: "tool.risk".into(),
            confidence: 0.0,
            hub_ms: Some(412),
            ..decision("2026-10-07T14:02:11Z", "vscode")
        };
        let line = row(&d);
        assert_eq!(plain(line.clone()), "14:02:11 vscode     tool.risk      abstain   412ms");
        assert_eq!(line.spans[1].style, theme::hub_status("abstain"));
        for (status, color) in [("ok", theme::ALLOW), ("abstain", theme::ABSTAIN), ("error", theme::FAILED)] {
            assert_eq!(theme::hub_status(status).fg, Some(color));
        }
    }

    #[test]
    fn outcome_takes_its_theme_color() {
        let line = row(&decision("", "c"));
        assert_eq!(line.spans[1].style, theme::outcome("ask-user"));
    }
}
