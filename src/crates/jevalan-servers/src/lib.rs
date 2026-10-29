//! Local model servers: what exists, how it is started and with what config the daemon runs.
//! Pure logic and no UI: the TUI and the `jevalan servers` subcommand use it equally.

pub mod borrador;
pub mod proceso;
pub mod registro;

pub use borrador::{Cambio, parchear, validar};
pub use proceso::{
    Estado, arrancar, borrar_pid, cola_log, escribir_pid, esperar, estado, parar, pid_de, pid_vivo, proceso_de_puerto,
    puerto_contesta,
};
pub use registro::{Registro, Servidor, dir_pids, escribir, leer, ruta_por_defecto};
