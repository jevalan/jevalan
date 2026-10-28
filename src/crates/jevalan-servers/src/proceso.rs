//! Arrancar, sondear y parar servidores locales. Sin UI: la pantalla solo pinta lo que sale de aquí.

use crate::registro::{Servidor, dir_pids};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

/// Lo que se puede decir de un servidor mirando su `.pid` y su puerto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Estado {
    /// El puerto contesta **y** el proceso es nuestro.
    Listo,
    /// Hay un proceso nuestro vivo, pero el puerto aún no contesta: está cargando.
    Cargando,
    /// Ni puerto ni proceso: se cayó, o nunca llegó a arrancar.
    Caido,
    /// El puerto lo ocupa otro: se dice **con su pid**, que es lo que pide la spec (§7).
    ///
    /// El `pid` viaja en 0 cuando el sistema no lo quiere decir —`netstat` ausente, o su salida en
    /// un idioma que no se entiende—: no se finge un pid.
    PuertoAjeno { pid: u32 },
    /// No hay `.pid` y el puerto está libre.
    SinArrancar,
}

/// ¿Contesta alguien en `port`? Se sondea la vuelta local, con un plazo corto: no se toca la red.
pub fn puerto_contesta(port: u16) -> bool {
    // 127.0.0.1 y no `localhost`: el sondeo ha de ser local y no depender de la resolución de nombres.
    let dir = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&dir, Duration::from_millis(300)).is_ok()
}

/// ¿Existe el proceso `pid`? 0 no es un pid válido, y los enormes no existen.
pub fn pid_vivo(pid: u32) -> bool {
    if pid == 0 || pid == u32::MAX {
        return false;
    }
    #[cfg(windows)]
    {
        // `tasklist` es la vía sin dependencias nuevas; se filtra en Rust, no en el shell.
        let salida = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output();
        match salida {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")),
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// La ruta del `.pid` de un servidor nombrado.
fn ruta_pid(nombre: &str) -> Option<PathBuf> {
    dir_pids().map(|d| d.join(format!("{nombre}.pid")))
}

/// El pid anotado para `nombre`, si lo hay y el fichero se entiende.
///
/// Un fichero que diga `0` se lee como si no hubiera pid: el 0 no es un pid utilizable —[`pid_vivo`]
/// lo rechaza de entrada, y [`escribir_pid`] nunca lo escribe—, y distinguir «cero» de «ilegible»
/// obligaría a cada llamante a filtrarlo otra vez. La consecuencia, dicha: un `.pid` con 0 se
/// comporta como un `.pid` que no está.
pub fn pid_de(nombre: &str) -> Option<u32> {
    let texto = std::fs::read_to_string(ruta_pid(nombre)?).ok()?;
    texto.trim().parse().ok().filter(|p| *p != 0)
}

/// Anota el pid de un servidor.
pub fn escribir_pid(nombre: &str, pid: u32) -> Result<(), String> {
    let ruta = ruta_pid(nombre).ok_or("sin directorio de datos para los `.pid`")?;
    if let Some(dir) = ruta.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("no se puede crear {}: {e}", dir.display()))?;
    }
    std::fs::write(&ruta, pid.to_string()).map_err(|e| format!("no se puede escribir {}: {e}", ruta.display()))
}

/// Borra el `.pid` de un servidor; si ya no estaba, nada ocurre.
///
/// **No lo llama [`estado`]**, que no escribe: un `.pid` caduco se limpia cuando alguien decide
/// —arrancar de nuevo, parar (tarea 3)—, no como efecto colateral de mirar. Si [`estado`] lo borrara,
/// dos llamadas seguidas dirían cosas distintas, y quien tenga que decidir no podría fiarse de la
/// segunda.
pub fn borrar_pid(nombre: &str) {
    if let Some(ruta) = ruta_pid(nombre) {
        let _ = std::fs::remove_file(ruta);
    }
}

/// El pid que `netstat` atribuye a quien escucha en `port`, leído de su salida.
///
/// Una línea de escucha es `  TCP    127.0.0.1:8009    0.0.0.0:0    LISTENING    1234`: cinco campos,
/// el cuarto `LISTENING` y el quinto el pid. Se separa de [`proceso_de_puerto`] para poder probar el
/// parseo sin depender de que `netstat` exista o conteste: lo que puede faltar es el programa, no el
/// formato. El cotejo del puerto es por el número entero —`ends_with(":8009")`— para que `:8021` no
/// valga por `:21`, y vale igual para una dirección entre corchetes (`[::]:8009`).
///
/// Solo lo llama la rama de Windows de [`proceso_de_puerto`], así que en Linux y macOS quedaría sin
/// usar y `clippy --all-targets -- -D warnings` se caería por código muerto: de ahí el `allow`, que
/// no tapa un descuido sino una consecuencia del `cfg`. El día que la otra rama aprenda `ss`/`lsof`,
/// lo alimentará esta misma función, sin tocar el `allow`.
#[allow(dead_code)]
fn pid_en_netstat(texto: &str, port: u16) -> Option<u32> {
    let sufijo = format!(":{port}");
    for linea in texto.lines() {
        let campos: Vec<&str> = linea.split_whitespace().collect();
        if campos.len() >= 5
            && campos[3] == "LISTENING"
            && campos[1].ends_with(&sufijo)
            && let Ok(pid) = campos[4].parse()
        {
            return Some(pid);
        }
    }
    None
}

/// El pid de quien escucha en `port`, si el sistema lo dice. Se pregunta solo cuando hace falta —un
/// puerto ocupado que no es nuestro—, porque `netstat` tarda cientos de milisegundos.
///
/// Cuando no se puede saber —sin `netstat`, o con una salida que no se entiende— devuelve `None`, y
/// quien llame decidirá si eso es un 0 o un silencio. No se adivina un pid.
pub fn proceso_de_puerto(port: u16) -> Option<u32> {
    #[cfg(windows)]
    {
        let salida = std::process::Command::new("netstat").args(["-ano", "-p", "TCP"]).output().ok()?;
        pid_en_netstat(&String::from_utf8_lossy(&salida.stdout), port)
    }
    #[cfg(not(windows))]
    {
        let _ = port;
        None // `ss`/`lsof` cuando haga falta: hoy solo se soporta Windows para los servidores locales.
    }
}

/// El estado de un servidor. **No escribe nada**: dos llamadas seguidas dan lo mismo, que es lo que
/// permite a quien lo consume (arrancar, la pantalla) decidir con un solo resultado.
///
/// El puerto es la señal que no engaña; el `.pid` dice si el proceso es nuestro. Sin `.pid` propio,
/// un puerto que contesta es de otro —el `ollama` de turno, un vecino, un `serve.bat` lanzado a mano
/// del que nadie anotó el pid— y se dice con su pid: darlo por `Listo` sería atribuirnos un proceso
/// que no arrancamos, que es justo lo que la spec §7 prohíbe.
pub fn estado(s: &Servidor) -> Estado {
    let nuestro = pid_de(&s.nombre);
    let vivo = nuestro.is_some_and(pid_vivo);
    if puerto_contesta(s.port) {
        if vivo {
            return Estado::Listo;
        }
        // Contesta y no es nuestro: se dice **con el pid de quien lo ocupa**, que es lo que pide la
        // spec (§7). `0` cuando el sistema no lo quiere decir: no se finge un pid.
        return Estado::PuertoAjeno { pid: proceso_de_puerto(s.port).unwrap_or(0) };
    }
    match nuestro {
        // `vivo` ya está medido arriba: volver a preguntar por el mismo proceso costaría otro
        // `tasklist` —cientos de milisegundos— y podría contestar otra cosa a mitad de la misma
        // llamada, que es exactamente lo que este rediseño quita de en medio.
        Some(_) if vivo => Estado::Cargando,
        Some(_) => Estado::Caido,
        None => Estado::SinArrancar,
    }
}

/// Lanza el servidor sin consola propia y con su salida añadida al `log` del registro.
///
/// Antes de lanzar nada se pregunta **de quién es el puerto**, no solo si contesta: desde que
/// [`Estado::Listo`] exige `.pid` nuestro y vivo (tarea 2), un puerto que contesta sin `.pid` nuestro
/// es de otro. Con uno ajeno no se lanza —lanzar sería pelearse por el puerto— ni se dice «ya
/// estaba» —sería mentir sobre un proceso que no arrancamos—: se dice de quién es y se para. Y un
/// servidor que ya está cargando tampoco se relanza: con Kev, que tarda uno o dos minutos, esa
/// ventana es la más ancha de todas, y un segundo proceso se pelearía con el primero por el puerto y
/// por la memoria de la máquina.
///
/// Del desacoplado, lo que se promete y lo que no: el proceso se crea **sin consola** —con
/// `CREATE_NO_WINDOW` y `DETACHED_PROCESS`—, de modo que no hereda la de jevalan ni saca una ventana
/// propia, y su salida va al `log` y no a una pantalla. Que además sobreviva al cierre de jevalan
/// **no lo ha medido ninguna prueba** —en Windows un hijo no muere con su padre, así que debería, pero
/// es una propiedad del sistema, no de estas banderas— y por eso no se afirma.
///
/// La segunda bandera no es adorno, aunque la documentación de `CreateProcess` diga que se ignora
/// junto a `DETACHED_PROCESS`: con solo `DETACHED_PROCESS`, cada `arrancar` abría una ventana de
/// consola. Se descubrió porque el banco de pruebas hacía parpadear una por cada `ping` que lanzaba.
///
/// Y el aviso del `.pid`, que es el precio de creérselo: si el sistema reutilizó el pid anotado para
/// otro proceso, un servidor que ya murió se lee como [`Estado::Cargando`] y esto devuelve `Ok` sin
/// lanzar nada —un no-op silencioso—. Su espejo está en [`parar`], donde el mismo error mata a ese
/// poseedor inocente del pid.
pub fn arrancar(s: &Servidor) -> Result<(), String> {
    match estado(s) {
        // Ya listo, o ya cargando: en los dos casos hay un proceso nuestro vivo y no hay nada que
        // lanzar.
        Estado::Listo | Estado::Cargando => return Ok(()),
        // Ajeno: se dice de quién, cuando el sistema lo sabe —la spec §7 pide el pid—, y no se toca.
        Estado::PuertoAjeno { pid } => {
            let dueno = if pid == 0 { String::new() } else { format!(" (pid {pid})") };
            return Err(format!("el puerto {} lo ocupa un proceso ajeno{dueno}: no se lanza nada encima", s.port));
        }
        Estado::Caido | Estado::SinArrancar => {}
    }
    if !s.cwd.is_dir() {
        return Err(format!("el cwd del servidor «{}» no existe: {}", s.nombre, s.cwd.display()));
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&s.log)
        .map_err(|e| format!("no se puede abrir {}: {e}", s.log.display()))?;
    let log_err = log.try_clone().map_err(|e| format!("no se puede duplicar el log: {e}"))?;

    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/c").arg(&s.command).current_dir(&s.cwd).stdout(log).stderr(log_err);
    for (k, v) in &s.env {
        cmd.env(k, v);
    }
    // Sin consola propia: el hijo no hereda la de jevalan, y su salida va al `log`.
    //
    // Van **las dos** banderas. `CREATE_NO_WINDOW` no es ruido aunque el manual diga que se ignora
    // junto a `DETACHED_PROCESS`: medido en esta máquina, con solo `DETACHED_PROCESS` cada `arrancar`
    // saca una ventana de consola —y las pruebas llaman a `arrancar` con comandos `ping`, así que una
    // corrida deja media docena de parpadeos—. Vale lo que se observa, no lo que promete el manual.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    let hijo = cmd.spawn().map_err(|e| format!("no se puede lanzar «{}»: {e}", s.command))?;
    escribir_pid(&s.nombre, hijo.id())
}

/// Sondea el puerto hasta `s.ready_ms`, informando del estado. Es lo que hace visible que Kev tarda
/// uno o dos minutos: devuelve el estado, no un timeout ciego.
///
/// Cuando el puerto llega a contestar no se afirma `Listo` sin más —el plan lo hacía, y era la puerta
/// de atrás del mismo error que la tarea 2 quitó de [`estado`]—: se devuelve el [`Estado`] entero,
/// que dirá `Listo` si el proceso es nuestro y [`Estado::PuertoAjeno`] si el puerto se lo ha quedado
/// otro mientras esperábamos.
pub fn esperar(s: &Servidor) -> Estado {
    let limite = std::time::Instant::now() + Duration::from_millis(s.ready_ms);
    loop {
        if puerto_contesta(s.port) {
            return estado(s);
        }
        // Si el proceso ya no está, no hay nada que esperar.
        match pid_de(&s.nombre) {
            Some(pid) if !pid_vivo(pid) => {
                borrar_pid(&s.nombre);
                return Estado::Caido;
            }
            _ => {}
        }
        if std::time::Instant::now() >= limite {
            return estado(s);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Mata el proceso anotado en el `.pid`, **con su árbol** (`taskkill /T`), y no toca lo que no
/// arrancamos. Devuelve `false` si ya no estaba.
///
/// Al `.pid` vivo **no se le compara con quien sostiene el socket**, y es deliberado: un `serve.bat`
/// —el de Kev, el de Laya— deja el `.pid` apuntando al `cmd.exe` que lo lanzó, y el socket lo
/// sostiene **su hijo**, el python del venv. Compararlos convertía en «ajeno» al servidor que
/// nosotros mismos habíamos arrancado, mientras [`estado`] lo daba por `Listo` en el mismo instante:
/// `parar` se negaba y el servidor se quedaba vivo. Matar el proceso anotado y su árbol es lo que se
/// pidió, y es lo único que funciona con un `.bat`.
///
/// Lo que cede, dicho: si el sistema reutilizó el pid anotado para otro proceso, se mata a ese otro
/// —el `.pid` es la única prueba de propiedad que hay, y se le cree—. Ajeno es, por tanto, lo que
/// **no** tiene un `.pid` nuestro vivo detrás.
pub fn parar(s: &Servidor) -> Result<bool, String> {
    let Some(pid) = pid_de(&s.nombre) else {
        // Sin `.pid` no hay nada nuestro que matar: si el puerto contesta, es de otro.
        if puerto_contesta(s.port) {
            return Err(format!("el puerto {} lo ocupa un proceso ajeno: no se toca", s.port));
        }
        return Ok(false);
    };
    if !pid_vivo(pid) {
        // Caduco: se retira aquí, porque `estado` es puro y no escribe (tarea 2), y **antes** de decir
        // nada del puerto: es el fichero de un proceso que ya no existe, y nadie más va a retirarlo.
        // Si además el puerto lo ocupa otro, se dice: es más útil que un «ok» sobre un fichero que ya
        // no vale.
        let ocupado = puerto_contesta(s.port);
        borrar_pid(&s.nombre);
        if ocupado {
            return Err(format!("el puerto {} lo ocupa un proceso ajeno: no se toca", s.port));
        }
        return Ok(false);
    }
    // Vivo y nuestro: se mata **el árbol**, y con él los hijos que sostengan el puerto.
    matar(pid)?;
    borrar_pid(&s.nombre);
    Ok(true)
}

/// Mata un proceso por su pid, con `taskkill` en Windows y `kill` en el resto.
fn matar(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    let salida = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
    #[cfg(not(windows))]
    let salida = std::process::Command::new("kill").args(["-9", &pid.to_string()]).output();
    match salida {
        // `taskkill` devuelve 128 si el proceso ya no estaba: eso no es un fallo para nosotros.
        Ok(o) if o.status.success() || o.status.code() == Some(128) => Ok(()),
        Ok(o) => Err(format!("no se pudo matar el proceso {pid}: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("no se pudo matar el proceso {pid}: {e}")),
    }
}

/// Las últimas `lineas` del log del servidor; vacío si no hay log todavía.
pub fn cola_log(s: &Servidor, lineas: usize) -> Vec<String> {
    let Ok(texto) = std::fs::read_to_string(&s.log) else { return Vec::new() };
    let todas: Vec<&str> = texto.lines().collect();
    todas[todas.len().saturating_sub(lineas)..].iter().map(|l| (*l).to_string()).collect()
}

#[cfg(test)]
mod tests {
    //! Estas pruebas **no son herméticas**, y conviene saberlo: escriben `.pid` de verdad en el
    //! directorio de datos de jevalan (`%LOCALAPPDATA%\jevalan\servers`, véase `registro::dir_pids`),
    //! lanzan procesos hijos y abren puertos de verdad. Por eso cada prueba usa su propio nombre de
    //! servidor —corren en paralelo y comparten el directorio—, y por eso un pánico a mitad puede
    //! dejar un `.pid` huérfano y un hijo vivo: es el precio de probar lo que se toca.
    //!
    //! Y son **Windows de hecho**, aunque el crate compile en cualquier parte: el andamio lanza un
    //! `ping -n` y un `.bat` que abre un `powershell`, y llama a `tasklist`, a `netstat` y a
    //! `taskkill`; todo servidor de prueba pasa, además, por el `cmd /c` que pone [`arrancar`] —el
    //! `ping` del andamio es el único hijo que no va por ahí—. En Linux o macOS compilarían —el
    //! `cfg` de [`Hijo`] y el de `matar` existen—, pero no pasarían: allí `proceso_de_puerto`
    //! devuelve `None` por diseño y no hay `cmd /c` que valga. Soportar esos sistemas es un trabajo
    //! aparte, no un descuido que estas pruebas puedan tapar.

    use super::*;
    use std::net::TcpListener;
    use std::path::PathBuf;

    /// Un puerto en el que nadie escucha y que **nadie nos puede quitar**: uno por debajo del rango
    /// dinámico de TCP, que aquí empieza en 1024 (`netsh int ipv4 show dynamicport tcp`: inicio 1024,
    /// 64511 puertos) y llega hasta el 65535. El sistema reparte los efímeros de ahí, en orden, así
    /// que un puerto alto —el `47_313` que hubo aquí— puede serle asignado a otra prueba a mitad de
    /// la corrida; uno por debajo de 1024, nunca. En Unix hay un motivo de más: sin privilegios no
    /// se puede ocupar.
    ///
    /// No se usa un `TcpListener` propio «vinculado pero sin escuchar», que sería lo ideal, porque en
    /// Rust **no existe**: `TcpListener::bind` llama a `listen` por dentro y contesta —medido; ver el
    /// informe—. Un puerto sordo ha de ser, por fuerza, un número que nadie ocupe.
    ///
    /// Lo que este número sí puede encontrarse: un servicio que ya estuviera escuchando aquí. Si eso
    /// pasa, la prueba cae **siempre y en el mismo sitio**, no a veces; entonces se elige otro puerto
    /// por debajo de 1024 y se deja dicho por qué.
    const PUERTO_SORDO: u16 = 471;

    /// Un servidor verosímil y completo, con el nombre que le dé la prueba que lo usa.
    fn servidor(nombre: &str, port: u16) -> Servidor {
        Servidor {
            nombre: nombre.to_string(),
            port,
            command: "cmd /c rem nada".into(),
            cwd: PathBuf::from("."),
            env: Default::default(),
            ready_ms: 1_000,
            log: PathBuf::from("prueba.log"),
        }
    }

    /// Un nombre que solo usa esta prueba, para no pisar el `.pid` de otra que corra a la vez.
    fn nombre(etiqueta: &str) -> String {
        format!("prueba-{etiqueta}-{}", std::process::id())
    }

    /// Un hijo de verdad, que vive unos segundos y muere al soltarse.
    ///
    /// Existe porque usar `std::process::id()` para probar los `.pid` no distingue nada: el pid
    /// propio está vivo por definición, así que una implementación que se saltara el fichero y
    /// asumiera el pid del test pasaría igual. Un hijo ajeno, al que además se puede matar, sí
    /// distingue. Y muere en `Drop` para que un pánico no deje un `ping` huérfano.
    ///
    /// Se lanza **sin consola** (`CREATE_NO_WINDOW`), que es lo mismo que hace [`arrancar`] con los
    /// servidores de verdad: un hijo de consola lanzado desde un proceso que no tiene la suya —el
    /// caso de esta casa— se lleva una ventana nueva, y la suite abre y cierra media docena por
    /// corrida. La bandera no cambia nada de lo que aquí se prueba: el hijo sigue vivo, y `pid_vivo`
    /// y `kill` lo ven igual.
    struct Hijo(std::process::Child);

    impl Hijo {
        fn new() -> Self {
            #[cfg(windows)]
            let hijo = {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                std::process::Command::new("ping")
                    .args(["-n", "30", "127.0.0.1"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                    .unwrap()
            };
            #[cfg(not(windows))]
            let hijo = std::process::Command::new("sleep").arg("30").spawn().unwrap();
            Self(hijo)
        }

        fn pid(&self) -> u32 {
            self.0.id()
        }

        fn matar(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    impl Drop for Hijo {
        fn drop(&mut self) {
            self.matar();
        }
    }

    /// Un servidor de verdad y efímero: un `cmd` que se queda vivo hasta que lo matan.
    ///
    /// Su `command` es un `ping`, que **no abre puertos**: así la prueba decide con su propio
    /// `TcpListener` si el puerto contesta, sin depender de lo que haga el hijo. Va **sin** `cmd /c`
    /// delante —que es quien lo pone, [`arrancar`]— porque es como lo declara `servers.yaml` de
    /// verdad: llevarlo aquí dejaba `cmd /c cmd /c ping`, un `cmd` de más por cada servidor de las
    /// pruebas. El `.pid` que se anota es el mismo en los dos casos: el del `cmd` exterior.
    fn servidor_que_escucha(nombre: &str, port: u16) -> Servidor {
        // `ping` mantiene el proceso vivo sin abrir puertos; la sonda se prueba con un listener aparte.
        Servidor {
            nombre: nombre.into(),
            port,
            command: "ping -n 30 127.0.0.1".into(),
            cwd: std::env::temp_dir(),
            env: Default::default(),
            ready_ms: 5_000,
            log: std::env::temp_dir().join(format!("{nombre}.log")),
        }
    }

    /// Un puerto en el que nadie escucha. Es [`PUERTO_SORDO`] —por debajo del rango dinámico, y ya
    /// razonado en las pruebas de arriba—: el plan lo llamaba `puerto_libre()` sin definirlo, y no
    /// hay motivo para inventar otro número que el que la tarea 2 ya eligió por buenas razones.
    fn puerto_libre() -> u16 {
        PUERTO_SORDO
    }

    /// Un `.bat` que lanza un hijo que **sostiene el puerto**, y deja dicho en un fichero cuál abrió.
    ///
    /// Es la forma de un `serve.bat` de verdad —el de Kev, el de Laya—, sin su venv: `arrancar` anota
    /// el pid del `cmd.exe`, y el socket lo abre **su hijo**. El banco de pruebas no tenía este caso
    /// —su único hijo, `cmd /c ping`, no abre puertos—, y por eso la rama de `parar` que decidía si
    /// el puerto era ajeno llegó a producción sin ejercitarse (Crítico C1 de la revisión 1).
    ///
    /// El puerto lo elige el hijo, no la prueba: reservarlo aquí para pasárselo sería quedárnoslo
    /// nosotros, y soltarlo para que él lo tomara sería una carrera con las demás pruebas. El hijo
    /// pide el 0 —«el que haya»— y escribe el elegido en un fichero, que es lo único que la prueba no
    /// puede saber de antemano.
    struct EscuchaDeVerdad {
        dir: PathBuf,
        fichero_puerto: PathBuf,
    }

    impl EscuchaDeVerdad {
        fn nueva(etiqueta: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("escucha-{etiqueta}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let fichero_puerto = dir.join("puerto.txt");
            // `cmd` no sabe abrir un socket, así que el hijo que escucha ha de ser otro; PowerShell
            // está en toda máquina Windows y no añade dependencias al crate. La ruta del fichero va
            // entre comillas simples —para que `cmd` no la toque— y el `-Command` entero entre dobles.
            let linea = format!(
                "powershell -NoProfile -ExecutionPolicy Bypass -Command \"$l = New-Object \
                 System.Net.Sockets.TcpListener([System.Net.IPAddress]::Loopback, 0); $l.Start(); \
                 [System.IO.File]::WriteAllText('{}', [string]$l.LocalEndpoint.Port); \
                 Start-Sleep -Seconds 45\"",
                fichero_puerto.display()
            );
            std::fs::write(dir.join("escucha.bat"), format!("@echo off\r\n{linea}\r\n")).unwrap();
            Self { dir, fichero_puerto }
        }

        /// El servidor del registro apuntando a ese `.bat`.
        ///
        /// `port` es el que [`arrancar`] mira **antes** de lanzar —ha de estar libre para que lance—;
        /// el de verdad se sabe después, y quien llame ha de corregirlo.
        fn servidor(&self, nombre: &str, port: u16) -> Servidor {
            Servidor {
                nombre: nombre.into(),
                port,
                command: self.dir.join("escucha.bat").display().to_string(),
                cwd: self.dir.clone(),
                env: Default::default(),
                ready_ms: 15_000,
                log: self.dir.join("escucha.log"),
            }
        }

        /// El puerto que el hijo dice haber abierto, o `None` si no lo dice a tiempo.
        fn puerto_abierto(&self) -> Option<u16> {
            let limite = std::time::Instant::now() + Duration::from_secs(30);
            while std::time::Instant::now() < limite {
                // El fichero puede pillarse a medio escribir; el reintento lo cubre.
                if let Ok(texto) = std::fs::read_to_string(&self.fichero_puerto)
                    && let Ok(puerto) = texto.trim().parse()
                {
                    return Some(puerto);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            None
        }
    }

    impl Drop for EscuchaDeVerdad {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Un `cmd` lanzado por [`arrancar`], que ha de morir pase lo que pase.
    ///
    /// Existe porque una afirmación que caiga antes de `parar` dejaría un `ping` vivo treinta
    /// segundos **y** su `.pid` en el directorio de datos: el mismo daño que una prueba en rojo,
    /// pero silencioso. `parar` lo mata por la vía normal; esto es la red por si no se llega.
    ///
    /// Mira **dos** sitios, y el segundo es el que importa cuando algo falla: lo que diga el `.pid`
    /// del disco —puede apuntar a un segundo lanzamiento que la prueba no llegó a ver— y el pid que
    /// la prueba conoció. Con `pid: None` sirve también para las pruebas donde no hay un proceso
    /// propio conocido de antemano y solo queda un fichero que retirar (M4 de la revisión 1).
    struct Guardia {
        nombre: String,
        pid: Option<u32>,
    }

    impl Guardia {
        /// La guardia del caso corriente: se sabe de antemano qué proceso se lanzó.
        fn nueva(nombre: &str, pid: u32) -> Self {
            Self { nombre: nombre.to_string(), pid: Some(pid) }
        }

        /// La guardia en la que solo el fichero sabe qué hay que matar —y qué hay que retirar—.
        fn del_fichero(nombre: &str) -> Self {
            Self { nombre: nombre.to_string(), pid: None }
        }
    }

    impl Drop for Guardia {
        fn drop(&mut self) {
            // Se mira también lo que diga el fichero: si una prueba en rojo llegó a lanzar un
            // segundo proceso, `arrancar` anotó **su** pid, y ese es el que quedaría huérfano.
            if let Some(anotado) = pid_de(&self.nombre) {
                let _ = matar(anotado);
            }
            if let Some(pid) = self.pid {
                let _ = matar(pid);
            }
            borrar_pid(&self.nombre);
        }
    }

    #[test]
    fn la_sonda_distingue_un_puerto_escuchando_de_uno_cerrado() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let vivo = l.local_addr().unwrap().port();
        assert!(puerto_contesta(vivo), "un listener ha de contestar");
        assert!(
            !puerto_contesta(PUERTO_SORDO),
            "algo escucha en el {PUERTO_SORDO} en esta máquina: pon otro puerto por debajo de 1024 en \
             `PUERTO_SORDO` y dilo, porque no es la sonda quien falla"
        );
    }

    #[test]
    fn un_proceso_vivo_se_reconoce_y_uno_muerto_no() {
        let mut hijo = Hijo::new();
        let pid = hijo.pid();
        assert!(pid_vivo(pid), "el hijo está vivo");
        hijo.matar();
        assert!(!pid_vivo(pid), "y una vez muerto, ya no");
        assert!(pid_vivo(std::process::id()), "este mismo proceso está vivo");
        // Un pid que no puede existir en Windows: se rechaza el 0 y los enormes.
        assert!(!pid_vivo(0));
        assert!(!pid_vivo(u32::MAX));
    }

    #[test]
    fn el_estado_cuenta_las_cuatro_situaciones() {
        let nombre = nombre("cuatro");

        // Sin `.pid` y sin nadie en el puerto: no se ha arrancado.
        assert_eq!(estado(&servidor(&nombre, PUERTO_SORDO)), Estado::SinArrancar);

        // Con un hijo de verdad vivo y el puerto callado: está cargando. El pid no es el nuestro, así
        // que la prueba no puede pasar por «asumir el pid propio».
        let mut hijo = Hijo::new();
        let pid = hijo.pid();
        escribir_pid(&nombre, pid).unwrap();
        assert_eq!(estado(&servidor(&nombre, PUERTO_SORDO)), Estado::Cargando);

        // Con el puerto contestando **y** el proceso vivo: listo.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        assert_eq!(estado(&servidor(&nombre, l.local_addr().unwrap().port())), Estado::Listo);
        drop(l);

        // Y una vez muerto el hijo: caído, con el `.pid` todavía en su sitio —`estado` no lo borra—.
        hijo.matar();
        assert_eq!(estado(&servidor(&nombre, PUERTO_SORDO)), Estado::Caido);
        assert_eq!(pid_de(&nombre), Some(pid), "`estado` lee y no borra el `.pid` caduco");
        borrar_pid(&nombre);
    }

    #[test]
    fn un_puerto_ocupado_por_un_ajeno_se_senala_con_su_pid() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let puerto = l.local_addr().unwrap().port();
        let nombre = nombre("ajeno");
        let s = servidor(&nombre, puerto);

        // Sin `.pid` nuestro —el caso corriente: `ollama`, un vecino— el puerto no es nuestro, y no se
        // disfraza de `Listo`. El pid ha de ser el que da `netstat`; si `netstat` no contesta, 0,
        // porque no se finge un pid.
        match (estado(&s), proceso_de_puerto(puerto)) {
            (Estado::PuertoAjeno { pid }, Some(esperado)) => {
                assert_eq!(pid, esperado, "el estado ha de llevar el pid que da netstat");
                // Y el listener es de este mismo proceso, así que ese pid ha de ser el nuestro.
                assert_eq!(esperado, std::process::id(), "el listener es de este mismo proceso");
            }
            (Estado::PuertoAjeno { pid }, None) => {
                assert_eq!(pid, 0, "sin netstat no se finge un pid");
                // En voz alta, para que `--nocapture` deje constancia de por qué no se afirmó más.
                eprintln!("netstat no contestó por {puerto}: no se pudo afirmar el pid ajeno");
            }
            (otro, _) => panic!("un puerto ajeno ha de señalarse como tal, y salió {otro:?}"),
        }

        // Un `.pid` caduco no lo cambia: el puerto sigue siendo de otro, y el fichero se queda donde
        // está —limpiarlo es de quien arranque o pare, no de quien mira—.
        escribir_pid(&nombre, u32::MAX).unwrap();
        let primero = estado(&s);
        assert!(matches!(primero, Estado::PuertoAjeno { .. }), "{primero:?}");
        assert_eq!(estado(&s), primero, "`estado` ha de ser idempotente");
        assert_eq!(pid_de(&nombre), Some(u32::MAX), "`estado` no ha de borrar el `.pid`");

        // Y con un `.pid` de un proceso vivo y el puerto contestando, el puerto sí es nuestro.
        let mut hijo = Hijo::new();
        escribir_pid(&nombre, hijo.pid()).unwrap();
        assert_eq!(estado(&s), Estado::Listo);
        hijo.matar();
        borrar_pid(&nombre);
        let _ = l;
    }

    /// El invariante que trae el rediseño: `estado` lee y no escribe, así que dos llamadas seguidas
    /// dan lo mismo en los dos casos que antes cambiaban de tick a tick.
    #[test]
    fn el_estado_dos_veces_seguidas_da_lo_mismo() {
        // Puerto sordo con `.pid` caduco: antes decía `Caido` y a la segunda `SinArrancar`, porque
        // borraba el fichero al detectarlo.
        let caido = nombre("idempotente-caido");
        escribir_pid(&caido, u32::MAX).unwrap();
        let sordo = servidor(&caido, PUERTO_SORDO);
        assert_eq!(estado(&sordo), Estado::Caido);
        assert_eq!(estado(&sordo), Estado::Caido, "la segunda llamada ha de decir lo mismo");
        assert_eq!(pid_de(&caido), Some(u32::MAX), "y el `.pid` caduco sigue donde estaba");
        borrar_pid(&caido);

        // Puerto ocupado con `.pid` caduco: antes decía `PuertoAjeno` y a la segunda `Listo`.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let ajeno_nombre = nombre("idempotente-ajeno");
        escribir_pid(&ajeno_nombre, u32::MAX).unwrap();
        let ajeno = servidor(&ajeno_nombre, l.local_addr().unwrap().port());
        let primero = estado(&ajeno);
        assert!(matches!(primero, Estado::PuertoAjeno { .. }), "{primero:?}");
        assert_eq!(estado(&ajeno), primero, "la segunda llamada ha de decir lo mismo");
        assert_eq!(pid_de(&ajeno_nombre), Some(u32::MAX), "y el `.pid` caduco sigue donde estaba");
        borrar_pid(&ajeno_nombre);
    }

    /// El 0 no es un pid: un fichero que lo diga se lee como si no hubiera pid. Se decide así —y no
    /// distinguiendo «cero» de «ilegible»— porque `pid_vivo(0)` es falso por definición y
    /// `escribir_pid` nunca lo escribe: distinguirlo obligaría a cada llamante a filtrarlo otra vez.
    #[test]
    fn un_pid_cero_en_el_fichero_no_cuenta_como_pid() {
        let nombre = nombre("cero");
        escribir_pid(&nombre, 0).unwrap();
        assert_eq!(pid_de(&nombre), None, "el 0 no es un pid utilizable");
        assert_eq!(estado(&servidor(&nombre, PUERTO_SORDO)), Estado::SinArrancar);

        // Y con el puerto contestando, un `.pid` que dice 0 tampoco nos hace dueños del puerto.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let ocupado = servidor(&nombre, l.local_addr().unwrap().port());
        let visto = estado(&ocupado);
        assert!(matches!(visto, Estado::PuertoAjeno { .. }), "{visto:?}");
        borrar_pid(&nombre);
    }

    /// La salida de `netstat -ano -p TCP` de esta máquina, con los puertos del registro en lugar de
    /// los suyos: la cabecera es literal —en español, y `LISTENING` se queda en inglés— y las líneas
    /// tienen la forma exacta que imprime. La última, entre corchetes, es de TCPv6: `-p TCP` no la
    /// imprime hoy, pero el mismo formato aparece en cuanto alguien cambie de bandera o de máquina.
    const NETSTAT: &str = "\
Conexiones activas

  Proto  Dirección local          Dirección remota        Estado           PID
  TCP    127.0.0.1:8009         0.0.0.0:0              LISTENING       4321
  TCP    127.0.0.1:8021         0.0.0.0:0              LISTENING       8765
  TCP    127.0.0.1:52000        127.0.0.1:443          ESTABLISHED     9999
  TCP    [::]:8009              [::]:0                 LISTENING       4321
";

    #[test]
    fn el_parseo_de_netstat_encuentra_a_quien_escucha() {
        assert_eq!(pid_en_netstat(NETSTAT, 8009), Some(4321));
        assert_eq!(pid_en_netstat(NETSTAT, 8021), Some(8765), "cada puerto, su proceso");
        // Un puerto que solo aparece conectado, no escuchando, no es de nadie que espere ahí.
        assert_eq!(pid_en_netstat(NETSTAT, 52000), None);
        // Y un puerto que no está, tampoco.
        assert_eq!(pid_en_netstat(NETSTAT, 1111), None);
        // El cotejo es por el puerto entero, no por su cola: `:8021` no ha de valer por `:21`.
        assert_eq!(pid_en_netstat(NETSTAT, 21), None);
        assert_eq!(pid_en_netstat(NETSTAT, 800), None);
        // La cabecera y las líneas cortas se saltan sin romper nada.
        assert_eq!(pid_en_netstat("Proto  Dirección local\n\n", 8009), None);
        assert_eq!(pid_en_netstat("", 8009), None);
    }

    /// ¿`netstat` existe y contesta algo con sentido? Es la única forma de distinguir un `None` de
    /// [`proceso_de_puerto`] que signifique «aquí no hay `netstat`» de uno que signifique «lo hay y
    /// no supe leerlo», que sí sería un fallo nuestro.
    fn netstat_disponible() -> bool {
        std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("LISTENING"))
    }

    /// `proceso_de_puerto` de verdad: habla con `netstat`, que tarda cientos de milisegundos y puede
    /// no estar en el PATH del proceso hijo. Por eso la prueba **no exige que `netstat` esté**: si no
    /// está, lo dice y se conforma. Pero si está —y aquí está—, un `None` sobre un listener propio ya
    /// no es «no se pudo»: es que no supimos leer su salida, y entonces la prueba ha de caer. Un
    /// listener propio es determinista: el pid ha de ser el nuestro.
    #[test]
    fn el_pid_de_un_puerto_lo_dice_netstat_si_esta() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let puerto = l.local_addr().unwrap().port();
        match (proceso_de_puerto(puerto), netstat_disponible()) {
            (Some(pid), _) => assert_eq!(pid, std::process::id(), "el listener es de este mismo proceso"),
            (None, false) => eprintln!("netstat no está en el PATH: no se pudo afirmar el pid de {puerto}"),
            (None, true) => panic!("netstat contesta, pero no supo decir el pid de {puerto}"),
        }
        // Y en un puerto sordo no hay nadie a quien atribuirlo.
        assert_eq!(proceso_de_puerto(PUERTO_SORDO), None);
    }

    // ------------------------------------------------------------------------------------------
    // Arrancar, esperar y parar (tarea 3)
    // ------------------------------------------------------------------------------------------

    /// El plan decía «si el puerto ya contesta, no lanzo nada», y eso dejó de bastar cuando la tarea
    /// 2 hizo que un puerto sin `.pid` nuestro fuera [`Estado::PuertoAjeno`]: hay que mirar **de
    /// quién** es. Si es de otro, no se lanza —lanzar sería pelearse por el puerto— y no se dice «ya
    /// estaba» —sería mentir sobre un proceso que no arrancamos—. Si es nuestro, ya estaba de verdad
    /// y no hay nada que hacer. Las dos mitades se prueban aquí porque el mismo puerto que contesta
    /// ha de dar una respuesta distinta según de quién sea.
    #[test]
    fn arrancar_no_lanza_nada_si_el_puerto_ya_contesta() {
        // Ajeno: el listener de esta misma prueba, sin `.pid` que lo reclame.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let ajeno = format!("ya-vivo-ajeno-{}", std::process::id());
        let _guardia_ajeno = Guardia::del_fichero(&ajeno);
        let s = Servidor { nombre: ajeno.clone(), ..servidor_que_escucha(&ajeno, l.local_addr().unwrap().port()) };
        let r = arrancar(&s);
        assert!(r.is_err(), "sobre un puerto ajeno no se lanza: {r:?}");
        assert!(r.unwrap_err().contains("ajeno"), "y el motivo ha de ser que el puerto es de otro");
        assert!(pid_de(&ajeno).is_none(), "no se anota un pid que no hemos arrancado");

        // Nuestro: `.pid` vivo y el puerto contestando. No se toca el `.pid`, que es la prueba de
        // que no se ha lanzado nada: un lanzamiento lo habría sobrescrito con el suyo.
        let nuestro = format!("ya-vivo-nuestro-{}", std::process::id());
        let mut hijo = Hijo::new();
        escribir_pid(&nuestro, hijo.pid()).unwrap();
        // Si una afirmación cae, ni el `.pid` ni un `ping` que `arrancar` hubiera lanzado de más han
        // de quedarse: la guardia mira el fichero, que es quien sabría de ese segundo proceso.
        let _guardia_nuestro = Guardia::del_fichero(&nuestro);
        let s = Servidor { nombre: nuestro.clone(), ..servidor_que_escucha(&nuestro, l.local_addr().unwrap().port()) };
        assert!(arrancar(&s).is_ok(), "un servidor ya listo no se vuelve a lanzar");
        assert_eq!(pid_de(&nuestro), Some(hijo.pid()), "y el pid anotado sigue siendo el suyo");
        hijo.matar();
        borrar_pid(&nuestro);
        let _ = l;
    }

    /// Un `cwd` que no existe se rechaza **antes** de lanzar nada: el `cmd` moriría al no poder
    /// situarse en él, y el motivo que llegaría a la pantalla sería el de `spawn`, no el de verdad.
    #[test]
    fn arrancar_rechaza_un_cwd_que_no_existe() {
        let nombre = format!("sin-cwd-{}", std::process::id());
        let s = Servidor {
            cwd: PathBuf::from("no-existe-esta-ruta-jevalan"),
            ..servidor_que_escucha(&nombre, puerto_libre())
        };
        let r = arrancar(&s);
        assert!(r.is_err(), "sin directorio de trabajo no se lanza: {r:?}");
        assert!(r.unwrap_err().contains("cwd"), "y el motivo ha de ser ese, no el del `spawn`");
        assert!(pid_de(&nombre).is_none(), "y no queda nada anotado");
        let _ = std::fs::remove_file(std::env::temp_dir().join(format!("{nombre}.log")));
    }

    #[test]
    fn arrancar_anota_el_pid_y_parar_lo_mata() {
        let nombre = format!("arranca-{}", std::process::id());
        let s = servidor_que_escucha(&nombre, puerto_libre());
        arrancar(&s).expect("se lanza");
        let pid = pid_de(&nombre).expect("queda anotado");
        // De aquí en adelante el hijo ha de morir pase lo que pase; `parar` lo hará antes, por la vía
        // normal, y el `Drop` de la guardia es la red por si una afirmación cae antes.
        let _guardia = Guardia::nueva(&nombre, pid);
        assert!(pid_vivo(pid), "y el proceso vive");

        assert!(parar(&s).unwrap(), "y se puede parar");
        assert!(pid_de(&nombre).is_none(), "el `.pid` se borra al parar");
        // Dar un momento al sistema para recolectar el proceso. El plan dormía 500 ms y miraba una
        // sola vez; con la máquina cargada —otra sesión compilando a la vez— ese único vistazo falla
        // y la prueba se cae por el reloj, no por el código. Se concede un plazo y se sondea dentro
        // de él: la afirmación es la misma —el proceso ha de morir—, pero deja de ser una carrera.
        let limite = std::time::Instant::now() + Duration::from_secs(5);
        while pid_vivo(pid) && std::time::Instant::now() < limite {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!pid_vivo(pid), "y el proceso ya no está");
    }

    /// La otra mitad del Review Focus 2: «arrancar dos veces seguidas». Con un servidor que tarda
    /// —Kev tarda uno o dos minutos— el puerto está callado casi todo ese rato, así que el guardián
    /// del puerto no basta: la segunda pulsación de «arrancar» ha de ver que **ya hay un proceso
    /// nuestro vivo** y no lanzar otro que se pelee por el puerto y por la memoria de la máquina.
    #[test]
    fn arrancar_dos_veces_seguidas_no_lanza_un_segundo_proceso() {
        let nombre = format!("dos-veces-{}", std::process::id());
        let s = servidor_que_escucha(&nombre, puerto_libre());
        arrancar(&s).expect("se lanza");
        let pid = pid_de(&nombre).expect("queda anotado");
        let _guardia = Guardia::nueva(&nombre, pid);
        // El `ping` no abre puerto, así que el servidor está «cargando»: es justo la ventana en la
        // que el plan solo miraba el puerto y lanzaba un segundo proceso.
        assert_eq!(estado(&s), Estado::Cargando);

        arrancar(&s).expect("la segunda vez no ha de ser un error: ya está en marcha");
        assert_eq!(pid_de(&nombre), Some(pid), "el pid no ha cambiado: no se lanzó otro");
        assert!(parar(&s).unwrap(), "y el que estaba se puede parar");
    }

    /// El ciclo entero sobre un proceso cuyo **hijo** sostiene el puerto, que es lo que hace un
    /// `serve.bat` de verdad: `arrancar` anota el pid del `cmd.exe`, el socket lo abre su hijo, y
    /// `estado` ha de decir `Listo` y `parar` ha de pararlo.
    ///
    /// Es la reproducción del Crítico C1 dentro de la suite, y sin ella el arreglo no queda
    /// protegido: con `parar` comparando el pid del `.pid` con el dueño del socket, aquí sale un
    /// `Err` «ajeno» y el servidor que nosotros mismos lanzamos se queda vivo. Es, además, lo que la
    /// spec §8 pide de `down` («`up` + `estado` + `down` sobre un comando largo trivial»).
    #[test]
    fn parar_mata_el_arbol_aunque_el_socket_lo_sostenga_su_hijo() {
        let nombre = format!("arbol-{}", std::process::id());
        let escucha = EscuchaDeVerdad::nueva("arbol");
        // El puerto que `arrancar` mira antes de lanzar ha de estar libre; el de verdad lo elige el
        // hijo, así que se empieza por el sordo y se corrige en cuanto el hijo lo dice.
        let mut s = escucha.servidor(&nombre, puerto_libre());
        arrancar(&s).expect("el .bat se lanza");
        let pid_del_cmd = pid_de(&nombre).expect("queda anotado el pid del cmd");
        let _guardia = Guardia::nueva(&nombre, pid_del_cmd);

        let puerto = escucha.puerto_abierto().expect("el hijo ha de decir qué puerto abrió");
        s.port = puerto;

        // Las dos caras del mismo instante: `estado` lo da por listo y `parar` ha de poder pararlo.
        assert_eq!(estado(&s), Estado::Listo, "el puerto contesta y el proceso es nuestro");
        // Y el socket **no** es del `cmd.exe`: lo sostiene su hijo. Si esto dejara de ser cierto, la
        // prueba estaría hablando de otro caso —el de la tarea 2— y conviene saberlo.
        match proceso_de_puerto(puerto) {
            Some(dueno) => assert_ne!(dueno, pid_del_cmd, "el socket ha de ser del hijo, no del cmd anotado"),
            None => eprintln!("netstat no dijo quién sostiene {puerto}: no se puede afirmar el reparto"),
        }

        assert!(parar(&s).unwrap(), "ha de parar el servidor que arrancamos, no llamarlo ajeno");
        assert!(pid_de(&nombre).is_none(), "y el `.pid` se retira");
        let limite = std::time::Instant::now() + Duration::from_secs(10);
        while puerto_contesta(puerto) && std::time::Instant::now() < limite {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!puerto_contesta(puerto), "y el puerto queda libre");
    }

    /// La mitad que le toca a [`parar`] de la limpieza del `.pid` caduco: un fichero que apunta a un
    /// proceso muerto, y un puerto libre, no son un error ni un «no se toca» —no hay nada de nadie
    /// que respetar—, sino un fichero que se retira y un «no estaba» dicho sin rodeos.
    #[test]
    fn parar_retira_el_pid_caduco_y_dice_que_no_estaba() {
        let nombre = format!("parar-caduco-{}", std::process::id());
        let mut hijo = Hijo::new();
        let pid = hijo.pid();
        hijo.matar();
        escribir_pid(&nombre, pid).unwrap();
        let _guardia = Guardia::del_fichero(&nombre);
        let s = servidor_que_escucha(&nombre, puerto_libre());

        assert!(!parar(&s).unwrap(), "no había nada que parar");
        assert!(pid_de(&nombre).is_none(), "y el `.pid` caduco se retira al saberlo");
    }

    #[test]
    fn parar_no_toca_un_puerto_ajeno() {
        // El puerto lo ocupa un listener nuestro, pero el `.pid` no apunta a nada vivo.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let nombre = format!("ajeno-{}", std::process::id());
        escribir_pid(&nombre, u32::MAX).unwrap();
        let _guardia = Guardia::del_fichero(&nombre);
        let s = Servidor { nombre: nombre.clone(), ..servidor_que_escucha(&nombre, l.local_addr().unwrap().port()) };
        let r = parar(&s);
        assert!(r.is_err(), "ha de negarse y decir por qué: {r:?}");
        assert!(r.unwrap_err().contains("ajeno"), "y el motivo ha de ser que el puerto es de otro");
        // Y el `.pid` del muerto **se retira también aquí**: no hay ningún proceso que respetar —el
        // que nombra ya no existe—, y dejarlo sería un dueño fantasma que nadie va a limpiar, porque
        // el único momento en que se sabe que ese fichero no vale es este. (Menor M1 de la revisión.)
        assert!(pid_de(&nombre).is_none(), "el `.pid` caduco se retira aunque el puerto sea de otro");
        let _ = l;
    }

    #[test]
    fn esperar_se_rinde_cuando_vence_el_plazo() {
        // Un puerto libre y un `ready_ms` corto: ha de devolver `Caido`/`SinArrancar` sin colgarse.
        let s = Servidor { ready_ms: 400, ..servidor_que_escucha("nunca", puerto_libre()) };
        // No se mide el reloj de `esperar`, que sería una carrera con la máquina: se monta un
        // **vigilante**. El defecto que esta prueba persigue es un cuelgue, y un cuelgue no se ve
        // comparando un cronómetro —se ve porque el mensaje no llega nunca—. Así la prueba cae sola
        // con un motivo, en vez de colgar la suite entera; y el plazo del vigilante es tan ancho
        // —diez segundos para un `ready_ms` de cuatrocientos milisegundos— que una máquina cargada
        // no lo roza.
        let (aviso, espera) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = aviso.send(esperar(&s));
        });
        match espera.recv_timeout(Duration::from_secs(10)) {
            Ok(e) => assert!(!matches!(e, Estado::Listo), "no puede estar listo: {e:?}"),
            Err(_) => panic!("`esperar` no volvió en 10 s con un `ready_ms` de 400 ms: se ha colgado"),
        }
    }

    /// La mitad que le toca a [`esperar`] de la limpieza del `.pid` caduco: [`estado`] es puro y no
    /// escribe (tarea 2), así que el fichero de un proceso muerto se retira aquí, en cuanto se sabe
    /// que no hay nada que esperar. Sin esto, un `.pid` huérfano se queda en el disco para siempre.
    ///
    /// La prueba **no** mira el reloj: lo que distingue a `esperar` dándose cuenta de la muerte de
    /// agotar el plazo es el propio `.pid` —las dos ramas devuelven `Caido`, pero solo la primera
    /// retira el fichero—, y esa es la propiedad que se afirma.
    #[test]
    fn esperar_retira_el_pid_caduco() {
        let nombre = format!("caduco-{}", std::process::id());
        let mut hijo = Hijo::new();
        let pid = hijo.pid();
        hijo.matar();
        escribir_pid(&nombre, pid).unwrap();
        let _guardia = Guardia::del_fichero(&nombre);
        let s = Servidor { ready_ms: 5_000, ..servidor_que_escucha(&nombre, puerto_libre()) };

        assert_eq!(esperar(&s), Estado::Caido, "un proceso muerto no se espera");
        assert!(pid_de(&nombre).is_none(), "y el `.pid` caduco se retira al saberlo");
    }

    /// `Listo` exige `.pid` nuestro y vivo (tarea 2). [`esperar`] devuelve un [`Estado`], así que no
    /// puede ser la puerta de atrás por la que un puerto ajeno se disfrace de servidor nuestro: el
    /// plan devolvía `Listo` en cuanto el puerto contestaba, y eso era el mismo error que la tarea 2
    /// quitó de [`estado`].
    #[test]
    fn esperar_no_da_por_listo_un_puerto_ajeno() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let nombre = format!("espera-ajeno-{}", std::process::id());
        let s = Servidor { ready_ms: 400, ..servidor_que_escucha(&nombre, l.local_addr().unwrap().port()) };
        let e = esperar(&s);
        assert!(!matches!(e, Estado::Listo), "un puerto ajeno no puede estar listo: {e:?}");
        // Y sin `.pid` no se ha anotado nada por el camino: `esperar` no arranca ni reclama nada.
        assert!(pid_de(&nombre).is_none());
        let _ = l;
    }

    #[test]
    fn la_cola_del_log_devuelve_las_ultimas_lineas() {
        let ruta = std::env::temp_dir().join(format!("cola-{}.log", std::process::id()));
        std::fs::write(&ruta, "una\ndos\ntres\ncuatro\n").unwrap();
        let s = Servidor { log: ruta.clone(), ..servidor_que_escucha("cola", puerto_libre()) };
        assert_eq!(cola_log(&s, 2), ["tres", "cuatro"]);
        let _ = std::fs::remove_file(&ruta);
    }
}
