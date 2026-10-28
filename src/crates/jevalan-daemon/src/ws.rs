//! `GET /events`: un mensaje JSON de texto por evento del bus.

use crate::AppState;
use crate::bus::lagged_event;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use jevalan_events::v2::{Event, EventKind, new_event};
use tokio::sync::broadcast::{Receiver, error::RecvError};

pub async fn events(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    // Suscribirse antes de aceptar evita perder lo ocurrido durante el handshake.
    let rx = state.bus.subscribe();
    upgrade.on_upgrade(move |socket| pump(socket, state, rx))
}

async fn pump(mut socket: WebSocket, state: AppState, mut rx: Receiver<Event>) {
    if !send_initial(&mut socket, &state).await {
        return;
    }
    loop {
        let event = tokio::select! {
            received = rx.recv() => match received {
                Ok(event) => event,
                Err(RecvError::Lagged(n)) => lagged_event(n),
                Err(RecvError::Closed) => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => continue,
            },
        };
        if !send(&mut socket, &event).await {
            return;
        }
    }
}

/// Anuncia los backends configurados (solo nombres) y reproduce los `FLOW_LOADED` y `CAPABILITY_LOADED` vigentes;
/// devuelve `false` si el cliente ya no está.
async fn send_initial(socket: &mut WebSocket, state: &AppState) -> bool {
    let names = state.cfg.backends.keys().cloned().collect();
    let backends = new_event(&format!("backends:{}", state.boot), 1, "daemon", EventKind::Backends { backends: names });
    if !send(socket, &backends).await {
        return false;
    }
    for event in state.flows.loaded_events().into_iter().chain(state.capabilities.loaded_events()) {
        if !send(socket, &event).await {
            return false;
        }
    }
    true
}

pub async fn send(socket: &mut WebSocket, event: &Event) -> bool {
    match serde_json::to_string(event) {
        Ok(json) => socket.send(Message::Text(json.into())).await.is_ok(),
        Err(_) => true,
    }
}
