//! Visual flow editor: the document, the patcher, the graph, the inspector and its screen.

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
