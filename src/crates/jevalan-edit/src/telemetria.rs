//! Live telemetry: the daemon's bus, to see what the published flow is deciding.

use jevalan_events::{Event, EventKind};
use jevalan_flow::Flow;
use jevalan_tui::flow_activity::FlowActivity;
use jevalan_tui::live;
use jevalan_tui::screen::{Feed, SourceStatus};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

/// The suffix with which a draft signs its events (section 5 of the spec).
pub const BORRADOR: &str = " (borrador)";

/// Is the event signed by a draft and not by the published flow?
pub fn es_borrador(flow: &str) -> bool {
    flow.ends_with(BORRADOR)
}

/// State of the connection to the bus, as the bottom band reports it.
#[derive(Debug, Clone, PartialEq)]
pub enum Estado {
    Conectando,
    Conectado,
    Caido(String),
    /// No bus: `--no-events` was requested, or there is no daemon to listen to.
    Apagado,
}

/// What the editor knows about the daemon through the event bus.
pub struct Telemetria {
    rx: Option<Receiver<Feed>>,
    /// Name of the flow being edited: only its decisions go to the band.
    nombre: String,
    /// Current flows by `FLOW_LOADED`: `FlowActivity` needs them to recognise its nodes.
    flows: BTreeMap<String, Arc<Flow>>,
    /// Edge traversals and in-flight packets of the published flow.
    pub activity: FlowActivity,
    pub estado: Estado,
    /// Summary of the published flow's last event, for the bottom band.
    pub ultimo: Option<String>,
    /// Backend catalogue the daemon declared. It is the one local validation uses when the daemon
    /// does not answer: the `BACKENDS` event only carries their names.
    pub backends: Vec<String>,
}

impl Telemetria {
    /// Connects to the daemon's bus, which retries patiently, like the dashboard.
    pub fn conectar(url: &str, token: Option<&str>, nombre: &str) -> Self {
        let mut telemetria = Self::apagada(nombre);
        telemetria.rx = Some(live::spawn(url.to_string(), token.map(str::to_string)));
        telemetria.estado = Estado::Conectando;
        telemetria
    }

    /// No bus: the editor works just the same, only without telemetry.
    pub fn apagada(nombre: &str) -> Self {
        Self {
            rx: None,
            nombre: nombre.to_string(),
            flows: BTreeMap::new(),
            activity: FlowActivity::new(),
            estado: Estado::Apagado,
            ultimo: None,
            backends: Vec::new(),
        }
    }

    /// Consumes whatever has arrived over the bus; returns `true` if something changed.
    pub fn latir(&mut self, ahora: Instant) -> bool {
        let mut cambio = false;
        // The receiver leaves `self` while draining: noting an event needs `&mut self`.
        if let Some(rx) = self.rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(Feed::Event(e)) => {
                        self.anotar(&e, ahora);
                        cambio = true;
                    }
                    Ok(Feed::Status(s)) => {
                        self.estado = match s {
                            SourceStatus::Connecting => Estado::Conectando,
                            SourceStatus::Connected => Estado::Conectado,
                            SourceStatus::Disconnected(motivo) => Estado::Caido(motivo),
                        };
                        cambio = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.estado = Estado::Caido("el bus se cerró".into());
                        break;
                    }
                }
            }
            self.rx = Some(rx);
        }
        // The edges' packets expire on their own: they have to be removed so the screen keeps going.
        if self.activity.has_packets() {
            self.activity.tick(ahora);
            cambio = true;
        }
        cambio
    }

    /// Notes an event: the backend catalogue, the current flows and the last thing the published
    /// flow decided. A draft does not count as a published decision, and `FlowActivity` ignores it by itself,
    /// because its name is not among the current flows.
    fn anotar(&mut self, e: &Event, ahora: Instant) {
        match &e.kind {
            EventKind::Backends { backends } => self.backends = backends.clone(),
            EventKind::FlowLoaded { flow, definition, .. } => {
                if let Ok(f) = serde_json::from_value::<Flow>(definition.clone()) {
                    self.flows.insert(flow.clone(), Arc::new(f));
                }
            }
            EventKind::FlowRemoved { flow } => {
                self.flows.remove(flow);
            }
            EventKind::DecisionMade { flow, outcome, duration_ms, .. }
                if flow == &self.nombre && !es_borrador(flow) =>
            {
                self.ultimo = Some(format!("{outcome} {duration_ms} ms"));
            }
            _ => {}
        }
        self.activity.on_event(&self.flows, e, ahora);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevalan_events::new_event;
    use serde_json::json;

    fn flujo() -> Flow {
        jevalan_flow::parse(
            "tool-risk-local",
            include_str!("../../jevalan-flow/fixtures/tool-risk-local-v1.yaml"),
            &["kev"],
            "kev",
        )
        .unwrap()
    }

    fn evento(kind: EventKind) -> Event {
        new_event("t", 1, "test", kind)
    }

    fn cargado(flujo: &Flow) -> Event {
        evento(EventKind::FlowLoaded {
            flow: flujo.name.clone(),
            nodes: flujo.nodes.len() as u32,
            definition: serde_json::to_value(flujo).unwrap(),
        })
    }

    fn decidido(flow: &str, outcome: &str, duration_ms: u64) -> Event {
        evento(EventKind::DecisionMade { flow: flow.into(), outcome: outcome.into(), path_len: 3, duration_ms })
    }

    /// Telemetry without a bus, already connected, so events can be noted by hand.
    fn sorda(nombre: &str) -> Telemetria {
        let mut telemetria = Telemetria::apagada(nombre);
        telemetria.estado = Estado::Conectado;
        telemetria
    }

    #[test]
    fn el_ultimo_evento_del_flujo_publicado_va_a_la_banda() {
        let flujo = flujo();
        let mut telemetria = sorda("tool-risk-local");
        telemetria.anotar(&cargado(&flujo), Instant::now());
        telemetria.anotar(&decidido("tool-risk-local", "allow", 274), Instant::now());
        assert_eq!(telemetria.ultimo.as_deref(), Some("allow 274 ms"));
    }

    #[test]
    fn los_eventos_de_un_borrador_ni_manchan_la_banda_ni_cuentan_como_flujo() {
        let mut telemetria = sorda("tool-risk-local");
        telemetria.anotar(&decidido("tool-risk-local (borrador)", "deny", 9), Instant::now());
        assert_eq!(telemetria.ultimo, None, "la decisión de un borrador no es la del flujo publicado");
        // And those of another flow do not count either: the band talks about the one being edited.
        telemetria.anotar(&decidido("agent-guard", "deny", 9), Instant::now());
        assert_eq!(telemetria.ultimo, None);
        assert!(es_borrador("tool-risk-local (borrador)") && !es_borrador("tool-risk-local"));
    }

    #[test]
    fn el_catalogo_de_backends_y_la_actividad_llegan_por_el_bus() {
        let mut telemetria = sorda("tool-risk-local");
        telemetria.anotar(&evento(EventKind::Backends { backends: vec!["kev".into(), "laya".into()] }), Instant::now());
        assert_eq!(telemetria.backends, ["kev", "laya"]);

        let ahora = Instant::now();
        let flujo = flujo();
        telemetria.anotar(&cargado(&flujo), ahora);
        telemetria.anotar(
            &evento(EventKind::NodeEnd {
                flow: "tool-risk-local".into(),
                node: "rm_root".into(),
                answer: json!(true),
                confidence: 1.0,
                port: "true".into(),
            }),
            ahora,
        );
        assert_eq!(telemetria.activity.edge_count("tool-risk-local", "rm_root", "true"), 1);
    }
}
