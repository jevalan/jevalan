//! El registro `servers.yaml`: qué servidores existen en esta máquina y cómo se arrancan.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Un servidor de modelos local, tal como lo declara el registro. La clave del mapa es su nombre,
/// que es también el nombre del backend en la config del daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Servidor {
    /// El nombre: la clave bajo la que aparece. No viaja en el fichero, se rellena al leerlo.
    #[serde(skip)]
    pub nombre: String,
    /// Puerto donde escucha. Obligatorio: es la única señal de vida que sirve con Kev y con Laya.
    pub port: u16,
    /// Una línea de shell, ejecutada con `cmd /c`: así siguen valiendo los `serve.bat` que ya funcionan.
    pub command: String,
    /// Directorio de trabajo del proceso, y la base de `log` si es relativo.
    pub cwd: PathBuf,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Cuánto se le concede para cargar antes de darlo por perdido. Kev tarda uno o dos minutos.
    #[serde(default = "ready_por_defecto")]
    pub ready_ms: u64,
    /// Dónde van su stdout y su stderr. Relativo a `cwd`.
    pub log: PathBuf,
}

fn ready_por_defecto() -> u64 {
    120_000
}

/// El registro entero: la config del daemon que edita la pantalla, y los servidores.
///
/// Es a la vez la forma del fichero: se lee y se escribe con esta misma estructura, para que un
/// campo que se añada aquí no se pierda por el camino en una copia paralela que nadie actualice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Registro {
    /// Ruta del fichero de config del daemon que la pantalla abre; se recuerda entre arranques.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<PathBuf>,
    #[serde(default)]
    pub servers: BTreeMap<String, Servidor>,
}

/// Ruta del registro por defecto: `./servers.yaml`, junto a donde se ejecute jevalan.
pub fn ruta_por_defecto() -> PathBuf {
    PathBuf::from("./servers.yaml")
}

/// El directorio de datos donde viven los `.pid`: el mismo que ya usa `daemon.json`.
pub fn dir_pids() -> Option<PathBuf> {
    jevalan_daemon::discovery::data_dir().map(|d| d.join("servers"))
}

/// Lee el registro. Un fichero que no se entiende, o un servidor al que le falta lo indispensable,
/// se rechaza **con su motivo**: la pantalla ha de poder decirlo.
///
/// Al leer se rellena `nombre` desde la clave, y se resuelve `log` contra `cwd` **solo si `log` es
/// relativo y `cwd` es absoluto**. Si `cwd` también es relativo, se dejan los dos tal cual: unir dos
/// relativos para volver a escribirlos los apilaría una vez por ciclo (`kev` + `kev.log` daría
/// `kev/kev.log`, y a la siguiente vuelta `kev/kev/kev.log`), así que la resolución se le deja a
/// quien vaya a usar la ruta, que es el único que sabe desde dónde.
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
        // El log es relativo a `cwd`; se resuelve aquí para que nadie tenga que acordarse. Con `cwd`
        // relativo no se toca: resolverlo obligaría a apilarlo en cada ciclo de lectura y escritura.
        if s.log.is_relative() && s.cwd.is_absolute() {
            s.log = s.cwd.join(&s.log);
        }
        s.nombre = nombre.clone();
        servidores.insert(nombre, s);
    }
    Ok(Registro { config, servers: servidores })
}

/// Una copia del registro con la única verdad de `nombre` restablecida: **la clave del mapa manda**.
///
/// Existe porque `nombre` no viaja en el fichero —va `#[serde(skip)]`—, así que un `Servidor`
/// construido a mano con el nombre sin rellenar, o con uno que no case con su clave, es un valor que
/// se desmiente a sí mismo. Devolverlo a la clave deja el registro coherente consigo mismo, y de paso
/// hace que las comparaciones que el resto del plan haga sobre un borrador no digan «ha cambiado»
/// por un campo que el fichero nunca guardó.
fn normalizar_nombres(reg: &Registro) -> Registro {
    let mut copia = reg.clone();
    for (nombre, s) in copia.servers.iter_mut() {
        s.nombre = nombre.clone();
    }
    copia
}

/// Escribe el registro.
///
/// **Normaliza el fichero**, y conviene saberlo antes de echar de menos algo:
///
/// - No conserva comentarios: el registro lo genera jevalan, no se edita a mano.
/// - El orden de los campos y el de los servidores es el de la estructura y el del `BTreeMap`, no
///   el que tenía el fichero.
/// - `env` sale siempre, aunque estuviera ausente, y `config` solo si lo hay —`None` se omite, en
///   vez de escribirse como `null`—.
/// - Un `log` que se resolvió al leer queda escrito ya resuelto, con lo que una segunda lectura ya
///   no lo vuelve a tocar.
/// - `nombre` no aparece en el fichero —va `#[serde(skip)]`—, pero la copia que se serializa lo
///   lleva re-derivado de la clave, y el registro del llamante no se toca.
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

    /// Directorio temporal que se borra al soltarse.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            // Cada prueba necesita su directorio: corren en paralelo y comparten `temp_dir()`.
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

    /// Un servidor verosímil y completo, del que cada prueba retoca lo que le interesa. `nombre` se
    /// deja como lo dejaría una construcción a mano: vacío, que es lo que `escribir` ha de re-derivar.
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
        // El `log` es relativo a `cwd`, y se resuelve al leerlo para que nadie tenga que acordarse.
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
        // Sin puerto: es la única señal de vida que funciona con Kev y con Laya.
        let sin_puerto = tmp.put("a.yaml", "servers: { kev: { command: x, cwd: 'C:\\', ready_ms: 1, log: l } }\n");
        // Sin la clave, el rechazo es de serde (`missing field`), y su texto no es asunto nuestro:
        // aquí solo se afirma que no pasa.
        assert!(leer(&sin_puerto).is_err());
        // Puerto cero: la clave está, y el rechazo ya es el nuestro, en castellano.
        let puerto_cero =
            tmp.put("b.yaml", "servers: { kev: { port: 0, command: x, cwd: 'C:\\', ready_ms: 1, log: l } }\n");
        assert!(leer(&puerto_cero).unwrap_err().contains("puerto"), "{}", leer(&puerto_cero).unwrap_err());
        // Sin `command` no hay nada que arrancar; con la clave presente pero en blanco, tampoco. Las
        // dos formas se prueban porque solo la segunda llega a la comprobación propia: la primera
        // muere antes, en serde.
        let sin_orden = tmp.put("c.yaml", "servers: { kev: { port: 8009, cwd: 'C:\\', ready_ms: 1, log: l } }\n");
        assert!(leer(&sin_orden).is_err());
        let orden_en_blanco =
            tmp.put("e.yaml", "servers: { kev: { port: 8009, command: '  ', cwd: 'C:\\', ready_ms: 1, log: l } }\n");
        assert!(leer(&orden_en_blanco).unwrap_err().contains("command"), "{}", leer(&orden_en_blanco).unwrap_err());
        // Y un `cwd` presente pero vacío, que es el otro rechazo que ningún otro caso alcanza.
        let sin_directorio =
            tmp.put("f.yaml", "servers: { kev: { port: 8009, command: x, cwd: '', ready_ms: 1, log: l } }\n");
        assert!(leer(&sin_directorio).unwrap_err().contains("cwd"), "{}", leer(&sin_directorio).unwrap_err());
        // Y un fichero que no es YAML.
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
        // `cwd` relativo deja `log` sin resolver, para que nadie lo apile al volver a escribirlo.
        let mut reg = Registro::default();
        reg.servers.insert("kev".to_string(), servidor(8009, ".", "kev.log"));
        let ruta = tmp.0.join("ida.yaml");
        escribir(&ruta, &reg).unwrap();
        let vuelta = leer(&ruta).unwrap();
        assert_eq!(vuelta.servers["kev"].log, Path::new("kev.log"), "el primer ciclo ya lo torció");
        // Y el segundo ciclo ha de dar lo mismo: es la idempotencia que se le pide al registro.
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
        // Y el registro del llamante no se toca: se trabaja sobre una copia.
        assert_eq!(reg.servers["kev"].nombre, "despistado");
    }

    #[test]
    fn el_fichero_nunca_lleva_el_nombre_de_un_servidor() {
        let tmp = Tmp::new();
        let mut reg = Registro::default();
        reg.servers.insert("kev".to_string(), servidor(8009, ".", "kev.log"));
        let ruta = tmp.0.join("kev.yaml");
        escribir(&ruta, &reg).unwrap();
        // `nombre` va `#[serde(skip)]`, así que no está en el texto: la clave del mapa es la única
        // fuente, y por eso la re-derivación no puede observarse desde el fichero —se comprueba
        // sobre la copia, en la prueba de arriba—.
        let texto = std::fs::read_to_string(&ruta).unwrap();
        assert!(!texto.contains("nombre"), "el nombre se coló en el fichero:\n{texto}");
        // Al volver del disco, el nombre sale de la clave, que es lo que se le pide al ciclo.
        assert_eq!(leer(&ruta).unwrap().servers["kev"].nombre, "kev");
    }

    #[test]
    fn escribir_dos_veces_seguidas_da_el_mismo_texto() {
        let tmp = Tmp::new();
        let ruta = tmp.put("servers.yaml", EJEMPLO);
        let reg = leer(&ruta).unwrap();
        // La primera escritura normaliza; la segunda ya no ha de cambiar ni un carácter.
        let primera = tmp.0.join("primera.yaml");
        escribir(&primera, &reg).unwrap();
        let segunda = tmp.0.join("segunda.yaml");
        escribir(&segunda, &leer(&primera).unwrap()).unwrap();
        assert_eq!(std::fs::read(&primera).unwrap(), std::fs::read(&segunda).unwrap());
    }
}
