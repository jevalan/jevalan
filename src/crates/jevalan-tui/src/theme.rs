//! Palette and styles: the TUI's only source of colours.

use crate::hook_log::{Kind, Level};
use crate::topology::PacketStatus;
use ratatui::style::{Color, Modifier, Style};

pub const ALLOW: Color = Color::Green;
pub const DENY: Color = Color::Red;
pub const ABSTAIN: Color = Color::Yellow;
pub const FAILED: Color = Color::LightRed;
pub const IDLE: Color = Color::DarkGray;
pub const ACCENT: Color = Color::Cyan;
pub const TEXT: Color = Color::Gray;
pub const BACKDROP: Color = Color::Black;

/// Colour of a flow outcome; unknown ones take the accent.
pub fn outcome(o: &str) -> Style {
    match o {
        "allow" | "ok" => Style::new().fg(ALLOW),
        "deny" => Style::new().fg(DENY),
        "error" => Style::new().fg(FAILED),
        "ask-user" => Style::new().fg(ABSTAIN),
        "abstain" => Style::new().fg(ABSTAIN).add_modifier(Modifier::DIM),
        _ => Style::new().fg(ACCENT),
    }
}

/// Colour of the `status` of a hub response: `ok`, `abstain` or `error`.
pub fn hub_status(s: &str) -> Style {
    match s {
        "ok" => Style::new().fg(ALLOW),
        "abstain" => Style::new().fg(ABSTAIN),
        "error" => Style::new().fg(FAILED),
        _ => Style::new().fg(ACCENT),
    }
}

/// Label of an `exec`/`http` specialist, to tell it apart from System One backends.
pub fn provider() -> Style {
    accent()
}

pub fn packet(s: &PacketStatus) -> Style {
    match s {
        PacketStatus::Ok => Style::new().fg(ALLOW),
        PacketStatus::Failed => Style::new().fg(FAILED),
        PacketStatus::Outcome(o) => outcome(o),
        PacketStatus::Hub(s) => hub_status(s),
    }
}

/// Fill colour of a hook verdict's label.
pub fn verdict_color(kind: Kind) -> Color {
    match kind {
        Kind::Allow => ALLOW,
        Kind::Ask => ABSTAIN,
        Kind::Deny => DENY,
        Kind::Abstain => IDLE,
        Kind::Error => Color::Magenta,
        Kind::Other => ACCENT,
    }
}

/// Label with a coloured background, like a rack chip.
pub fn chip(kind: Kind) -> Style {
    let fg = if kind == Kind::Abstain { TEXT } else { BACKDROP };
    Style::new().fg(fg).bg(verdict_color(kind)).add_modifier(Modifier::BOLD)
}

/// Colour of a probability bar according to its band.
pub fn level_color(level: Level) -> Color {
    match level {
        Level::Low => ALLOW,
        Level::Mid => ABSTAIN,
        Level::High => DENY,
    }
}

pub fn text() -> Style {
    Style::new().fg(TEXT)
}

pub fn dim() -> Style {
    Style::new().fg(IDLE)
}

pub fn led_off() -> Style {
    Style::new().fg(IDLE)
}

pub fn accent() -> Style {
    Style::new().fg(ACCENT)
}

/// Latency history under each backend.
pub fn spark() -> Style {
    accent()
}

pub fn title() -> Style {
    accent().add_modifier(Modifier::BOLD)
}

pub fn error() -> Style {
    Style::new().fg(FAILED).add_modifier(Modifier::BOLD)
}
