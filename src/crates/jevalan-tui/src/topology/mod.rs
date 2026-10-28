//! Topología animada: modelo, reducer y layout. Lógica pura, sin ratatui.

mod layout;
mod model;
mod reducer;

pub use layout::{CHIP_H, LED_PANEL_W, Placed, SWITCH_H, layout, sparkline_row};
pub use model::{
    BackendState, CLIENT_GONE, CLIENT_IDLE, CapabilityInfo, ClientState, Cue, DOWN_AFTER, MAX_OPEN_TRACES,
    MAX_VISIBLE_CLIENTS, NodeId, PACKET_TTL, PASSTHROUGH, Packet, PacketStatus, Topology, progress,
};
