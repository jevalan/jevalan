//! `jev-hub/0.1` decision hub: envelope, capability registry and classification. Pure logic.

pub mod capability;
pub mod classify;
pub mod envelope;
pub mod lab;
pub mod program;
pub mod shadow;

pub const PROTOCOL: &str = "jev-hub/0.1";
