//! Editor visual de flujos: el documento, el parcheador, el grafo, el inspector y su pantalla.

pub mod app;
pub mod daemon;
pub mod documento;
pub mod edicion;
pub mod grafo;
pub mod inspector;
pub mod patch;
pub mod run;
pub mod telemetria;
pub mod vista;

pub use run::run;
