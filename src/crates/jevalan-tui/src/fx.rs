//! Modo de efectos visuales.

use crate::theme;
use crate::topology::{Cue, NodeId};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::fmt;
use std::str::FromStr;
use std::time::Instant;
use tachyonfx::{Effect, EffectManager, Interpolation, Motion, fx};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FxMode {
    Off,
    #[default]
    Events,
    Full,
}

impl FxMode {
    pub fn next(self) -> Self {
        match self {
            FxMode::Off => FxMode::Events,
            FxMode::Events => FxMode::Full,
            FxMode::Full => FxMode::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FxMode::Off => "off",
            FxMode::Events => "events",
            FxMode::Full => "full",
        }
    }
}

impl fmt::Display for FxMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FxMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" => Ok(FxMode::Off),
            "events" => Ok(FxMode::Events),
            "full" => Ok(FxMode::Full),
            other => Err(format!("fx desconocido: {other} (usa off, events o full)")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxKind {
    Flash,
    FadeIn,
    FadeOut,
    Blink,
}

/// Qué efecto toca para un cue en un modo dado; `Events` y `Full` coinciden.
pub fn effect_for(cue: &Cue, mode: FxMode) -> Option<(FxKind, NodeId)> {
    if mode == FxMode::Off {
        return None;
    }
    match cue {
        Cue::Denied(h) => Some((FxKind::Flash, NodeId::Port(h.clone()))),
        Cue::ClientJoined(c) => Some((FxKind::FadeIn, NodeId::Client(c.clone()))),
        Cue::ClientLeft(c) => Some((FxKind::FadeOut, NodeId::Client(c.clone()))),
        Cue::BackendDown(e) => Some((FxKind::Blink, NodeId::Backend(e.clone()))),
        Cue::Error(_) => None,
    }
}

fn build(kind: FxKind, area: Rect) -> Effect {
    let effect = match kind {
        FxKind::Flash => fx::fade_from_fg(theme::DENY, (400, Interpolation::QuadOut)),
        FxKind::FadeIn => fx::fade_from_fg(theme::BACKDROP, (500, Interpolation::QuadOut)),
        FxKind::FadeOut => fx::dissolve((500, Interpolation::Linear)),
        FxKind::Blink => fx::repeat(
            fx::sequence(&[fx::fade_to_fg(theme::DENY, 150), fx::fade_from_fg(theme::DENY, 150)]),
            fx::RepeatMode::Times(3),
        ),
    };
    effect.with_area(area)
}

/// Efectos en curso y último instante procesado.
pub struct FxState {
    manager: EffectManager<String>,
    last: Option<Instant>,
    ambient: bool,
}

impl Default for FxState {
    fn default() -> Self {
        Self::new()
    }
}

impl FxState {
    pub fn new() -> Self {
        Self { manager: EffectManager::default(), last: None, ambient: false }
    }

    pub fn on_cue(&mut self, cue: &Cue, mode: FxMode, area_of: impl Fn(&NodeId) -> Option<Rect>) {
        if let Some((kind, id)) = effect_for(cue, mode)
            && let Some(area) = area_of(&id)
        {
            self.manager.add_effect(build(kind, area));
        }
    }

    /// Destello suave de la fila que acaba de llegar a la lista de decisiones del hook.
    pub fn on_arrival(&mut self, mode: FxMode, area: Rect) {
        if mode != FxMode::Off {
            self.manager.add_effect(fx::fade_from_fg(theme::ACCENT, (700, Interpolation::QuadOut)).with_area(area));
        }
    }

    /// Transición de entrada del dashboard: `coalesce` en `Events` y `Full`.
    pub fn on_start(&mut self, mode: FxMode, area: Rect) {
        if mode != FxMode::Off {
            self.manager.add_effect(fx::coalesce((600, Interpolation::QuadOut)).with_area(area));
        }
    }

    /// Transición al cambiar de pantalla, solo en `Full`.
    pub fn on_enter_screen(&mut self, mode: FxMode, area: Rect) {
        if mode == FxMode::Full {
            let sweep = fx::sweep_in(Motion::LeftToRight, 10, 0, theme::ACCENT, (400, Interpolation::QuadOut));
            self.manager.add_effect(sweep.with_area(area));
        }
    }

    /// Fondo ambiental sobre `area`: solo en `Full`; en otro modo se cancela.
    pub fn sync_ambient(&mut self, mode: FxMode, area: Rect) {
        match (mode == FxMode::Full, self.ambient) {
            (true, false) => {
                let wave = fx::never_complete(fx::ping_pong(fx::hsl_shift_fg([0.0, 0.0, 8.0], 3000)));
                self.manager.add_unique_effect("ambient", wave.with_area(area));
                self.ambient = true;
            }
            (false, true) => {
                self.manager.cancel_unique_effect("ambient");
                self.ambient = false;
            }
            _ => {}
        }
    }

    pub fn process(&mut self, mode: FxMode, now: Instant, buf: &mut Buffer, area: Rect) {
        if mode == FxMode::Off {
            self.manager = EffectManager::default();
            self.ambient = false;
            self.last = None;
            return;
        }
        let delta = self.last.map_or(std::time::Duration::ZERO, |l| now.saturating_duration_since(l));
        self.last = Some(now);
        self.manager.process_effects(delta.into(), buf, area);
    }

    pub fn is_running(&self) -> bool {
        self.manager.is_running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cycles_and_parses() {
        assert_eq!(FxMode::default(), FxMode::Events);
        assert_eq!(FxMode::Off.next().next().next(), FxMode::Off);
        for m in [FxMode::Off, FxMode::Events, FxMode::Full] {
            assert_eq!(m.as_str().parse::<FxMode>(), Ok(m));
        }
        assert!("loud".parse::<FxMode>().is_err());
    }

    #[test]
    fn effect_table() {
        assert_eq!(effect_for(&Cue::Denied("h".into()), FxMode::Off), None);
        assert_eq!(
            effect_for(&Cue::Denied("h".into()), FxMode::Events),
            Some((FxKind::Flash, NodeId::Port("h".into())))
        );
        assert_eq!(
            effect_for(&Cue::ClientJoined("c".into()), FxMode::Full),
            Some((FxKind::FadeIn, NodeId::Client("c".into())))
        );
        assert_eq!(
            effect_for(&Cue::ClientLeft("c".into()), FxMode::Events),
            Some((FxKind::FadeOut, NodeId::Client("c".into())))
        );
        assert_eq!(
            effect_for(&Cue::BackendDown("jev".into()), FxMode::Events),
            Some((FxKind::Blink, NodeId::Backend("jev".into())))
        );
        assert_eq!(effect_for(&Cue::Error("x".into()), FxMode::Full), None);
    }

    #[test]
    fn off_mode_never_runs_effects() {
        let area = Rect::new(0, 0, 20, 5);
        let mut fx = FxState::new();
        fx.on_start(FxMode::Off, area);
        fx.on_cue(&Cue::Denied("h".into()), FxMode::Off, |_| Some(area));
        let mut buf = Buffer::empty(area);
        fx.process(FxMode::Off, Instant::now(), &mut buf, area);
        assert!(!fx.is_running());
    }

    #[test]
    fn events_effect_finishes_and_off_clears_it() {
        let area = Rect::new(0, 0, 20, 5);
        let t0 = Instant::now();
        let mut fx = FxState::new();
        fx.on_cue(&Cue::Denied("h".into()), FxMode::Events, |_| Some(area));
        assert!(fx.is_running());
        let mut buf = Buffer::empty(area);
        fx.process(FxMode::Events, t0, &mut buf, area);
        assert!(fx.is_running());
        fx.process(FxMode::Events, t0 + Duration::from_secs(2), &mut buf, area);
        assert!(!fx.is_running());
        fx.on_cue(&Cue::Denied("h".into()), FxMode::Events, |_| Some(area));
        fx.process(FxMode::Off, t0, &mut buf, area);
        assert!(!fx.is_running());
    }

    #[test]
    fn ambient_only_in_full_and_cancelled_after() {
        let area = Rect::new(0, 0, 20, 5);
        let t0 = Instant::now();
        let mut buf = Buffer::empty(area);
        let mut fx = FxState::new();
        fx.sync_ambient(FxMode::Events, area);
        assert!(!fx.is_running());
        fx.sync_ambient(FxMode::Full, area);
        fx.process(FxMode::Full, t0, &mut buf, area);
        fx.process(FxMode::Full, t0 + Duration::from_secs(10), &mut buf, area);
        assert!(fx.is_running());
        fx.sync_ambient(FxMode::Events, area);
        fx.process(FxMode::Events, t0 + Duration::from_secs(11), &mut buf, area);
        fx.process(FxMode::Events, t0 + Duration::from_secs(12), &mut buf, area);
        assert!(!fx.is_running());
    }
}
