//! El borrador de la config del daemon: se edita el **texto**, no se reserializa.
//!
//! `serde_yaml` no conserva comentarios y `config/lab/config.local.yaml` está comentado, así que la pantalla
//! parchea el texto por líneas. La validación, en cambio, es la del daemon: `config::load`.

use jevalan_daemon::config::{Config, load};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Cuántos temporales ha pedido este proceso. Es lo que hace **único** el directorio de cada
/// validación, y no es un adorno: la pantalla valida mientras se teclea, y dos validaciones a la vez
/// —dos hilos de la TUI, o dos pruebas del propio crate, que corren en paralelo— compartirían
/// `config.yaml` y una borraría el directorio de la otra a mitad de lectura. Eso se manifiesta como
/// un «no se puede leer» sobre un texto que sí era bueno.
static TEMPORALES: AtomicU64 = AtomicU64::new(0);

// El temporal que acaba de usar **esta hebra**, para que una prueba pueda afirmar sobre el suyo y no
// sobre el de al lado. El contador de arriba es del proceso entero, así que con él solo no se puede
// distinguir «mi directorio» de «el de otra prueba que está corriendo a la vez»: esto sí.
//
// Va bajo `cfg(test)` porque en producción no hay nada que mirar aquí, y es una pila —y no un solo
// hueco— para que anidar validaciones no se pise a sí mismo.
#[cfg(test)]
thread_local! {
    static ULTIMO_TEMPORAL: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// El nombre del directorio temporal de la siguiente validación: el pid, para no pisar a otro
/// proceso, y un contador, para no pisarse a sí mismo.
fn nombre_temporal() -> String {
    format!("jevalan-borrador-{}-{}", std::process::id(), TEMPORALES.fetch_add(1, Ordering::Relaxed))
}

/// Un valor a reescribir dentro del bloque `backends:`. Solo valores: nunca se añaden ni se quitan
/// backends desde la pantalla, porque eso cambia el conjunto y no un valor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cambio {
    pub backend: String,
    pub clave: String,
    pub valor: String,
}

/// Un directorio temporal que se retira al salir de su ámbito, también si algo falla por el camino.
///
/// Sin él, un `load` que devolviera pronto dejaría el directorio en `%TEMP%` a cada validación, y
/// volver a la función para añadir el borrado en cada rama de salida es justo lo que se olvida. Que
/// el borrado sea «lo mejor que se pueda» —y no un error que propagar— es a propósito: no hay nada
/// que el llamante pueda hacer con un `%TEMP%` que no se deja limpiar, y fallar la validación por eso
/// sería convertir un problema de limpieza en uno de corrección.
struct Temporal(PathBuf);

impl Drop for Temporal {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Valida un texto de config con **las reglas del daemon**, sin tocar el fichero del usuario.
///
/// `config::load` toma una ruta, así que el texto se deja en un fichero temporal y se lee de ahí: es
/// la única forma de que la autoridad sea el propio daemon y no una copia de sus reglas. Lo que no se
/// toca es el fichero de config **del usuario**: no se abre, ni se escribe, ni se mueve.
///
/// El temporal es **uno por llamada** —el nombre lleva el pid y un contador— y se retira antes de
/// volver, con la guarda que también cubre la vuelta temprana del `?`, de modo que validar el borrador
/// mientras se teclea no deja rastro en disco ni estorba a la validación de al lado.
pub fn validar(texto: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Config, String> {
    let dir = std::env::temp_dir().join(nombre_temporal());
    std::fs::create_dir_all(&dir).map_err(|e| format!("no se puede crear {}: {e}", dir.display()))?;
    let guarda = Temporal(dir);
    #[cfg(test)]
    ULTIMO_TEMPORAL.with(|pila| pila.borrow_mut().push(guarda.0.clone()));
    let ruta = guarda.0.join("config.yaml");
    std::fs::write(&ruta, texto).map_err(|e| format!("no se puede escribir {}: {e}", ruta.display()))?;
    load(Some(&ruta), env)
}

/// Reescribe los valores pedidos dentro del bloque `backends:`, conservando comentarios y orden.
///
/// Se reconocen las entradas **en línea** (`kev: { url: …, timeout_ms: … }`), que es como están hoy
/// en `config/lab/config.local.yaml`. Todo lo demás se **niega** con un mensaje explícito, en vez de
/// reescribir el fichero perdiendo algo: un mapa en bloque, un **ancla en línea** (`kev: &base { … }`,
/// cuyo ancla desaparecería dejando a sus alias sin destino), una **clave de fusión** (`<<:`) o
/// cualquier resto que no se entienda. La regla es simple y conservadora: **entre `backend:` y su `{`
/// solo puede haber espacios**, y tras el `}` de cierre, o nada, o espacios y un comentario.
///
/// Cada cambio se aplica sobre el resultado del anterior, así que dos cambios sobre el mismo backend
/// —la url y el plazo, por ejemplo— salen los dos en la misma línea.
///
/// **Los finales de línea.** El fichero entero se reescribe con **un solo** estilo: CRLF si el
/// original tenía alguno, y LF si no tenía ninguno. Un fichero **mezclado** —una línea CRLF entre
/// líneas LF, que es lo que deja un editor que se equivoca una vez— sale por tanto unificado al estilo
/// que ya predominaba, no conservado línea a línea. Se elige así porque todas las líneas se vuelven a
/// unir con el mismo separador de todos modos, y porque lo que de verdad importa es que el fichero no
/// quede con dos estilos a la vez: eso es lo que hace que el control de versiones lo enseñe entero como
/// cambiado. Está probado en las dos direcciones (CRLF se queda CRLF, y el mezclado se unifica).
pub fn parchear(texto: &str, cambios: &[Cambio]) -> Result<String, String> {
    let mut lineas: Vec<String> = texto.lines().map(str::to_string).collect();
    let (inicio, fin) = bloque_backends(&lineas)?;
    for cambio in cambios {
        let mut hecho = false;
        for linea in lineas.iter_mut().take(fin).skip(inicio) {
            if let Some(nueva) = reescribir(linea, cambio)? {
                *linea = nueva;
                hecho = true;
                break;
            }
        }
        if !hecho {
            return Err(format!(
                "no se reconoce la entrada del backend «{}»: ha de estar en línea, como `{}: {{ url: …, {}: … }}`",
                cambio.backend, cambio.backend, cambio.clave
            ));
        }
    }
    // Se conserva el final de línea del fichero original: el dominante, si los hubiera mezclados.
    // Como todas las líneas se vuelven a unir con **el mismo** separador, un fichero mezclado sale ya
    // unificado al estilo dominante, sin necesidad de ninguna pasada de normalización detrás: `lines()`
    // se lleva los `\r` de cada línea consigo, y el `join` pone el suyo.
    let cr = if texto.contains("\r\n") { "\r\n" } else { "\n" };
    let mut salida = lineas.join(cr);
    if texto.ends_with('\n') {
        salida.push_str(cr);
    }
    Ok(salida)
}

/// Los límites del bloque `backends:`: desde su línea hasta la primera que no esté indentada más.
fn bloque_backends(lineas: &[String]) -> Result<(usize, usize), String> {
    let Some(cabeza) = lineas.iter().position(|l| l.trim_end() == "backends:" || l.starts_with("backends: {")) else {
        return Err("la config no declara un bloque `backends:`".into());
    };
    if lineas[cabeza].trim() != "backends:" {
        return Err("el bloque `backends:` ha de ser un mapa en varias líneas para poder parchearlo".into());
    }
    let fin = lineas[cabeza + 1..]
        .iter()
        .position(|l| !l.trim().is_empty() && !l.starts_with(' ') && !l.starts_with('\t'))
        .map_or(lineas.len(), |i| cabeza + 1 + i);
    Ok((cabeza + 1, fin))
}

/// Si `linea` declara `cambio.backend` en línea, devuelve la línea con su `clave` reescrita.
///
/// Los tres «no» que devuelve son distintos a propósito: `Ok(None)` es «esta línea no es de ese
/// backend, sigue mirando»; `Err` —entrada en bloque, ancla, clave de fusión, cola que no se entiende,
/// o clave que el backend no declara— es «esta sí es, y no se puede parchear»; y `Ok(Some(…))` es el
/// cambio hecho.
///
/// Lo que se exige, y por qué:
///
/// - **Entre `backend:` y su `{` no puede haber nada más que espacios.** Un ancla (`kev: &base { … }`)
///   vive justo ahí, y al reconstruir la línea se perdería: sus alias (`laya: *base`) quedarían
///   apuntando a un ancla que ya no existe. Delante del mapa, una clave de fusión (`kev: <<: { … }`)
///   se niega por el mismo motivo. **Dentro** del mapa, en cambio, un `<<:` u otra clave se conserva
///   tal cual: es un valor más, se copia sin tocarlo, y negarse por él sería inventarse un problema.
/// - **El `}` que cierra el mapa es el primero que hay tras el `{`, y no cuenta el que esté dentro de
///   comillas**, ni el de un comentario de cola —y este proyecto comenta con ejemplos de mapas en
///   línea—: con la última llave de la línea, el comentario desaparecería, y como el resultado seguiría
///   validando se escribiría en disco sin que nadie se enterase. Con la primera de fuera de comillas,
///   el comentario y los valores con llaves quedan donde estaban.
/// - **Tras el `}` de cierre, o nada, o espacios y un comentario.** Es lo que cierra la puerta a
///   cualquier resto que no se entienda.
fn reescribir(linea: &str, cambio: &Cambio) -> Result<Option<String>, String> {
    let recortada = linea.trim_start();
    let Some(resto) = recortada.strip_prefix(&format!("{}:", cambio.backend)) else {
        return Ok(None);
    };
    // Ha de ser una entrada con su mapa en línea; si no hay llaves, no hay mapa que reescribir: puede
    // ser un mapa en bloque, un alias (`laya: *base`) o algo que no se reconoce. Se niega sin decir
    // cuál —el texto no basta para distinguirlos sin un parser de YAML— y se enseña lo que hay.
    let (Some(abre), Some(cierra)) = llaves_del_mapa(resto) else {
        return Err(format!(
            "la entrada del backend «{}» no declara su mapa en línea (¿un bloque, un alias?): «{}» no se parchea, para no perder lo que hubiera",
            cambio.backend,
            resto.trim()
        ));
    };
    let delante = resto[..abre].trim();
    if !delante.is_empty() {
        // Lo que va delante del mapa es un ancla (`&base`) o una clave de fusión (`<<`); en los dos
        // casos, reconstruir la línea lo borraría: el ancla dejaría huérfanos a sus alias, y la fusión
        // perdería lo que aporta. Se nombra el caso para que el mensaje sirva de algo.
        let caso = if delante.starts_with('&') { "un ancla" } else { "una clave de fusión" };
        return Err(format!(
            "la entrada del backend «{}» lleva {caso} («{delante}») delante de su mapa: no se parchea para no dejarlo huérfano",
            cambio.backend
        ));
    }
    let cola = &resto[cierra + 1..];
    // La cola ha de ser nada, o espacios y un comentario. Si no, es algo que no se entiende.
    let cola_limpia = cola.trim_start();
    if !(cola_limpia.is_empty() || cola_limpia.starts_with('#')) {
        return Err(format!(
            "tras el mapa del backend «{}» hay algo que no se entiende («{}»): no se parchea para no perderlo",
            cambio.backend, cola_limpia
        ));
    }
    let dentro = &resto[abre + 1..cierra];
    // Si dentro del mapa hay un `{` fuera de comillas, lo que se ha tomado por cierre no lo es: sería
    // una entrada anidada, que este parcheo por líneas no sabe reescribir. Se niega en vez de escribir
    // una línea a medias.
    if dentro.contains('{') {
        return Err(format!(
            "el mapa del backend «{}» lleva otro mapa dentro: no se parchea, que este borrador solo reconoce valores sueltos",
            cambio.backend
        ));
    }
    let mut partes: Vec<String> = Vec::new();
    let mut encontrado = false;
    for trozo in dentro.split(',') {
        let trozo = trozo.trim();
        match trozo.split_once(':') {
            Some((clave, _valor)) if clave.trim() == cambio.clave => {
                partes.push(format!("{}: {}", cambio.clave, cambio.valor));
                encontrado = true;
            }
            _ => partes.push(trozo.to_string()),
        }
    }
    if !encontrado {
        return Err(format!("el backend «{}» no declara la clave «{}»", cambio.backend, cambio.clave));
    }
    let sangria = &linea[..linea.len() - recortada.len()];
    // El comentario que hubiera tras la llave se conserva tal cual.
    Ok(Some(format!("{sangria}{}: {{ {} }}{cola}", cambio.backend, partes.join(", "))))
}

/// Las llaves del mapa en línea de `resto`: la **primera** `{` y la **primera** `}` que la cierra.
///
/// La de cierre no puede ser la última de la línea: una llave dentro del comentario de cola —y este
/// proyecto comenta con ejemplos de mapas en línea— haría que «la última» se llevara el comentario por
/// delante. Con la primera que cierra el mapa, el comentario queda donde estaba.
///
/// No cuentan ni la llave que esté **dentro de unas comillas** —`{ url: '…/a}b' }` es un valor con una
/// llave, no el final del mapa— ni nada de lo que haya **tras un `#`**, que ya es comentario: un
/// comentario que diga «cierra con }» no cierra nada, y si la única llave estuviera ahí el mapa se
/// queda sin cierre y `parchear` se niega, que es lo correcto. Se sigue el entrecomillado a mano, que
/// es lo que basta para un mapa en una línea.
fn llaves_del_mapa(resto: &str) -> (Option<usize>, Option<usize>) {
    let Some(abre) = resto.find('{') else {
        return (None, None);
    };
    let mut comilla: Option<char> = None;
    for (desplazamiento, c) in resto[abre + 1..].char_indices() {
        match comilla {
            Some(abierta) => {
                if c == abierta {
                    comilla = None;
                }
            }
            None => match c {
                '\'' | '"' => comilla = Some(c),
                '#' => return (Some(abre), None),
                '}' => return (Some(abre), Some(abre + 1 + desplazamiento)),
                _ => {}
            },
        }
    }
    (Some(abre), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un trozo de `config/lab/config.local.yaml`, con sus comentarios y sus mapas en línea.
    const CONFIG: &str = r#"# Configuración local de jevalan: Kev y Laya.
server:
  addr: 127.0.0.1:8080
backends:
  kev:  { url: 'http://127.0.0.1:8009', timeout_ms: 120000 }   # el clasificador de casa
  laya: { url: 'http://127.0.0.1:8021', timeout_ms: 120000 }
  # un comentario suelto que ha de sobrevivir
  ollama: { url: 'http://localhost:11434', timeout_ms: 5000 }
default_backend: kev
flows_dir: ./config/flows
"#;

    fn env_de(_: &str) -> Option<String> {
        None
    }

    /// Un backend llamado como la clave de fuera del bloque, para comprobar dos cosas de una vez: que
    /// el parcheo **solo** mira dentro del bloque `backends:` —si mirara en todo el fichero, tocaría el
    /// `server:` de arriba, que es lo que el rango impide— y que una línea de fuera que empiece por el
    /// nombre de un backend no se confunde con su entrada.
    fn con_senuelo() -> String {
        let mut lineas: Vec<String> = CONFIG.lines().map(str::to_string).collect();
        lineas.insert(1, "server: ./no-soy-un-backend".to_string());
        lineas.insert(2, "default_backend: nadie".to_string());
        lineas.insert(7, "  server: { puerto: 1, timeout_ms: 2 }".to_string());
        lineas.join("\n") + "\n"
    }

    /// Una validación y **su** directorio, que se lee de [`ULTIMO_TEMPORAL`] —el de esta hebra— y no del
    /// contador del proceso. Ese contador no distingue «mi directorio» de «el de otra prueba que corre a
    /// la vez»: con él, una prueba lee restos que no son suyos y falla por una razón que no es la suya.
    fn validar_y_su_dir(texto: &str) -> (Result<Config, String>, PathBuf) {
        let resultado = validar(texto, &env_de);
        (resultado, ultimo_temporal())
    }

    /// El directorio que acaba de usar esta hebra.
    fn ultimo_temporal() -> PathBuf {
        ULTIMO_TEMPORAL.with(|pila| pila.borrow_mut().pop()).expect("validar ha de dejar su temporal anotado")
    }

    /// Que `ruta` ya no está en el disco.
    fn ha_desaparecido(ruta: &std::path::Path) {
        assert!(!ruta.exists(), "el temporal de la validación ha de desaparecer: {}", ruta.display());
    }

    /// `validar` escribe en su temporal y **lo retira**: el directorio de cada llamada desaparece antes
    /// de que la llamada vuelva —en el primer viaje, en el segundo, y también cuando la config es mala y
    /// se sale por el camino de error—. Un `?` que dejara el temporal atrás se vería aquí.
    #[test]
    fn validar_no_deja_restos_en_el_temporal() {
        let (resultado, dir) = validar_y_su_dir(CONFIG);
        assert!(resultado.is_ok(), "esta config es buena");
        ha_desaparecido(&dir);
        // Un segundo viaje, para que no sea la primera llamada la que limpia por casualidad.
        let (resultado, dir) = validar_y_su_dir(CONFIG);
        assert!(resultado.is_ok());
        ha_desaparecido(&dir);
        // Y una config que el daemon rechaza: el temporal se va igual.
        let (resultado, dir) = validar_y_su_dir("no es: [una config");
        assert!(resultado.is_err(), "este YAML no ha de validar");
        ha_desaparecido(&dir);
    }

    /// La guarda hace **las dos mitades de su contrato**, y se comprueban por separado a propósito:
    /// anotar el directorio —sin lo que las demás pruebas no podrían afirmar sobre «el suyo»— y
    /// retirarlo, que es lo que deja el `%TEMP%` limpio aunque la validación se salga por un `?`.
    ///
    /// El «mientras vive» se mira **desde dentro de la validación**: el `env` solo se llama en el
    /// `load`, cuando el directorio y su `config.yaml` ya están escritos y la guarda aún no ha caído.
    /// Retirar el directorio **al construirla** —el mutante que no guarda el suyo— hace que este
    /// «existe» falle, que es justo lo que hay que cazar: un `Drop` que no borra no se ve de otra forma,
    /// porque la validación siguiente reutiliza el nombre y limpia el rastro de la anterior.
    #[test]
    fn la_guarda_del_temporal_anota_el_suyo_y_lo_retira() {
        let existe_config = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let testigo = std::sync::Arc::clone(&existe_config);
        let mirando = move |_: &str| {
            // La pila se lee **ahora**, no al construir el cierre: entonces todavía no había nada.
            let vivo = ULTIMO_TEMPORAL.with(|pila| {
                pila.borrow().last().is_some_and(|ruta| ruta.exists() && ruta.join("config.yaml").exists())
            });
            testigo.store(vivo, Ordering::Relaxed);
            None
        };
        let resultado = validar(CONFIG, &mirando);
        assert!(resultado.is_ok(), "esta config es buena");
        assert!(
            existe_config.load(Ordering::Relaxed),
            "mientras la validación corre, su directorio y su config.yaml han de estar en el disco"
        );
        ha_desaparecido(&ultimo_temporal());
    }

    /// Dos validaciones **a la vez**, cada una con su directorio, y ninguna deja el suyo ni le estorba
    /// a la otra: las dos han de leer su config entera y decir que sí. Se comprueba con una config
    /// buena, que es la que obliga a leer el fichero de verdad.
    ///
    /// La barrera es lo que las hace coincidir —sin ella, una podría terminar antes de que la otra
    /// empezara y la prueba no mediría ninguna carrera—, y cada hilo devuelve **el suyo**, así que la
    /// afirmación no depende de lo que estén haciendo las demás pruebas del crate.
    #[test]
    fn validar_a_la_vez_no_se_pisan_ni_dejan_restos() {
        let barrera = std::sync::Arc::new(std::sync::Barrier::new(3));
        let correr = |barrera: std::sync::Arc<std::sync::Barrier>| {
            std::thread::spawn(move || {
                barrera.wait();
                let resultado = validar(CONFIG, &env_de);
                (resultado, ultimo_temporal())
            })
        };
        let una = correr(std::sync::Arc::clone(&barrera));
        let otra = correr(std::sync::Arc::clone(&barrera));
        barrera.wait();
        let (resultado_una, dir_una) = una.join().unwrap();
        let (resultado_otra, dir_otra) = otra.join().unwrap();
        assert!(resultado_una.is_ok() && resultado_otra.is_ok(), "las dos han de leer su config entera");
        assert_ne!(dir_una, dir_otra, "cada una ha de tener su propio directorio: {dir_una:?}");
        ha_desaparecido(&dir_una);
        ha_desaparecido(&dir_otra);
    }

    #[test]
    fn validar_usa_las_reglas_del_daemon_y_no_una_copia() {
        assert!(validar(CONFIG, &env_de).is_ok(), "esta config es buena");
        // Un campo que el daemon no conoce: `deny_unknown_fields` ha de rechazarlo.
        let sobra = CONFIG.replace("default_backend: kev", "default_backend: kev\ninventado: 1");
        let error = validar(&sobra, &env_de).unwrap_err();
        assert!(error.contains("inventado"), "el mensaje ha de nombrar el campo: {error}");
        // Un default_backend que no está entre los backends.
        let mal_default = CONFIG.replace("default_backend: kev", "default_backend: nadie");
        assert!(validar(&mal_default, &env_de).unwrap_err().contains("nadie"));
    }

    #[test]
    fn parchear_cambia_solo_el_valor_y_conserva_los_comentarios() {
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let nuevo = parchear(CONFIG, &cambios).unwrap();
        assert!(nuevo.contains("timeout_ms: 60000"), "{nuevo}");
        assert!(nuevo.contains("# el clasificador de casa"), "el comentario de la línea sobrevive: {nuevo}");
        assert!(nuevo.contains("# un comentario suelto que ha de sobrevivir"), "y el suelto también: {nuevo}");
        assert!(nuevo.contains("laya: { url: 'http://127.0.0.1:8021', timeout_ms: 120000 }"), "y laya no se toca");
        // Y lo parcheado ha de seguir validando.
        assert!(validar(&nuevo, &env_de).is_ok());
    }

    #[test]
    fn parchear_se_niega_con_un_backend_que_no_existe() {
        let cambios = [Cambio { backend: "nadie".into(), clave: "timeout_ms".into(), valor: "1".into() }];
        let error = parchear(CONFIG, &cambios).unwrap_err();
        assert!(error.contains("nadie"), "{error}");
    }

    #[test]
    fn parchear_se_niega_antes_que_perder_comentarios() {
        // Un backend en bloque, no en línea: no se reconoce, y se ha de negar en vez de reescribirlo.
        let en_bloque = r#"backends:
  kev:
    url: 'http://127.0.0.1:8009'   # en bloque, no en línea
    timeout_ms: 120000
default_backend: kev
"#;
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(en_bloque, &cambios).unwrap_err();
        assert!(error.contains("kev"), "ha de decir qué entrada no reconoce: {error}");
        // Y ha de ser **este** motivo, y no el genérico de «no se reconoce la entrada» de más abajo: el
        // plan admitía también «reconoce», pero esa palabra está en el otro mensaje, así que un
        // `parchear` que se saltara la entrada en bloque y siguiera buscaría pasaría por bueno.
        assert!(error.contains("bloque"), "ha de decir que está en bloque: {error}");
        assert!(!error.contains("no se reconoce la entrada"), "y no el mensaje de «no la encuentro»: {error}");
    }

    /// Un **ancla en línea** no se toca: entre `backend:` y su `{` no puede haber nada más que
    /// espacios, y `&base` vive justo ahí. Al reconstruir la línea el ancla desaparecería, y sus alias
    /// (`laya: *base`) quedarían apuntando a un ancla que ya no existe —un fichero roto, y encima sin
    /// que nadie se entere—. Se ha de **negar** (spec §5.4), que es lo que el doc-comment promete.
    #[test]
    fn parchear_se_niega_con_una_entrada_con_ancla_en_linea() {
        let con_ancla = "backends:\n  kev: &base { url: 'http://127.0.0.1:8009', timeout_ms: 120000 }\n  laya: *base\n";
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(con_ancla, &cambios).unwrap_err();
        assert!(error.contains("kev") && error.contains("ancla"), "ha de nombrar el caso: {error}");
        // Y no hay texto de vuelta: el `*base` sigue teniendo su ancla en el original, que es lo que
        // se salvaría si el parcheo hubiera pasado de largo.
        assert!(con_ancla.contains("&base"), "el original no se toca");
    }

    /// Un `}` dentro del **comentario de cola** no puede llevarse el comentario por delante. El cierre
    /// del mapa es el primero que hay tras el `{`, no el último de la línea: con el último, el
    /// comentario desaparece y —lo que es peor— el resultado **sigue validando**, así que se escribiría
    /// en disco sin que nadie lo note. No es un caso raro: este proyecto comenta con mapas en línea.
    ///
    /// Se parchea **la línea que lleva el comentario con la llave** (la de `laya`): es la única donde
    /// las dos formas de buscar el cierre se distinguen. Con el cierre mal elegido y un backend una
    /// línea más abajo no se vería, y una prueba así sería vacua.
    #[test]
    fn parchear_conserva_el_comentario_aunque_lleve_una_llave() {
        let con_llave = CONFIG.replace(
            "laya: { url: 'http://127.0.0.1:8021', timeout_ms: 120000 }",
            "laya: { url: 'http://127.0.0.1:8021', timeout_ms: 120000 }   # cierra con } y sigue el comentario",
        );
        let cambios = [Cambio { backend: "laya".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let nuevo = parchear(&con_llave, &cambios).unwrap();
        assert!(nuevo.contains("timeout_ms: 60000"), "{nuevo}");
        assert!(
            nuevo.contains("# cierra con } y sigue el comentario"),
            "el comentario ha de seguir entero, llave incluida: {nuevo}"
        );
        assert!(nuevo.contains("# el clasificador de casa"), "y el otro también: {nuevo}");
        assert!(nuevo.contains("ollama: { url: 'http://localhost:11434', timeout_ms: 5000 }"), "y ollama intacto");
        assert!(validar(&nuevo, &env_de).is_ok(), "y el resultado ha de validar: {nuevo}");
    }

    /// Tras el `}` de cierre, o nada, o espacios y un comentario. Cualquier otro resto se rechaza en vez
    /// de arrastrarlo: es la puerta que cierra el `<<:` de fusión y todo lo que no se entienda.
    #[test]
    fn parchear_se_niega_con_un_resto_que_no_entiende_tras_el_mapa() {
        let con_resto = CONFIG.replace(
            "kev:  { url: 'http://127.0.0.1:8009', timeout_ms: 120000 }   # el clasificador de casa",
            "kev:  { url: 'http://127.0.0.1:8009', timeout_ms: 120000 } <<: *comun",
        );
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(&con_resto, &cambios).unwrap_err();
        assert!(error.contains("kev"), "{error}");
        // Y no se ha perdido nada: el original sigue con su fusión.
        assert!(con_resto.contains("<<: *comun"), "el original no se toca");
    }

    /// La url es un cambio más, y **el backend que se pide es el que se toca**: se prueba con los dos
    /// que hay en la config, porque el `backend` del `Cambio` es un parámetro y una sola llamada lo
    /// dejaría sin ejercitar. Con solo `laya`, un `parchear` que ignorase el nombre y escribiera
    /// siempre en la primera línea de `kev:` pasaría la prueba sin que nadie se enterara.
    #[test]
    fn parchear_la_url_es_un_cambio_mas() {
        for (backend, url_vieja, url_nueva) in [("laya", "8021", "9000"), ("kev", "8009", "9100")] {
            let cambios = [Cambio {
                backend: backend.into(),
                clave: "url".into(),
                valor: format!("http://127.0.0.1:{url_nueva}"),
            }];
            let nuevo = parchear(CONFIG, &cambios).unwrap();
            assert!(nuevo.contains(&format!("http://127.0.0.1:{url_nueva}")), "{nuevo}");
            assert_eq!(
                nuevo.matches(&format!("http://127.0.0.1:{url_vieja}")).count(),
                0,
                "la url vieja de «{backend}» ha de haber desaparecido: {nuevo}"
            );
        }
        // Y el espacio de más que separa `kev:` de su mapa sigue siendo suyo, porque `kev` no se tocó.
        let cambios = [Cambio { backend: "laya".into(), clave: "url".into(), valor: "http://127.0.0.1:9000".into() }];
        let nuevo = parchear(CONFIG, &cambios).unwrap();
        assert!(nuevo.contains("kev:  { url: 'http://127.0.0.1:8009'"), "kev sigue igual: {nuevo}");
        assert_eq!(nuevo.matches("8009").count(), 1, "la url de kev aparece una sola vez: {nuevo}");
    }

    // --- Pruebas añadidas: lo que el plan pide por escrito pero no afirma con un test. ---

    /// El `env` de la validación **llega al daemon**: una variable que el daemon mira ha de cambiar el
    /// veredicto. Sin esto, un `validar` que ignorara `env` pasaría los tres casos del plan, porque
    /// los tres se deciden sin entorno.
    #[test]
    fn validar_pasa_el_entorno_al_daemon() {
        let con_env = |clave: &str| (clave == "JEVALAN_ADDR").then(|| "no-es-una-dirección".to_string());
        let error = validar(CONFIG, &con_env).unwrap_err();
        assert!(error.contains("JEVALAN_ADDR") || error.contains("dirección inválida"), "{error}");
        // Y con el entorno de siempre, la misma config sí vale: el rechazo es del entorno, no del texto.
        assert!(validar_y_su_dir(CONFIG).0.is_ok());
    }

    /// Dos cambios sobre el mismo backend salen juntos, y ninguno pisa al otro.
    #[test]
    fn parchear_dos_cambios_del_mismo_backend_a_la_vez() {
        let cambios = [
            Cambio { backend: "kev".into(), clave: "url".into(), valor: "http://127.0.0.1:9100".into() },
            Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() },
        ];
        let nuevo = parchear(CONFIG, &cambios).unwrap();
        assert!(
            nuevo.contains("kev: { url: http://127.0.0.1:9100, timeout_ms: 60000 }"),
            "los dos cambios han de caber en la línea: {nuevo}"
        );
        assert!(nuevo.contains("# el clasificador de casa"), "y el comentario sobrevivir: {nuevo}");
        assert!(validar(&nuevo, &env_de).is_ok());
    }

    /// Una clave que el backend no declara se rechaza **nombrando la clave**: callar y devolver el
    /// texto sin tocar haría creer a la pantalla que el cambio se escribió.
    #[test]
    fn parchear_se_niega_con_una_clave_que_el_backend_no_declara() {
        let cambios = [Cambio { backend: "kev".into(), clave: "path".into(), valor: "/v1".into() }];
        let error = parchear(CONFIG, &cambios).unwrap_err();
        assert!(error.contains("kev") && error.contains("path"), "{error}");
    }

    /// El parcheo mira **solo dentro del bloque `backends:`**, y esa es toda la gracia del rango: aquí
    /// hay un backend llamado `server` y, fuera del bloque, una línea `server:` (y otra
    /// `default_backend:`) que empiezan igual. Si el rango se ignorara y se buscara en todo el fichero,
    /// el cambio iría a parar a la línea de fuera —que no es del backend— y la de dentro se quedaría
    /// sin tocar; con el rango, se toca la de dentro y las de fuera quedan intactas.
    ///
    /// El texto **no** lo valida el daemon (mezcla a propósito un `default_backend` que no existe): lo
    /// que se fija es el parcheo del texto, que no ha de necesitar una config válida para no tocar lo
    /// que no entiende.
    #[test]
    fn parchear_solo_mira_dentro_del_bloque_y_no_en_todo_el_fichero() {
        let compuesto = con_senuelo();
        assert!(compuesto.starts_with("# Configuración"), "los señuelos han de quedar fuera: {compuesto}");
        let cambios = [Cambio { backend: "server".into(), clave: "timeout_ms".into(), valor: "7".into() }];
        let nuevo = parchear(&compuesto, &cambios).unwrap();
        // La de dentro del bloque, cambiada.
        assert!(nuevo.contains("  server: { puerto: 1, timeout_ms: 7 }"), "el del bloque ha de cambiar: {nuevo}");
        // Y las de fuera, intactas.
        assert!(nuevo.contains("server: ./no-soy-un-backend"), "el `server:` de fuera no se toca: {nuevo}");
        assert!(nuevo.contains("default_backend: nadie"), "ni el default_backend: {nuevo}");
        assert_eq!(nuevo.matches("timeout_ms: 2").count(), 0, "el viejo ya no está: {nuevo}");
    }

    /// El borrador **no convierte** los finales de línea del fichero: un `config.yaml` en CRLF —que es
    /// lo que hay en Windows— ha de seguir en CRLF, o cada guardado dejaría el fichero entero como
    /// cambiado para el control de versiones.
    #[test]
    fn parchear_conserva_los_finales_de_linea_crlf() {
        let crlf = CONFIG.replace('\n', "\r\n");
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let nuevo = parchear(&crlf, &cambios).unwrap();
        assert!(nuevo.contains("timeout_ms: 60000"), "{nuevo}");
        assert!(nuevo.contains("\r\n"), "ha de seguir en CRLF: {nuevo:?}");
        assert!(!nuevo.replace("\r\n", "").contains('\n'), "y no ha de mezclar los dos: {nuevo:?}");
        assert!(nuevo.ends_with("\r\n"), "y ha de acabar donde acababa: {nuevo:?}");
    }

    /// Y un fichero que **no** acaba en salto de línea no lo gana: el borrador no añade nada que el
    /// original no tuviera.
    #[test]
    fn parchear_no_anade_un_salto_de_linea_final_que_no_estaba() {
        let sin_final = CONFIG.trim_end_matches('\n');
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let nuevo = parchear(sin_final, &cambios).unwrap();
        assert!(!nuevo.ends_with('\n'), "no ha de inventarse el final: {nuevo:?}");
        assert!(nuevo.contains("timeout_ms: 60000"), "{nuevo}");
    }

    /// El bloque `backends:` se reconoce por **su línea**, no por parecerse a ella: una línea que solo
    /// empiece por `backends` —aquí `backends_dir`, y bien podría ser un comentario— no puede tomarse
    /// por el bloque, o el rango quedaría vacío y el backend pedido se leería como si no existiera.
    ///
    /// El texto de esta prueba **no** lo valida el daemon (le sobra un campo): lo que se fija aquí es el
    /// parcheo del texto, que no ha de necesitar que la config sea válida para no tocar lo que no
    /// entiende.
    #[test]
    fn parchear_no_confunde_una_linea_que_empieza_por_backends_con_el_bloque() {
        let con_dir = CONFIG.replace("server:", "backends_dir: ./backends\nserver:");
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let nuevo = parchear(&con_dir, &cambios).unwrap();
        assert!(nuevo.contains("timeout_ms: 60000"), "el cambio ha de llegar a kev: {nuevo}");
        assert!(nuevo.contains("backends_dir: ./backends"), "y el señuelo ha de quedar intacto: {nuevo}");
    }

    /// Ruta del `config/lab/config.local.yaml` de verdad, o `None` si no está.
    ///
    /// Se resuelve desde `CARGO_MANIFEST_DIR` y **se lee en tiempo de ejecución**, no con `include_str!`:
    /// ese fichero está en `.gitignore` y **no viaja con el repositorio**, así que un `include_str!`
    /// haría que un clon limpio **no compilara** —ni las pruebas ni `clippy --all-targets`—. Aquí, si
    /// falta, la prueba lo dice y se salta: en una máquina con el fichero muerde el artefacto real, y en
    /// un clon limpio las demás puertas siguen en pie.
    fn config_local_de_verdad() -> Option<String> {
        let ruta = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/lab/config.local.yaml");
        match std::fs::read_to_string(&ruta) {
            Ok(texto) => Some(texto),
            Err(e) => {
                eprintln!(
                    "AVISO: se salta la prueba del fichero real: no se puede leer {} ({e}). Ese fichero está en .gitignore y no viaja con el repositorio, así que su ausencia en un clon limpio es lo esperado.",
                    ruta.display()
                );
                None
            }
        }
    }

    /// **El caso real**: `config/lab/config.local.yaml`, que es el fichero que la pantalla abre de verdad.
    /// Se lee el artefacto y no una copia —si alguien lo cambia a mapas en bloque, o le quita los
    /// comentarios, esto lo dice—, se parchea, se comprueban los comentarios y **se valida con el
    /// daemon**, que es el viaje entero. Si el fichero no está (clon limpio: está ignorado por git), la
    /// prueba se salta diciéndolo, en vez de impedir que el crate compile.
    #[test]
    fn parchear_el_config_local_de_verdad_conserva_sus_comentarios_y_valida() {
        let Some(real) = config_local_de_verdad() else { return };
        assert!(real.contains("# Daemon íntegramente local"), "el fichero real ha de traer sus comentarios");
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "9000".into() }];
        let nuevo = parchear(&real, &cambios).expect("el config.local.yaml de verdad ha de poder parchearse");
        // Ojo al espacio: el fichero trae `kev:  ` con dos, y la línea parcheada sale con uno, porque se
        // reconstruye. Es de la línea que se toca; las que no se tocan conservan el suyo (laya, abajo).
        assert!(nuevo.contains("kev: { url: \"http://127.0.0.1:8009\", timeout_ms: 9000 }"), "{nuevo}");
        assert!(nuevo.contains("# Daemon íntegramente local"), "el comentario de cabecera sobrevive: {nuevo}");
        assert!(nuevo.contains("# Requiere Kev en :8009"), "y el segundo también: {nuevo}");
        assert!(nuevo.contains("laya: { url: \"http://127.0.0.1:8021\", timeout_ms: 5000 }"), "y laya no se toca");
        assert!(validar(&nuevo, &env_de).is_ok(), "y el resultado ha de validar con el daemon: {nuevo}");
    }

    /// La validación es **la del daemon**: una `url` que no sea http(s) la rechaza él, y aquí se
    /// comprueba que `validar` lo dice, en vez de darla por buena porque el YAML esté bien formado.
    #[test]
    fn validar_rechaza_una_url_que_no_es_http() {
        let mala = CONFIG.replace("'http://127.0.0.1:8009'", "no-es-http");
        let error = validar(&mala, &env_de).unwrap_err();
        assert!(error.contains("no-es-http") || error.contains("http"), "el motivo ha de nombrar la url: {error}");
        assert!(validar(CONFIG, &env_de).is_ok(), "y la buena sigue valiendo");
    }

    /// Un fichero con los finales **mezclados** sale unificado al estilo que ya predominaba: no queda
    /// con dos estilos a la vez, que es lo que hace que el control de versiones lo enseñe entero como
    /// cambiado. Es la decisión que declara el doc-comment de `parchear`, y aquí queda fijada.
    ///
    /// Se mira **cada `\r`, uno a uno**: «hay algún CRLF» no basta, porque un `\r` suelto —el que deja
    /// una línea CRLF entre líneas LF— pasaría ese filtro y el fichero seguiría mezclado. Esa es la
    /// aserción que muerde: sin ella, la prueba daba verde con un `\r` suelto dentro.
    #[test]
    fn parchear_normaliza_un_fichero_con_finales_mezclados() {
        // Una sola línea CRLF entre líneas LF: el estilo elegido es CRLF, y no ha de quedar ni un LF
        // ni un CR sueltos.
        let mezclado = CONFIG.replacen("server:\n", "server:\r\n", 1);
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let nuevo = parchear(&mezclado, &cambios).unwrap();
        assert!(nuevo.contains("timeout_ms: 60000"), "{nuevo}");
        assert!(
            !nuevo.replace("\r\n", "\n").contains('\r'),
            "no puede quedar un CR que no encabece un CRLF: {nuevo:?}"
        );
        assert!(!nuevo.replace("\r\n", "").contains('\n'), "ni un LF suelto: {nuevo:?}");
        assert!(nuevo.contains("\r\n"), "y el estilo elegido ha de ser el que ya tenía: {nuevo:?}");
    }

    // --- Ronda de arreglo 2 ---

    /// Una `}` **dentro de un valor entrecomillado** no es el final del mapa. Tomarla por el cierre hace
    /// que el resto del valor se lea como «algo que no se entiende» y que una entrada perfectamente en
    /// línea se rechace: una negativa falsa, que impide guardar algo bueno. Se prueba el valor, y una
    /// llave justo al final del valor, que es el borde.
    #[test]
    fn parchear_acepta_una_llave_dentro_de_un_valor_entrecomillado() {
        let con_llave_en_valor = CONFIG.replace("'http://127.0.0.1:8009'", "'http://127.0.0.1:8009/a}b'");
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let nuevo = parchear(&con_llave_en_valor, &cambios).unwrap();
        assert!(nuevo.contains("timeout_ms: 60000"), "el cambio ha de hacerse: {nuevo}");
        assert!(nuevo.contains("'http://127.0.0.1:8009/a}b'"), "y el valor con su llave ha de quedar igual: {nuevo}");
        assert!(nuevo.contains("# el clasificador de casa"), "y el comentario sobrevivir: {nuevo}");
        // La llave al final del valor: la primera `}` de fuera de comillas sigue siendo la del mapa.
        let al_final = CONFIG.replace("'http://127.0.0.1:8009'", "'http://127.0.0.1:8009/a}'");
        let nuevo = parchear(&al_final, &cambios).unwrap();
        assert!(nuevo.contains("timeout_ms: 60000"), "{nuevo}");
        assert!(nuevo.contains("'http://127.0.0.1:8009/a}'"), "{nuevo}");
    }

    /// La ayuda que decide dónde acaba el mapa, con sus cuatro casos: una llave en un valor, una llave
    /// en el comentario de cola (que no ha de contar como cierre), y las comillas de los dos estilos.
    #[test]
    fn las_llaves_del_mapa_no_cuentan_las_que_van_dentro_de_comillas() {
        // Sin llaves de más: el cierre es el de verdad, y la cola empieza donde debe.
        let simple = "{ url: 'x', timeout_ms: 1 }   # fin";
        let (abre, cierra) = llaves_del_mapa(simple);
        assert_eq!((abre, cierra), (Some(0), Some(simple.find('}').unwrap())));
        // Una llave en el valor entrecomillado: el cierre sigue siendo el de fuera.
        let en_valor = "{ url: 'a}b', timeout_ms: 1 }   # fin";
        let (_, cierra) = llaves_del_mapa(en_valor);
        let cierre_esperado = en_valor.rfind('}').unwrap();
        assert_eq!(cierra, Some(cierre_esperado), "el de fuera, no el de dentro de las comillas");
        assert!(en_valor[cierre_esperado + 1..].trim_start().starts_with('#'), "y la cola es el comentario");
        // Comillas dobles, el otro estilo que aparece en el fichero real.
        let dobles = "{ url: \"a}b\", timeout_ms: 1 }";
        assert_eq!(llaves_del_mapa(dobles).1, Some(dobles.rfind('}').unwrap()));
        // Y el comentario no aporta cierre: si la llave estuviera solo en el comentario, el mapa se
        // queda sin cerrar y esto ha de decirlo —así `parchear` se niega en vez de comerse el comentario,
        // y no se lía con el apóstrofo del propio comentario—.
        assert_eq!(llaves_del_mapa("{ url: 'x'   # cierra con }").1, None);
    }

    /// Un mapa **dentro** del mapa no se sabe reescribir: se niega, en vez de escribir una línea a
    /// medias con lo que se creyera el cierre.
    #[test]
    fn parchear_se_niega_con_un_mapa_anidado_dentro_del_mapa() {
        let anidado = CONFIG.replace("'http://127.0.0.1:8009'", "{ interno: 1 }");
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(&anidado, &cambios).unwrap_err();
        assert!(error.contains("kev") && error.contains("mapa"), "ha de nombrar el caso: {error}");
        assert!(anidado.contains("{ interno: 1 }"), "y el original no se toca");
    }

    /// Un **alias** (`laya: *base`) no declara un mapa en línea y no se parchea, pero el motivo ha de
    /// nombrar lo que hay: decir «está en bloque» de un alias manda a mirar donde no es.
    #[test]
    fn parchear_se_niega_con_un_alias_y_lo_dice() {
        let con_alias = CONFIG.replace("laya: { url: 'http://127.0.0.1:8021', timeout_ms: 120000 }", "laya: *base");
        let cambios = [Cambio { backend: "laya".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(&con_alias, &cambios).unwrap_err();
        assert!(error.contains("laya"), "ha de decir de qué entrada habla: {error}");
        assert!(error.contains("*base"), "y ha de enseñar lo que se ha encontrado: {error}");
        assert!(error.contains("alias"), "nombrando el caso, no «en bloque»: {error}");
    }

    /// Una **clave de fusión delante** del mapa sí se niega —reconstruir la línea la borraría—, mientras
    /// que **dentro** del mapa se conserva tal cual, porque es un valor más. Las dos mitades, para que
    /// el doc-comment no diga más de lo que el código hace.
    #[test]
    fn parchear_distingue_una_fusion_delante_del_mapa_de_una_dentro() {
        let delante = CONFIG.replace(
            "kev:  { url: 'http://127.0.0.1:8009', timeout_ms: 120000 }",
            "kev:  <<: { url: 'http://127.0.0.1:8009', timeout_ms: 120000 }",
        );
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(&delante, &cambios).unwrap_err();
        assert!(error.contains("fusión"), "ha de nombrar la fusión: {error}");

        let dentro = CONFIG.replace(
            "kev:  { url: 'http://127.0.0.1:8009', timeout_ms: 120000 }",
            "kev:  { url: 'http://127.0.0.1:8009', timeout_ms: 120000, <<: *comun }",
        );
        let nuevo = parchear(&dentro, &cambios).expect("una fusión dentro es un valor más, y no se rechaza");
        assert!(nuevo.contains("timeout_ms: 60000"), "{nuevo}");
        assert!(nuevo.contains("<<: *comun"), "y la fusión se conserva tal cual: {nuevo}");
    }
}
