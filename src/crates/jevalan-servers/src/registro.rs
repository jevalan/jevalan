//! The `servers.yaml` registry: which servers exist on this machine and how they are started.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A local model server, as the registry declares it. The map key is its name,
/// which is also the backend's name in the daemon config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Servidor {
    /// The name: the key under which it appears. It does not travel in the file, it is filled in on reading.
    #[serde(skip)]
    pub nombre: String,
    /// Port where it listens. Mandatory: it is the only sign of life that works with Kev and with Laya.
    pub port: u16,
    /// A shell line, executed with `cmd /c`: this way the `serve.bat` files that already work stay valid.
    pub command: String,
    /// The process's working directory, and the base of `log` if that is relative.
    pub cwd: PathBuf,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// How long it is given to load before being written off. Kev takes one or two minutes.
    #[serde(default = "ready_por_defecto")]
    pub ready_ms: u64,
    /// Where its stdout and stderr go. Relative to `cwd`.
    pub log: PathBuf,
}

fn ready_por_defecto() -> u64 {
    120_000
}

/// The whole registry: the daemon config the screen edits, and the servers.
///
/// It is also the shape of the file: it is read and written with this very structure, so that a
/// field added here is not lost along the way in a parallel copy that nobody updates.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Registro {
    /// Path of the daemon config file that the screen opens; it is remembered between starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<PathBuf>,
    #[serde(default)]
    pub servers: BTreeMap<String, Servidor>,
}

/// Default registry path: `./servers.yaml`, next to where jevalan is run.
pub fn ruta_por_defecto() -> PathBuf {
    PathBuf::from("./servers.yaml")
}

/// The data directory where the `.pid` files live: the same one `daemon.json` already uses.
pub fn dir_pids() -> Option<PathBuf> {
    jevalan_daemon::discovery::data_dir().map(|d| d.join("servers"))
}

/// Reads the registry. A file that is not understood, or a server missing the indispensable,
/// is rejected **with its reason**: the screen must be able to say it.
///
/// On reading, `nombre` is filled in from the key, and `log` is resolved against `cwd` **only if `log` is
/// relative and `cwd` is absolute**. If `cwd` is also relative, both are left as they are: joining two
/// relative ones to write them back out would pile them up once per cycle (`kev` + `kev.log` would give
/// `kev/kev.log`, and on the next round `kev/kev/kev.log`), so the resolution is left to
/// whoever is going to use the path, who is the only one who knows from where.
pub fn leer(ruta: &Path) -> Result<Registro, String> {
    let texto = std::fs::read_to_string(ruta).map_err(|e| format!("no se puede leer {}: {e}", ruta.display()))?;
    let leido: Registro =
        serde_yaml::from_str(&texto).map_err(|e| format!("YAML inválido en {}: {e}", ruta.display()))?;
    let Registro { config, servers } = leido;
    let mut servidores = BTreeMap::new();
    for (nombre, mut s) in servers {
        if s.port == 0 {
            return Err(format!("el servidor «{nombre}» no declara un puerto válido"));
        }
        if s.command.trim().is_empty() {
            return Err(format!("el servidor «{nombre}» no declara command: no hay nada que arrancar"));
        }
        if s.cwd.as_os_str().is_empty() {
            return Err(format!("el servidor «{nombre}» no declara cwd"));
        }
        // The log is relative to `cwd`; it is resolved here so that nobody has to remember. With a relative
        // `cwd` it is not touched: resolving it would force piling it up on every read-and-write cycle.
        if s.log.is_relative() && s.cwd.is_absolute() {
            s.log = s.cwd.join(&s.log);
        }
        s.nombre = nombre.clone();
        servidores.insert(nombre, s);
    }
    Ok(Registro { config, servers: servidores })
}

/// A copy of the registry with the single truth of `nombre` restored: **the map key rules**.
///
/// It exists because `nombre` does not travel in the file (it is `#[serde(skip)]`), so a `Servidor`
/// built by hand with the name left blank, or with one that does not match its key, is a value that
/// contradicts itself. Returning it to the key leaves the registry consistent with itself, and incidentally
/// makes the comparisons the rest of the plan makes on a draft not say "it has changed"
/// because of a field the file never stored.
fn normalizar_nombres(reg: &Registro) -> Registro {
    let mut copia = reg.clone();
    for (nombre, s) in copia.servers.iter_mut() {
        s.nombre = nombre.clone();
    }
    copia
}

/// Writes the registry.
///
/// **It normalises the file**, and it is worth knowing before missing something:
///
/// - It does not preserve comments: the registry is generated by jevalan, not edited by hand.
/// - The order of the fields and of the servers is that of the structure and of the `BTreeMap`, not
///   the one the file had.
/// - `env` always comes out, even if it was absent, and `config` only if there is one (`None` is omitted, instead
///   of being written as `null`).
/// - A `log` that was resolved on reading is left written already resolved, so a second reading does
///   not touch it again.
/// - `nombre` does not appear in the file (it is `#[serde(skip)]`), but the copy that is serialised
///   carries it re-derived from the key, and the caller's registry is not touched.
pub fn escribir(ruta: &Path, reg: &Registro) -> Result<(), String> {
    let texto = serde_yaml::to_string(&normalizar_nombres(reg)).map_err(|e| format!("no se puede serializar: {e}"))?;
    if let Some(dir) = ruta.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("no se puede crear {}: {e}", dir.display()))?;
    }
    std::fs::write(ruta, texto).map_err(|e| format!("no se puede escribir {}: {e}", ruta.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EJEMPLO: &str = r#"# Cómo levantar los servidores de modelos locales en esta máquina.
# La clave de cada servidor es el nombre del backend en la config del daemon.
config: 'config/lab/config.local.yaml'   # el fichero que edita la pantalla; se recuerda aquí
servers:
  kev:
    port: 8009
    command: 'C:\models\kev\serve.bat 0.8b'
    cwd: 'C:\models\kev'
    env: { HF_HUB_DISABLE_SYMLINKS: "1" }
    ready_ms: 180000
    log: 'kev-serve.log'
  laya:
    port: 8021
    command: 'C:\models\laya\serve.bat'
    cwd: 'C:\models\laya'
    ready_ms: 120000
    log: 'laya-serve.log'
"#;

    /// Temporary directory that is deleted when dropped.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            // Each test needs its own directory: they run in parallel and share `temp_dir()`.
            static SIGUIENTE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = SIGUIENTE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("jevalan-servers-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn put(&self, nombre: &str, texto: &str) -> PathBuf {
            let ruta = self.0.join(nombre);
            std::fs::write(&ruta, texto).unwrap();
            ruta
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A plausible, complete server, of which each test tweaks what interests it. `nombre` is
    /// left as a hand construction would leave it: empty, which is what `escribir` has to re-derive.
    fn servidor(port: u16, cwd: &str, log: &str) -> Servidor {
        Servidor {
            nombre: String::new(),
            port,
            command: "serve.bat".to_string(),
            cwd: PathBuf::from(cwd),
            env: BTreeMap::new(),
            ready_ms: 1_000,
            log: PathBuf::from(log),
        }
    }

    #[test]
    fn el_registro_se_lee_con_su_config_y_sus_servidores() {
        let tmp = Tmp::new();
        let ruta = tmp.put("servers.yaml", EJEMPLO);
        let reg = leer(&ruta).unwrap();

        assert_eq!(reg.config.as_deref(), Some(Path::new("config/lab/config.local.yaml")));
        assert_eq!(reg.servers.len(), 2);
        let kev = &reg.servers["kev"];
        assert_eq!(kev.nombre, "kev");
        assert_eq!(kev.port, 8009);
        assert_eq!(kev.command, r"C:\models\kev\serve.bat 0.8b");
        assert_eq!(kev.cwd, Path::new(r"C:\models\kev"));
        assert_eq!(kev.env.get("HF_HUB_DISABLE_SYMLINKS").map(String::as_str), Some("1"));
        assert_eq!(kev.ready_ms, 180_000);
        // The `log` is relative to `cwd`, and is resolved on reading so that nobody has to remember.
        assert_eq!(kev.log, Path::new(r"C:\models\kev\kev-serve.log"));
    }

    #[test]
    fn un_registro_sin_config_deja_la_config_en_none() {
        let tmp = Tmp::new();
        let ruta = tmp.put("servers.yaml", "servers: {}\n");
        assert!(leer(&ruta).unwrap().config.is_none());
    }

    #[test]
    fn un_registro_imposible_se_rechaza_con_su_motivo() {
        let tmp = Tmp::new();
        // Without a port: it is the only sign of life that works with Kev and with Laya.
        let sin_puerto = tmp.put("a.yaml", "servers: { kev: { command: x, cwd: 'C:\\', ready_ms: 1, log: l } }\n");
        // Without the key, the rejection is serde's (`missing field`), and its text is none of our business:
        // here it is only asserted that it does not pass.
        assert!(leer(&sin_puerto).is_err());
        // Port zero: the key is there, and the rejection is now our own, in Spanish.
        let puerto_cero =
            tmp.put("b.yaml", "servers: { kev: { port: 0, command: x, cwd: 'C:\\', ready_ms: 1, log: l } }\n");
        assert!(leer(&puerto_cero).unwrap_err().contains("puerto"), "{}", leer(&puerto_cero).unwrap_err());
        // Without `command` there is nothing to start; with the key present but blank, neither. Both
        // forms are tested because only the second reaches our own check: the first
        // dies earlier, in serde.
        let sin_orden = tmp.put("c.yaml", "servers: { kev: { port: 8009, cwd: 'C:\\', ready_ms: 1, log: l } }\n");
        assert!(leer(&sin_orden).is_err());
        let orden_en_blanco =
            tmp.put("e.yaml", "servers: { kev: { port: 8009, command: '  ', cwd: 'C:\\', ready_ms: 1, log: l } }\n");
        assert!(leer(&orden_en_blanco).unwrap_err().contains("command"), "{}", leer(&orden_en_blanco).unwrap_err());
        // And a `cwd` that is present but empty, which is the other rejection that no other case reaches.
        let sin_directorio =
            tmp.put("f.yaml", "servers: { kev: { port: 8009, command: x, cwd: '', ready_ms: 1, log: l } }\n");
        assert!(leer(&sin_directorio).unwrap_err().contains("cwd"), "{}", leer(&sin_directorio).unwrap_err());
        // And a file that is not YAML.
        let basura = tmp.put("d.yaml", "esto: [no es un registro");
        assert!(leer(&basura).is_err());
    }

    #[test]
    fn escribir_y_volver_a_leer_devuelve_lo_mismo() {
        let tmp = Tmp::new();
        let ruta = tmp.put("servers.yaml", EJEMPLO);
        let reg = leer(&ruta).unwrap();
        let otra = tmp.0.join("copia.yaml");
        escribir(&otra, &reg).unwrap();
        assert_eq!(leer(&otra).unwrap().servers, reg.servers);
        assert_eq!(leer(&otra).unwrap().config, reg.config);
    }

    #[test]
    fn el_ciclo_de_escritura_y_lectura_no_apila_un_log_relativo() {
        let tmp = Tmp::new();
        // A relative `cwd` leaves `log` unresolved, so that nobody piles it up when writing it back.
        let mut reg = Registro::default();
        reg.servers.insert("kev".to_string(), servidor(8009, ".", "kev.log"));
        let ruta = tmp.0.join("ida.yaml");
        escribir(&ruta, &reg).unwrap();
        let vuelta = leer(&ruta).unwrap();
        assert_eq!(vuelta.servers["kev"].log, Path::new("kev.log"), "el primer ciclo ya lo torció");
        // And the second cycle must give the same: it is the idempotence asked of the registry.
        let otra = tmp.0.join("vuelta.yaml");
        escribir(&otra, &vuelta).unwrap();
        assert_eq!(leer(&otra).unwrap().servers, vuelta.servers);
    }

    #[test]
    fn al_normalizar_manda_la_clave_del_mapa_y_no_el_nombre_del_servidor() {
        let mut reg = Registro::default();
        let mut a_mano = servidor(8009, ".", "kev.log");
        a_mano.nombre = "despistado".to_string();
        reg.servers.insert("kev".to_string(), a_mano);
        let copia = normalizar_nombres(&reg);
        assert_eq!(copia.servers["kev"].nombre, "kev");
        // And the caller's registry is not touched: a copy is worked on.
        assert_eq!(reg.servers["kev"].nombre, "despistado");
    }

    #[test]
    fn el_fichero_nunca_lleva_el_nombre_de_un_servidor() {
        let tmp = Tmp::new();
        let mut reg = Registro::default();
        reg.servers.insert("kev".to_string(), servidor(8009, ".", "kev.log"));
        let ruta = tmp.0.join("kev.yaml");
        escribir(&ruta, &reg).unwrap();
        // `nombre` is `#[serde(skip)]`, so it is not in the text: the map key is the only
        // source, and that is why the re-derivation cannot be observed from the file (it is checked
        // on the copy, in the test above).
        let texto = std::fs::read_to_string(&ruta).unwrap();
        assert!(!texto.contains("nombre"), "el nombre se coló en el fichero:\n{texto}");
        // On coming back from disk, the name comes from the key, which is what the cycle is asked for.
        assert_eq!(leer(&ruta).unwrap().servers["kev"].nombre, "kev");
    }

    #[test]
    fn escribir_dos_veces_seguidas_da_el_mismo_texto() {
        let tmp = Tmp::new();
        let ruta = tmp.put("servers.yaml", EJEMPLO);
        let reg = leer(&ruta).unwrap();
        // The first write normalises; the second must not change a single character.
        let primera = tmp.0.join("primera.yaml");
        escribir(&primera, &reg).unwrap();
        let segunda = tmp.0.join("segunda.yaml");
        escribir(&segunda, &leer(&primera).unwrap()).unwrap();
        assert_eq!(std::fs::read(&primera).unwrap(), std::fs::read(&segunda).unwrap());
    }
}
