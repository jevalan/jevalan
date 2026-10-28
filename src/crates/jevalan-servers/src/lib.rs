//! Servidores de modelos locales: qué existe, cómo se arranca y con qué config corre el daemon.
//! Lógica pura y sin UI: la TUI y el subcomando `jevalan servers` la usan por igual.

pub mod borrador;
pub mod proceso;
pub mod registro;

pub use borrador::{Cambio, parchear, validar};
pub use proceso::{
    Estado, arrancar, borrar_pid, cola_log, escribir_pid, esperar, estado, parar, pid_de, pid_vivo, proceso_de_puerto,
    puerto_contesta,
};
pub use registro::{Registro, Servidor, dir_pids, escribir, leer, ruta_por_defecto};
