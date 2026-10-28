//! Telemetría en vivo: el bus del daemon, para ver qué está decidiendo el flujo publicado.

use jevalan_events::{Event, EventKind};
use jevalan_flow::Flow;
use jevalan_tui::flow_activity::FlowActivity;
use jevalan_tui::live;
use jevalan_tui::screen::{Feed, SourceStatus};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

/// El sufijo con el que un borrador firma sus eventos (sección 5 de la spec).
pub const BORRADOR: &str = " (borrador)";

/// ¿El evento lo firma un borrador y no el flujo publicado?
pub fn es_borrador(flow: &str) -> bool {
    flow.ends_with(BORRADOR)
}

/// Estado de la conexión con el bus, tal como lo cuenta la banda inferior.
#[derive(Debug, Clone, PartialEq)]
pub enum Estado {
    Conectando,
    Conectado,
    Caido(String),
    /// Sin bus: se pidió `--no-events`, o no hay daemon al que escuchar.
    Apagado,
}

/// Lo que el editor sabe del daemon por el bus de eventos.
pub struct Telemetria {
    rx: Option<Receiver<Feed>>,
    /// Nombre del flujo que se edita: solo sus decisiones van a la banda.
    nombre: String,
    /// Flujos vigentes por `FLOW_LOADED`: `FlowActivity` los necesita para reconocer sus nodos.
    flows: BTreeMap<String, Arc<Flow>>,
    /// Recorridos de las aristas y paquetes en vuelo del flujo publicado.
    pub activity: FlowActivity,
    pub estado: Estado,
    /// Resumen del último evento del flujo publicado, para la banda inferior.
    pub ultimo: Option<String>,
    /// Catálogo de backends que el daemon declaró. Es el que usa la validación local cuando el daemon
    /// no responde: el evento `BACKENDS` solo trae sus nombres.
    pub backends: Vec<String>,
}

impl Telemetria {
    /// Se conecta al bus del daemon, que reintenta con paciencia, como el dashboard.
    pub fn conectar(url: &str, token: Option<&str>, nombre: &str) -> Self {
        let mut telemetria = Self::apagada(nombre);
        telemetria.rx = Some(live::spawn(url.to_string(), token.map(str::to_string)));
        telemetria.estado = Estado::Conectando;
        telemetria
    }

    /// Sin bus: el editor funciona igual, solo que sin telemetría.
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

    /// Consume lo que haya llegado por el bus; devuelve `true` si algo cambió.
    pub fn latir(&mut self, ahora: Instant) -> bool {
        let mut cambio = false;
        // El receptor sale de `self` mientras se drena: anotar un evento necesita `&mut self`.
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
        // Los paquetes de las aristas caducan solos: hay que retirarlos para que la pantalla siga.
        if self.activity.has_packets() {
            self.activity.tick(ahora);
            cambio = true;
        }
        cambio
    }

    /// Anota un evento: el catálogo de backends, los flujos vigentes y lo último que decidió el flujo
    /// publicado. Un borrador no cuenta como decisión publicada, y `FlowActivity` lo ignora solo,
    /// porque su nombre no está entre los flujos vigentes.
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

    /// Una telemetría sin bus, ya conectada, para poder anotar eventos a mano.
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
        // Y los de otro flujo tampoco: la banda habla del que se edita.
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
