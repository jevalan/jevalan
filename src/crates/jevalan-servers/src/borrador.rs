//! The daemon config draft: the **text** is edited, it is not reserialised.
//!
//! `serde_yaml` does not preserve comments and `config/lab/config.local.yaml` is commented, so the screen
//! patches the text line by line. Validation, in contrast, is the daemon's: `config::load`.

use jevalan_daemon::config::{Config, load};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// How many temporaries this process has requested. It is what makes each validation's directory
/// **unique**, and it is not decoration: the screen validates while typing, and two validations at once
/// (two TUI threads, or two of the crate's own tests, which run in parallel) would share
/// `config.yaml` and one would delete the other's directory halfway through reading. That shows up as
/// a "cannot be read" on a text that was in fact good.
static TEMPORALES: AtomicU64 = AtomicU64::new(0);

// The temporary this **thread** has just used, so that a test can assert on its own and not
// on the one next door. The counter above is process-wide, so with it alone one cannot
// tell "my directory" from "that of another test running at the same time": this can.
//
// It is under `cfg(test)` because in production there is nothing to look at here, and it is a stack (not a single
// slot) so that nested validations do not trample themselves.
#[cfg(test)]
thread_local! {
    static ULTIMO_TEMPORAL: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The name of the next validation's temporary directory: the pid, so as not to clobber another
/// process, and a counter, so as not to clobber itself.
fn nombre_temporal() -> String {
    format!("jevalan-borrador-{}-{}", std::process::id(), TEMPORALES.fetch_add(1, Ordering::Relaxed))
}

/// A value to rewrite inside the `backends:` block. Values only: backends are never added or removed
/// from the screen, because that changes the set and not a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cambio {
    pub backend: String,
    pub clave: String,
    pub valor: String,
}

/// A temporary directory removed on leaving its scope, also if something fails along the way.
///
/// Without it, a `load` that returned early would leave the directory in `%TEMP%` on every validation, and
/// going back to the function to add the deletion on every exit branch is exactly what gets forgotten. That
/// the deletion is "best effort" (and not an error to propagate) is deliberate: there is nothing
/// the caller can do about a `%TEMP%` that will not be cleaned, and failing validation for that
/// would turn a cleanup problem into a correctness one.
struct Temporal(PathBuf);

impl Drop for Temporal {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Validates a config text with **the daemon's rules**, without touching the user's file.
///
/// `config::load` takes a path, so the text is left in a temporary file and read from there: it is
/// the only way for the authority to be the daemon itself and not a copy of its rules. What is not
/// touched is the **user's** config file: it is not opened, written or moved.
///
/// The temporary is **one per call** (the name carries the pid and a counter) and is removed before
/// returning, with the guard that also covers the early return of `?`, so that validating the draft
/// while typing leaves no trace on disk and does not get in the way of the validation next door.
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

/// Rewrites the requested values inside the `backends:` block, preserving comments and order.
///
/// **Inline** entries are recognised (`kev: { url: …, timeout_ms: … }`), which is how they are today
/// in `config/lab/config.local.yaml`. Everything else is **refused** with an explicit message, instead of
/// rewriting the file and losing something: a block map, an **inline anchor** (`kev: &base { … }`,
/// whose anchor would vanish leaving its aliases without a target), a **merge key** (`<<:`) or
/// any remainder that is not understood. The rule is simple and conservative: **between `backend:` and its `{`
/// there can only be spaces**, and after the closing `}`, either nothing, or spaces and a comment.
///
/// Each change is applied on the result of the previous one, so two changes to the same backend
/// (the url and the timeout, say) both come out on the same line.
///
/// **Line endings.** The whole file is rewritten with **a single** style: CRLF if the
/// original had any, and LF if it had none. A **mixed** file (a CRLF line among
/// LF lines, which is what an editor that errs once leaves behind) therefore comes out unified to the style
/// that already predominated, not preserved line by line. It is chosen this way because all the lines are joined
/// back with the same separator anyway, and because what really matters is that the file does not
/// end up with two styles at once: that is what makes version control show it as entirely
/// changed. It is tested in both directions (CRLF stays CRLF, and the mixed one is unified).
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
    // The original file's line ending is preserved: the dominant one, if they were mixed.
    // Since all the lines are joined back with **the same** separator, a mixed file already comes out
    // unified to the dominant style, with no normalisation pass needed afterwards: `lines()`
    // takes each line's `\r` with it, and the `join` puts its own.
    let cr = if texto.contains("\r\n") { "\r\n" } else { "\n" };
    let mut salida = lineas.join(cr);
    if texto.ends_with('\n') {
        salida.push_str(cr);
    }
    Ok(salida)
}

/// The bounds of the `backends:` block: from its line to the first one that is not indented further.
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

/// If `linea` declares `cambio.backend` inline, returns the line with its `clave` rewritten.
///
/// The three "no"s it returns are distinct on purpose: `Ok(None)` is "this line is not for that
/// backend, keep looking"; `Err` (block entry, anchor, merge key, a tail that is not understood,
/// or a key the backend does not declare) is "this one is it, and it cannot be patched"; and `Ok(Some(…))` is the
/// change made.
///
/// What is required, and why:
///
/// - **Between `backend:` and its `{` there can be nothing but spaces.** An anchor (`kev: &base { … }`)
///   lives right there, and rebuilding the line would lose it: its aliases (`laya: *base`) would be left
///   pointing at an anchor that no longer exists. In front of the map, a merge key (`kev: <<: { … }`)
///   is refused for the same reason. **Inside** the map, however, a `<<:` or another key is kept
///   as is: it is just another value, copied without touching it, and refusing because of it would be inventing a problem.
/// - **The `}` that closes the map is the first one after the `{`, and one inside
///   quotes does not count**, nor does one in a trailing comment (and this project comments with inline-map
///   examples): with the last brace of the line, the comment would disappear, and since the result would still
///   validate it would be written to disk without anyone noticing. With the first one outside quotes,
///   the comment and the values containing braces stay where they were.
/// - **After the closing `}`, either nothing, or spaces and a comment.** That is what shuts the door on
///   any remainder that is not understood.
fn reescribir(linea: &str, cambio: &Cambio) -> Result<Option<String>, String> {
    let recortada = linea.trim_start();
    let Some(resto) = recortada.strip_prefix(&format!("{}:", cambio.backend)) else {
        return Ok(None);
    };
    // It must be an entry with its inline map; if there are no braces, there is no map to rewrite: it could
    // be a block map, an alias (`laya: *base`) or something unrecognised. It is refused without saying
    // which (the text is not enough to tell them apart without a YAML parser) and what is there is shown.
    let (Some(abre), Some(cierra)) = llaves_del_mapa(resto) else {
        return Err(format!(
            "la entrada del backend «{}» no declara su mapa en línea (¿un bloque, un alias?): «{}» no se parchea, para no perder lo que hubiera",
            cambio.backend,
            resto.trim()
        ));
    };
    let delante = resto[..abre].trim();
    if !delante.is_empty() {
        // What precedes the map is an anchor (`&base`) or a merge key (`<<`); in both
        // cases, rebuilding the line would erase it: the anchor would orphan its aliases, and the merge
        // would lose what it contributes. The case is named so the message is of some use.
        let caso = if delante.starts_with('&') { "un ancla" } else { "una clave de fusión" };
        return Err(format!(
            "la entrada del backend «{}» lleva {caso} («{delante}») delante de su mapa: no se parchea para no dejarlo huérfano",
            cambio.backend
        ));
    }
    let cola = &resto[cierra + 1..];
    // The tail must be nothing, or spaces and a comment. Otherwise it is something that is not understood.
    let cola_limpia = cola.trim_start();
    if !(cola_limpia.is_empty() || cola_limpia.starts_with('#')) {
        return Err(format!(
            "tras el mapa del backend «{}» hay algo que no se entiende («{}»): no se parchea para no perderlo",
            cambio.backend, cola_limpia
        ));
    }
    let dentro = &resto[abre + 1..cierra];
    // If inside the map there is a `{` outside quotes, what was taken for the closing one is not: it would be
    // a nested entry, which this line-by-line patching does not know how to rewrite. It is refused instead of writing
    // a half-done line.
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
    // Any comment after the brace is kept as is.
    Ok(Some(format!("{sangria}{}: {{ {} }}{cola}", cambio.backend, partes.join(", "))))
}

/// The braces of the inline map in `resto`: the **first** `{` and the **first** `}` that closes it.
///
/// The closing one cannot be the last on the line: a brace inside the trailing comment (and this
/// project comments with inline-map examples) would make "the last one" take the comment with it.
/// With the first one that closes the map, the comment stays where it was.
///
/// Neither the brace **inside quotes** counts (`{ url: '…/a}b' }` is a value with a
/// brace, not the end of the map) nor anything **after a `#`**, which is already a comment: a
/// comment saying "closes with }" closes nothing, and if the only brace were there the map is
/// left unclosed and `parchear` refuses, which is correct. The quoting is followed by hand, which
/// is enough for a map on one line.
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

    /// A piece of `config/lab/config.local.yaml`, with its comments and its inline maps.
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

    /// A backend named like the key outside the block, to check two things at once: that
    /// the patching looks **only** inside the `backends:` block (if it looked in the whole file, it would touch the
    /// `server:` above, which is what the range prevents) and that a line outside that starts with the
    /// name of a backend is not mistaken for its entry.
    fn con_senuelo() -> String {
        let mut lineas: Vec<String> = CONFIG.lines().map(str::to_string).collect();
        lineas.insert(1, "server: ./no-soy-un-backend".to_string());
        lineas.insert(2, "default_backend: nadie".to_string());
        lineas.insert(7, "  server: { puerto: 1, timeout_ms: 2 }".to_string());
        lineas.join("\n") + "\n"
    }

    /// A validation and **its** directory, read from [`ULTIMO_TEMPORAL`] (this thread's) and not from the
    /// process counter. That counter cannot tell "my directory" from "that of another test running at
    /// the same time": with it, a test reads leftovers that are not its own and fails for a reason that is not its own.
    fn validar_y_su_dir(texto: &str) -> (Result<Config, String>, PathBuf) {
        let resultado = validar(texto, &env_de);
        (resultado, ultimo_temporal())
    }

    /// The directory this thread has just used.
    fn ultimo_temporal() -> PathBuf {
        ULTIMO_TEMPORAL.with(|pila| pila.borrow_mut().pop()).expect("validar ha de dejar su temporal anotado")
    }

    /// That `ruta` is no longer on disk.
    fn ha_desaparecido(ruta: &std::path::Path) {
        assert!(!ruta.exists(), "el temporal de la validación ha de desaparecer: {}", ruta.display());
    }

    /// `validar` writes to its temporary and **removes it**: each call's directory disappears before
    /// the call returns (on the first trip, on the second, and also when the config is bad and
    /// it exits through the error path). A `?` that left the temporary behind would be seen here.
    #[test]
    fn validar_no_deja_restos_en_el_temporal() {
        let (resultado, dir) = validar_y_su_dir(CONFIG);
        assert!(resultado.is_ok(), "esta config es buena");
        ha_desaparecido(&dir);
        // A second trip, so it is not the first call that cleans up by chance.
        let (resultado, dir) = validar_y_su_dir(CONFIG);
        assert!(resultado.is_ok());
        ha_desaparecido(&dir);
        // And a config the daemon rejects: the temporary goes away just the same.
        let (resultado, dir) = validar_y_su_dir("no es: [una config");
        assert!(resultado.is_err(), "este YAML no ha de validar");
        ha_desaparecido(&dir);
    }

    /// The guard does **both halves of its contract**, and they are checked separately on purpose:
    /// noting the directory (without which the other tests could not assert about "their own") and
    /// removing it, which is what leaves `%TEMP%` clean even if validation exits through a `?`.
    ///
    /// The "while it lives" is checked **from inside the validation**: `env` is only called in
    /// `load`, when the directory and its `config.yaml` are already written and the guard has not yet dropped.
    /// Removing the directory **on construction** (the mutant that does not keep its own) makes this
    /// "exists" fail, which is exactly what must be caught: a `Drop` that does not delete cannot be seen any other way,
    /// because the next validation reuses the name and cleans up the previous one's trace.
    #[test]
    fn la_guarda_del_temporal_anota_el_suyo_y_lo_retira() {
        let existe_config = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let testigo = std::sync::Arc::clone(&existe_config);
        let mirando = move |_: &str| {
            // The stack is read **now**, not when the closure is built: back then there was nothing yet.
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

    /// Two validations **at the same time**, each with its directory, and neither leaves its own nor gets in
    /// the other's way: both must read their whole config and say yes. It is checked with a good
    /// config, which is the one that forces actually reading the file.
    ///
    /// The barrier is what makes them coincide (without it, one could finish before the other
    /// started and the test would measure no race), and each thread returns **its own**, so the
    /// assertion does not depend on what the crate's other tests are doing.
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
        // A field the daemon does not know: `deny_unknown_fields` must reject it.
        let sobra = CONFIG.replace("default_backend: kev", "default_backend: kev\ninventado: 1");
        let error = validar(&sobra, &env_de).unwrap_err();
        assert!(error.contains("inventado"), "el mensaje ha de nombrar el campo: {error}");
        // A default_backend that is not among the backends.
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
        // And what was patched must still validate.
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
        // A block backend, not an inline one: it is not recognised, and must be refused instead of rewritten.
        let en_bloque = r#"backends:
  kev:
    url: 'http://127.0.0.1:8009'   # en bloque, no en línea
    timeout_ms: 120000
default_backend: kev
"#;
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(en_bloque, &cambios).unwrap_err();
        assert!(error.contains("kev"), "ha de decir qué entrada no reconoce: {error}");
        // And it must be **this** reason, and not the generic "entry not recognised" one further below: the
        // plan also admitted "recognise", but that word is in the other message, so a
        // `parchear` that skipped the block entry and kept searching would pass as good.
        assert!(error.contains("bloque"), "ha de decir que está en bloque: {error}");
        assert!(!error.contains("no se reconoce la entrada"), "y no el mensaje de «no la encuentro»: {error}");
    }

    /// An **inline anchor** is not touched: between `backend:` and its `{` there can be nothing but
    /// spaces, and `&base` lives right there. Rebuilding the line would make the anchor disappear, and its aliases
    /// (`laya: *base`) would be left pointing at an anchor that no longer exists (a broken file, and with
    /// no one the wiser). It must be **refused** (spec §5.4), which is what the doc-comment promises.
    #[test]
    fn parchear_se_niega_con_una_entrada_con_ancla_en_linea() {
        let con_ancla = "backends:\n  kev: &base { url: 'http://127.0.0.1:8009', timeout_ms: 120000 }\n  laya: *base\n";
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(con_ancla, &cambios).unwrap_err();
        assert!(error.contains("kev") && error.contains("ancla"), "ha de nombrar el caso: {error}");
        // And there is no text returned: the `*base` still has its anchor in the original, which is what
        // would be saved if the patching had gone past.
        assert!(con_ancla.contains("&base"), "el original no se toca");
    }

    /// A `}` inside the **trailing comment** must not take the comment with it. The map's
    /// closing is the first one after the `{`, not the last on the line: with the last, the
    /// comment disappears and, what is worse, the result **still validates**, so it would be written
    /// to disk without anyone noticing. It is not a rare case: this project comments with inline maps.
    ///
    /// **The line carrying the comment with the brace** (`laya`'s) is patched: it is the only one where
    /// the two ways of finding the closing differ. With the closing chosen wrongly and a backend one
    /// line further down it would not show, and a test like that would be vacuous.
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

    /// After the closing `}`, either nothing, or spaces and a comment. Any other remainder is rejected instead
    /// of being dragged along: it is the door shut by the merge `<<:` and everything that is not understood.
    #[test]
    fn parchear_se_niega_con_un_resto_que_no_entiende_tras_el_mapa() {
        let con_resto = CONFIG.replace(
            "kev:  { url: 'http://127.0.0.1:8009', timeout_ms: 120000 }   # el clasificador de casa",
            "kev:  { url: 'http://127.0.0.1:8009', timeout_ms: 120000 } <<: *comun",
        );
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(&con_resto, &cambios).unwrap_err();
        assert!(error.contains("kev"), "{error}");
        // And nothing was lost: the original still has its merge.
        assert!(con_resto.contains("<<: *comun"), "el original no se toca");
    }

    /// The url is one more change, and **the backend asked for is the one touched**: it is tested with the two
    /// in the config, because the `Cambio`'s `backend` is a parameter and a single call would
    /// leave it unexercised. With `laya` alone, a `parchear` that ignored the name and always wrote
    /// on `kev:`'s first line would pass the test without anyone noticing.
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
        // And the extra space separating `kev:` from its map is still its own, because `kev` was not touched.
        let cambios = [Cambio { backend: "laya".into(), clave: "url".into(), valor: "http://127.0.0.1:9000".into() }];
        let nuevo = parchear(CONFIG, &cambios).unwrap();
        assert!(nuevo.contains("kev:  { url: 'http://127.0.0.1:8009'"), "kev sigue igual: {nuevo}");
        assert_eq!(nuevo.matches("8009").count(), 1, "la url de kev aparece una sola vez: {nuevo}");
    }

    // --- Added tests: what the plan asks for in writing but does not assert with a test. ---

    /// The validation's `env` **reaches the daemon**: a variable the daemon looks at must change the
    /// verdict. Without this, a `validar` that ignored `env` would pass the plan's three cases, because
    /// all three are decided without an environment.
    #[test]
    fn validar_pasa_el_entorno_al_daemon() {
        let con_env = |clave: &str| (clave == "JEVALAN_ADDR").then(|| "no-es-una-dirección".to_string());
        let error = validar(CONFIG, &con_env).unwrap_err();
        assert!(error.contains("JEVALAN_ADDR") || error.contains("dirección inválida"), "{error}");
        // And with the usual environment, the same config is valid: the rejection is the environment's, not the text's.
        assert!(validar_y_su_dir(CONFIG).0.is_ok());
    }

    /// Two changes to the same backend come out together, and neither tramples the other.
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

    /// A key the backend does not declare is rejected **naming the key**: staying quiet and returning the
    /// text untouched would make the screen believe the change was written.
    #[test]
    fn parchear_se_niega_con_una_clave_que_el_backend_no_declara() {
        let cambios = [Cambio { backend: "kev".into(), clave: "path".into(), valor: "/v1".into() }];
        let error = parchear(CONFIG, &cambios).unwrap_err();
        assert!(error.contains("kev") && error.contains("path"), "{error}");
    }

    /// The patching looks **only inside the `backends:` block**, and that is the whole point of the range: here
    /// there is a backend called `server` and, outside the block, a `server:` line (and another
    /// `default_backend:`) that start the same. If the range were ignored and the whole file searched,
    /// the change would end up on the outside line (which is not the backend's) and the one inside would be left
    /// untouched; with the range, the inside one is touched and the outside ones stay intact.
    ///
    /// The text is **not** validated by the daemon (it deliberately mixes in a `default_backend` that does not exist): what
    /// is fixed is the text patching, which must not need a valid config in order not to touch what
    /// it does not understand.
    #[test]
    fn parchear_solo_mira_dentro_del_bloque_y_no_en_todo_el_fichero() {
        let compuesto = con_senuelo();
        assert!(compuesto.starts_with("# Configuración"), "los señuelos han de quedar fuera: {compuesto}");
        let cambios = [Cambio { backend: "server".into(), clave: "timeout_ms".into(), valor: "7".into() }];
        let nuevo = parchear(&compuesto, &cambios).unwrap();
        // The one inside the block, changed.
        assert!(nuevo.contains("  server: { puerto: 1, timeout_ms: 7 }"), "el del bloque ha de cambiar: {nuevo}");
        // And those outside, intact.
        assert!(nuevo.contains("server: ./no-soy-un-backend"), "el `server:` de fuera no se toca: {nuevo}");
        assert!(nuevo.contains("default_backend: nadie"), "ni el default_backend: {nuevo}");
        assert_eq!(nuevo.matches("timeout_ms: 2").count(), 0, "el viejo ya no está: {nuevo}");
    }

    /// The draft **does not convert** the file's line endings: a CRLF `config.yaml` (which is
    /// what there is on Windows) must stay CRLF, or each save would leave the whole file looking
    /// changed to version control.
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

    /// And a file that does **not** end in a newline does not gain one: the draft adds nothing the
    /// original did not have.
    #[test]
    fn parchear_no_anade_un_salto_de_linea_final_que_no_estaba() {
        let sin_final = CONFIG.trim_end_matches('\n');
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let nuevo = parchear(sin_final, &cambios).unwrap();
        assert!(!nuevo.ends_with('\n'), "no ha de inventarse el final: {nuevo:?}");
        assert!(nuevo.contains("timeout_ms: 60000"), "{nuevo}");
    }

    /// The `backends:` block is recognised by **its line**, not by resembling it: a line that merely
    /// starts with `backends` (here `backends_dir`, and it could well be a comment) cannot be taken
    /// for the block, or the range would be empty and the requested backend would be read as if it did not exist.
    ///
    /// This test's text is **not** validated by the daemon (it has an extra field): what is fixed here is the
    /// text patching, which must not need the config to be valid in order not to touch what it does not
    /// understand.
    #[test]
    fn parchear_no_confunde_una_linea_que_empieza_por_backends_con_el_bloque() {
        let con_dir = CONFIG.replace("server:", "backends_dir: ./backends\nserver:");
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let nuevo = parchear(&con_dir, &cambios).unwrap();
        assert!(nuevo.contains("timeout_ms: 60000"), "el cambio ha de llegar a kev: {nuevo}");
        assert!(nuevo.contains("backends_dir: ./backends"), "y el señuelo ha de quedar intacto: {nuevo}");
    }

    /// Path of the real `config/lab/config.local.yaml`, or `None` if it is not there.
    ///
    /// It is resolved from `CARGO_MANIFEST_DIR` and **read at run time**, not with `include_str!`:
    /// that file is in `.gitignore` and **does not travel with the repository**, so an `include_str!`
    /// would make a clean clone **fail to compile** (neither the tests nor `clippy --all-targets`). Here, if
    /// it is missing, the test says so and is skipped: on a machine with the file the real artefact bites, and in
    /// a clean clone the other gates stay standing.
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

    /// **The real case**: `config/lab/config.local.yaml`, which is the file the screen actually opens.
    /// The artefact is read and not a copy (if someone changes it to block maps, or strips its
    /// comments, this says so), patched, the comments are checked and **it is validated with the
    /// daemon**, which is the whole trip. If the file is not there (clean clone: it is ignored by git), the
    /// test is skipped saying so, instead of preventing the crate from compiling.
    #[test]
    fn parchear_el_config_local_de_verdad_conserva_sus_comentarios_y_valida() {
        let Some(real) = config_local_de_verdad() else { return };
        assert!(real.contains("# Fully local daemon"), "el fichero real ha de traer sus comentarios");
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "9000".into() }];
        let nuevo = parchear(&real, &cambios).expect("el config.local.yaml de verdad ha de poder parchearse");
        // Mind the space: the file has `kev:  ` with two, and the patched line comes out with one, because it is
        // rebuilt. That is for the line that is touched; those not touched keep theirs (laya, below).
        assert!(nuevo.contains("kev: { url: \"http://127.0.0.1:8009\", timeout_ms: 9000 }"), "{nuevo}");
        assert!(nuevo.contains("# Fully local daemon"), "el comentario de cabecera sobrevive: {nuevo}");
        assert!(nuevo.contains("# Needs Kev on :8009"), "y el segundo también: {nuevo}");
        assert!(nuevo.contains("laya: { url: \"http://127.0.0.1:8021\", timeout_ms: 5000 }"), "y laya no se toca");
        assert!(validar(&nuevo, &env_de).is_ok(), "y el resultado ha de validar con el daemon: {nuevo}");
    }

    /// Validation is **the daemon's**: a `url` that is not http(s) is rejected by it, and here it is
    /// checked that `validar` says so, instead of accepting it because the YAML is well formed.
    #[test]
    fn validar_rechaza_una_url_que_no_es_http() {
        let mala = CONFIG.replace("'http://127.0.0.1:8009'", "no-es-http");
        let error = validar(&mala, &env_de).unwrap_err();
        assert!(error.contains("no-es-http") || error.contains("http"), "el motivo ha de nombrar la url: {error}");
        assert!(validar(CONFIG, &env_de).is_ok(), "y la buena sigue valiendo");
    }

    /// A file with **mixed** endings comes out unified to the style that already predominated: it does not
    /// end up with two styles at once, which is what makes version control show it as entirely
    /// changed. It is the decision `parchear`'s doc-comment declares, and here it is pinned down.
    ///
    /// **Each `\r` is checked, one by one**: "there is some CRLF" is not enough, because a stray `\r` (the one a
    /// CRLF line leaves among LF lines) would pass that filter and the file would still be mixed. That is the
    /// assertion that bites: without it, the test went green with a stray `\r` inside.
    #[test]
    fn parchear_normaliza_un_fichero_con_finales_mezclados() {
        // A single CRLF line among LF lines: the chosen style is CRLF, and not a single stray LF
        // or CR must remain.
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

    // --- Fix round 2 ---

    /// A `}` **inside a quoted value** is not the end of the map. Taking it for the closing makes
    /// the rest of the value read as "something not understood" and a perfectly inline entry
    /// be rejected: a false refusal, which prevents saving something good. The value is tested, and a
    /// brace right at the end of the value, which is the edge.
    #[test]
    fn parchear_acepta_una_llave_dentro_de_un_valor_entrecomillado() {
        let con_llave_en_valor = CONFIG.replace("'http://127.0.0.1:8009'", "'http://127.0.0.1:8009/a}b'");
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let nuevo = parchear(&con_llave_en_valor, &cambios).unwrap();
        assert!(nuevo.contains("timeout_ms: 60000"), "el cambio ha de hacerse: {nuevo}");
        assert!(nuevo.contains("'http://127.0.0.1:8009/a}b'"), "y el valor con su llave ha de quedar igual: {nuevo}");
        assert!(nuevo.contains("# el clasificador de casa"), "y el comentario sobrevivir: {nuevo}");
        // The brace at the end of the value: the first `}` outside quotes is still the map's.
        let al_final = CONFIG.replace("'http://127.0.0.1:8009'", "'http://127.0.0.1:8009/a}'");
        let nuevo = parchear(&al_final, &cambios).unwrap();
        assert!(nuevo.contains("timeout_ms: 60000"), "{nuevo}");
        assert!(nuevo.contains("'http://127.0.0.1:8009/a}'"), "{nuevo}");
    }

    /// The helper that decides where the map ends, with its four cases: a brace in a value, a brace
    /// in the trailing comment (which must not count as the closing), and the quotes of both styles.
    #[test]
    fn las_llaves_del_mapa_no_cuentan_las_que_van_dentro_de_comillas() {
        // With no extra braces: the closing is the real one, and the tail starts where it should.
        let simple = "{ url: 'x', timeout_ms: 1 }   # fin";
        let (abre, cierra) = llaves_del_mapa(simple);
        assert_eq!((abre, cierra), (Some(0), Some(simple.find('}').unwrap())));
        // A brace in the quoted value: the closing is still the outside one.
        let en_valor = "{ url: 'a}b', timeout_ms: 1 }   # fin";
        let (_, cierra) = llaves_del_mapa(en_valor);
        let cierre_esperado = en_valor.rfind('}').unwrap();
        assert_eq!(cierra, Some(cierre_esperado), "el de fuera, no el de dentro de las comillas");
        assert!(en_valor[cierre_esperado + 1..].trim_start().starts_with('#'), "y la cola es el comentario");
        // Double quotes, the other style that appears in the real file.
        let dobles = "{ url: \"a}b\", timeout_ms: 1 }";
        assert_eq!(llaves_del_mapa(dobles).1, Some(dobles.rfind('}').unwrap()));
        // And the comment contributes no closing: if the brace were only in the comment, the map is
        // left unclosed and this must say so, so `parchear` refuses instead of swallowing the comment,
        // and does not get tangled up with the comment's own apostrophe.
        assert_eq!(llaves_del_mapa("{ url: 'x'   # cierra con }").1, None);
    }

    /// A map **inside** the map cannot be rewritten: it is refused, instead of writing a half-done
    /// line with what was taken to be the closing.
    #[test]
    fn parchear_se_niega_con_un_mapa_anidado_dentro_del_mapa() {
        let anidado = CONFIG.replace("'http://127.0.0.1:8009'", "{ interno: 1 }");
        let cambios = [Cambio { backend: "kev".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(&anidado, &cambios).unwrap_err();
        assert!(error.contains("kev") && error.contains("mapa"), "ha de nombrar el caso: {error}");
        assert!(anidado.contains("{ interno: 1 }"), "y el original no se toca");
    }

    /// An **alias** (`laya: *base`) declares no inline map and is not patched, but the reason must
    /// name what is there: saying "it is in block form" about an alias sends one to look in the wrong place.
    #[test]
    fn parchear_se_niega_con_un_alias_y_lo_dice() {
        let con_alias = CONFIG.replace("laya: { url: 'http://127.0.0.1:8021', timeout_ms: 120000 }", "laya: *base");
        let cambios = [Cambio { backend: "laya".into(), clave: "timeout_ms".into(), valor: "60000".into() }];
        let error = parchear(&con_alias, &cambios).unwrap_err();
        assert!(error.contains("laya"), "ha de decir de qué entrada habla: {error}");
        assert!(error.contains("*base"), "y ha de enseñar lo que se ha encontrado: {error}");
        assert!(error.contains("alias"), "nombrando el caso, no «en bloque»: {error}");
    }

    /// A **merge key in front of** the map is refused (rebuilding the line would erase it), whereas
    /// **inside** the map it is kept as is, because it is just another value. Both halves, so that
    /// the doc-comment does not say more than the code does.
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
