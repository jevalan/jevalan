//! Start, probe and stop local servers. No UI: the screen only paints what comes out of here.

use crate::registro::{Servidor, dir_pids};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

/// What can be said about a server by looking at its `.pid` and its port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Estado {
    /// The port answers **and** the process is ours.
    Listo,
    /// There is a live process of ours, but the port does not answer yet: it is loading.
    Cargando,
    /// Neither port nor process: it fell over, or never got started.
    Caido,
    /// Someone else holds the port: it is said **with its pid**, which is what the spec asks for (§7).
    ///
    /// The `pid` travels as 0 when the system will not say it (`netstat` absent, or its output in
    /// a language that is not understood): no pid is faked.
    PuertoAjeno { pid: u32 },
    /// There is no `.pid` and the port is free.
    SinArrancar,
}

/// Does anyone answer on `port`? The local loopback is probed, with a short timeout: the network is not touched.
pub fn puerto_contesta(port: u16) -> bool {
    // 127.0.0.1 and not `localhost`: the probe must be local and not depend on name resolution.
    let dir = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&dir, Duration::from_millis(300)).is_ok()
}

/// Does process `pid` exist? 0 is not a valid pid, and huge ones do not exist.
pub fn pid_vivo(pid: u32) -> bool {
    if pid == 0 || pid == u32::MAX {
        return false;
    }
    #[cfg(windows)]
    {
        // `tasklist` is the route with no new dependencies; it is filtered in Rust, not in the shell.
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

/// The path of a named server's `.pid`.
fn ruta_pid(nombre: &str) -> Option<PathBuf> {
    dir_pids().map(|d| d.join(format!("{nombre}.pid")))
}

/// The pid recorded for `nombre`, if there is one and the file is understood.
///
/// A file that says `0` is read as if there were no pid: 0 is not a usable pid ([`pid_vivo`]
/// rejects it outright, and [`escribir_pid`] never writes it), and distinguishing "zero" from "unreadable"
/// would force every caller to filter it again. The consequence, stated: a `.pid` with 0
/// behaves like a `.pid` that is not there.
pub fn pid_de(nombre: &str) -> Option<u32> {
    let texto = std::fs::read_to_string(ruta_pid(nombre)?).ok()?;
    texto.trim().parse().ok().filter(|p| *p != 0)
}

/// Records a server's pid.
pub fn escribir_pid(nombre: &str, pid: u32) -> Result<(), String> {
    let ruta = ruta_pid(nombre).ok_or("sin directorio de datos para los `.pid`")?;
    if let Some(dir) = ruta.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("no se puede crear {}: {e}", dir.display()))?;
    }
    std::fs::write(&ruta, pid.to_string()).map_err(|e| format!("no se puede escribir {}: {e}", ruta.display()))
}

/// Deletes a server's `.pid`; if it was already gone, nothing happens.
///
/// **[`estado`] does not call it**, as it does not write: a stale `.pid` is cleaned up when someone decides to
/// (start again, stop (task 3)), not as a side effect of looking. If [`estado`] deleted it,
/// two consecutive calls would say different things, and whoever has to decide could not trust the
/// second.
pub fn borrar_pid(nombre: &str) {
    if let Some(ruta) = ruta_pid(nombre) {
        let _ = std::fs::remove_file(ruta);
    }
}

/// The pid `netstat` attributes to whoever listens on `port`, read from its output.
///
/// A listening line is `  TCP    127.0.0.1:8009    0.0.0.0:0    LISTENING    1234`: five fields,
/// the fourth `LISTENING` and the fifth the pid. It is split off from [`proceso_de_puerto`] so the
/// parsing can be tested without depending on `netstat` existing or answering: what can be missing is the program, not the
/// format. The port is matched by the whole number (`ends_with(":8009")`) so that `:8021` does not
/// count as `:21`, and it works the same for a bracketed address (`[::]:8009`).
///
/// Only called by the Windows branch of [`proceso_de_puerto`], so on Linux and macOS it would be left
/// unused and `clippy --all-targets -- -D warnings` would fall over for dead code: hence the `allow`, which
/// does not hide an oversight but a consequence of the `cfg`. The day the other branch learns `ss`/`lsof`,
/// this same function will feed it, without touching the `allow`.
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

/// The pid of whoever listens on `port`, if the system says. It is only asked when needed (an
/// occupied port that is not ours), because `netstat` takes hundreds of milliseconds.
///
/// When it cannot be known (no `netstat`, or an output that is not understood) it returns `None`, and
/// the caller will decide whether that is a 0 or a silence. No pid is guessed.
pub fn proceso_de_puerto(port: u16) -> Option<u32> {
    #[cfg(windows)]
    {
        let salida = std::process::Command::new("netstat").args(["-ano", "-p", "TCP"]).output().ok()?;
        pid_en_netstat(&String::from_utf8_lossy(&salida.stdout), port)
    }
    #[cfg(not(windows))]
    {
        let _ = port;
        None // `ss`/`lsof` when needed: today only Windows is supported for local servers.
    }
}

/// A server's state. **It writes nothing**: two consecutive calls give the same, which is what
/// lets whoever consumes it (start, the screen) decide with a single result.
///
/// The port is the signal that does not lie; the `.pid` says whether the process is ours. Without its own `.pid`,
/// a port that answers belongs to someone else (the `ollama` of the day, a neighbour, a hand-launched `serve.bat`
/// whose pid nobody recorded) and it is said with its pid: reporting it as `Listo` would be claiming a process
/// we did not start, which is exactly what spec §7 forbids.
pub fn estado(s: &Servidor) -> Estado {
    let nuestro = pid_de(&s.nombre);
    let vivo = nuestro.is_some_and(pid_vivo);
    if puerto_contesta(s.port) {
        if vivo {
            return Estado::Listo;
        }
        // It answers and is not ours: it is said **with the pid of whoever holds it**, which is what the
        // spec asks for (§7). `0` when the system will not say: no pid is faked.
        return Estado::PuertoAjeno { pid: proceso_de_puerto(s.port).unwrap_or(0) };
    }
    match nuestro {
        // `vivo` is already measured above: asking again about the same process would cost another
        // `tasklist` (hundreds of milliseconds) and could answer something else halfway through the same
        // call, which is exactly what this redesign removes.
        Some(_) if vivo => Estado::Cargando,
        Some(_) => Estado::Caido,
        None => Estado::SinArrancar,
    }
}

/// Launches the server without its own console and with its output appended to the registry's `log`.
///
/// Before launching anything it asks **whose the port is**, not just whether it answers: since
/// [`Estado::Listo`] requires a live `.pid` of ours (task 2), a port that answers without a `.pid` of ours
/// belongs to someone else. With a foreign one it neither launches (launching would be fighting over the port) nor says "it was
/// already there" (that would be lying about a process we did not start): it says whose it is and stops. And a
/// server that is already loading is not relaunched either: with Kev, which takes one or two minutes, that
/// window is the widest of all, and a second process would fight the first over the port and
/// over the machine's memory.
///
/// On detaching, what is promised and what is not: the process is created **without a console**
/// (with `CREATE_NO_WINDOW` and `DETACHED_PROCESS`), so it does not inherit jevalan's or open a window
/// of its own, and its output goes to the `log` and not to a screen. That it also survives jevalan closing
/// **has not been measured by any test** (on Windows a child does not die with its parent, so it should, but
/// that is a property of the system, not of these flags) and so it is not claimed.
///
/// The second flag is not decoration, even though `CreateProcess`'s documentation says it is ignored
/// together with `DETACHED_PROCESS`: with `DETACHED_PROCESS` alone, every `arrancar` opened a console
/// window. It was discovered because the test bench made one flash for every `ping` it launched.
///
/// And the `.pid` warning, which is the price of believing it: if the system reused the recorded pid for
/// another process, a server that has already died reads as [`Estado::Cargando`] and this returns `Ok` without
/// launching anything (a silent no-op). Its mirror is in [`parar`], where the same mistake kills that
/// innocent holder of the pid.
pub fn arrancar(s: &Servidor) -> Result<(), String> {
    match estado(s) {
        // Already ready, or already loading: in both cases there is a live process of ours and there is nothing to
        // launch.
        Estado::Listo | Estado::Cargando => return Ok(()),
        // Foreign: it is said whose, when the system knows (spec §7 asks for the pid), and it is not touched.
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
    // Without its own console: the child does not inherit jevalan's, and its output goes to the `log`.
    //
    // **Both** flags go in. `CREATE_NO_WINDOW` is not noise even though the manual says it is ignored
    // together with `DETACHED_PROCESS`: measured on this machine, with `DETACHED_PROCESS` alone every `arrancar`
    // pops up a console window (and the tests call `arrancar` with `ping` commands, so one
    // run leaves half a dozen flashes). What is observed counts, not what the manual promises.
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

/// Probes the port until `s.ready_ms`, reporting the state. It is what makes visible that Kev takes
/// one or two minutes: it returns the state, not a blind timeout.
///
/// When the port does answer, `Listo` is not simply claimed (the plan did so, and it was the back door
/// of the same mistake that task 2 removed from [`estado`]): the whole [`Estado`] is returned,
/// which will say `Listo` if the process is ours and [`Estado::PuertoAjeno`] if the port was taken by
/// someone else while we waited.
pub fn esperar(s: &Servidor) -> Estado {
    let limite = std::time::Instant::now() + Duration::from_millis(s.ready_ms);
    loop {
        if puerto_contesta(s.port) {
            return estado(s);
        }
        // If the process is gone, there is nothing to wait for.
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

/// Kills the process recorded in the `.pid`, **with its tree** (`taskkill /T`), and does not touch what we did
/// not start. Returns `false` if it was already gone.
///
/// The live `.pid` is **not compared with whoever holds the socket**, and this is deliberate: a `serve.bat`
/// (Kev's, Laya's) leaves the `.pid` pointing at the `cmd.exe` that launched it, and the socket is
/// held by **its child**, the venv's python. Comparing them turned the server we
/// had started ourselves into a "foreign" one, while [`estado`] reported it as `Listo` at the same moment:
/// `parar` refused and the server stayed alive. Killing the recorded process and its tree is what was
/// asked for, and it is the only thing that works with a `.bat`.
///
/// What it gives up, stated: if the system reused the recorded pid for another process, that other one is killed
/// (the `.pid` is the only proof of ownership there is, and it is believed). Foreign is, therefore, whatever
/// has **no** live `.pid` of ours behind it.
pub fn parar(s: &Servidor) -> Result<bool, String> {
    let Some(pid) = pid_de(&s.nombre) else {
        // Without a `.pid` there is nothing of ours to kill: if the port answers, it belongs to someone else.
        if puerto_contesta(s.port) {
            return Err(format!("el puerto {} lo ocupa un proceso ajeno: no se toca", s.port));
        }
        return Ok(false);
    };
    if !pid_vivo(pid) {
        // Stale: it is removed here, because `estado` is pure and writes nothing (task 2), and **before** saying
        // anything about the port: it is the file of a process that no longer exists, and nobody else will remove it.
        // If, in addition, someone else holds the port, it is said: that is more useful than an "ok" about a file that is no
        // longer valid.
        let ocupado = puerto_contesta(s.port);
        borrar_pid(&s.nombre);
        if ocupado {
            return Err(format!("el puerto {} lo ocupa un proceso ajeno: no se toca", s.port));
        }
        return Ok(false);
    }
    // Alive and ours: **the tree** is killed, and with it the children that hold the port.
    matar(pid)?;
    borrar_pid(&s.nombre);
    Ok(true)
}

/// Kills a process by its pid, with `taskkill` on Windows and `kill` elsewhere.
fn matar(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    let salida = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
    #[cfg(not(windows))]
    let salida = std::process::Command::new("kill").args(["-9", &pid.to_string()]).output();
    match salida {
        // `taskkill` returns 128 if the process was already gone: that is not a failure for us.
        Ok(o) if o.status.success() || o.status.code() == Some(128) => Ok(()),
        Ok(o) => Err(format!("no se pudo matar el proceso {pid}: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("no se pudo matar el proceso {pid}: {e}")),
    }
}

/// The last `lineas` of the server's log; empty if there is no log yet.
pub fn cola_log(s: &Servidor, lineas: usize) -> Vec<String> {
    let Ok(texto) = std::fs::read_to_string(&s.log) else { return Vec::new() };
    let todas: Vec<&str> = texto.lines().collect();
    todas[todas.len().saturating_sub(lineas)..].iter().map(|l| (*l).to_string()).collect()
}

#[cfg(test)]
mod tests {
    //! These tests **are not hermetic**, and it is worth knowing: they write real `.pid` files in
    //! jevalan's data directory (`%LOCALAPPDATA%\jevalan\servers`, see `registro::dir_pids`),
    //! launch child processes and open real ports. That is why each test uses its own server name
    //! (they run in parallel and share the directory), and why a panic midway can
    //! leave an orphan `.pid` and a live child: it is the price of testing what is touched.
    //!
    //! And they are **Windows in fact**, even though the crate compiles anywhere: the scaffolding launches a
    //! `ping -n` and a `.bat` that opens a `powershell`, and calls `tasklist`, `netstat` and
    //! `taskkill`; every test server also goes through the `cmd /c` that [`arrancar`] adds (the scaffolding's
    //! `ping` is the only child that does not go that way). On Linux or macOS they would compile (the
    //! `cfg` of [`Hijo`] and that of `matar` exist) but would not pass: there `proceso_de_puerto`
    //! returns `None` by design and there is no `cmd /c` to speak of. Supporting those systems is a separate
    //! job, not an oversight these tests can cover up.

    use super::*;
    use std::net::TcpListener;
    use std::path::PathBuf;

    /// A port where nobody listens and that **nobody can take from us**: one below the dynamic
    /// TCP range, which here starts at 1024 (`netsh int ipv4 show dynamicport tcp`: start 1024,
    /// 64511 ports) and goes up to 65535. The system hands out ephemeral ports from there, in order, so
    /// a high port (the `47_313` that was here) can be assigned to another test midway through
    /// the run; one below 1024, never. On Unix there is one more reason: without privileges it
    /// cannot be occupied.
    ///
    /// A `TcpListener` of our own that is "bound but not listening" is not used, which would be ideal, because in
    /// Rust it **does not exist**: `TcpListener::bind` calls `listen` internally and answers (measured; see the
    /// report). A deaf port has to be, perforce, a number that nobody occupies.
    ///
    /// What this number can still run into: a service that was already listening here. If that
    /// happens, the test fails **always and in the same place**, not sometimes; then another port
    /// below 1024 is chosen and the reason is left written down.
    const PUERTO_SORDO: u16 = 471;

    /// A plausible, complete server, with the name given by the test that uses it.
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

    /// A name used only by this test, so as not to clobber the `.pid` of another running at the same time.
    fn nombre(etiqueta: &str) -> String {
        format!("prueba-{etiqueta}-{}", std::process::id())
    }

    /// A real child, which lives a few seconds and dies when dropped.
    ///
    /// It exists because using `std::process::id()` to test the `.pid` files distinguishes nothing: the
    /// own pid is alive by definition, so an implementation that skipped the file and
    /// assumed the test's pid would pass all the same. A foreign child, which can also be killed, does
    /// distinguish. And it dies in `Drop` so that a panic does not leave an orphan `ping`.
    ///
    /// It is launched **without a console** (`CREATE_NO_WINDOW`), which is what [`arrancar`] does with the
    /// real servers: a console child launched from a process that has none of its own (the
    /// case here) gets a new window, and the suite opens and closes half a dozen per
    /// run. The flag changes nothing of what is tested here: the child is still alive, and `pid_vivo`
    /// and `kill` see it the same.
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

    /// A real, ephemeral server: a `cmd` that stays alive until it is killed.
    ///
    /// Its `command` is a `ping`, which **opens no ports**: this way the test decides with its own
    /// `TcpListener` whether the port answers, without depending on what the child does. It goes **without** `cmd /c`
    /// in front (which is what [`arrancar`] adds) because that is how the real `servers.yaml` declares it:
    /// carrying it here left `cmd /c cmd /c ping`, one extra `cmd` per test server. The `.pid` that is
    /// recorded is the same in both cases: the outer `cmd`'s.
    fn servidor_que_escucha(nombre: &str, port: u16) -> Servidor {
        // `ping` keeps the process alive without opening ports; the probe is tested with a separate listener.
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

    /// A port where nobody listens. It is [`PUERTO_SORDO`] (below the dynamic range, and already
    /// reasoned out in the tests above): the plan called it `puerto_libre()` without defining it, and there
    /// is no reason to invent another number than the one task 2 already chose for good reasons.
    fn puerto_libre() -> u16 {
        PUERTO_SORDO
    }

    /// A `.bat` that launches a child that **holds the port**, and leaves in a file which one it opened.
    ///
    /// It is the shape of a real `serve.bat` (Kev's, Laya's), without its venv: `arrancar` records
    /// the `cmd.exe`'s pid, and the socket is opened by **its child**. The test bench lacked this case
    /// (its only child, `cmd /c ping`, opens no ports), and that is why the branch of `parar` that decided whether
    /// the port was foreign reached production unexercised (Critical C1 of review 1).
    ///
    /// The port is chosen by the child, not the test: reserving it here to pass it on would mean keeping it
    /// ourselves, and releasing it for the child to take would be a race with the other tests. The child
    /// asks for 0 ("whichever there is") and writes the chosen one to a file, which is the only thing the test
    /// cannot know in advance.
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
            // `cmd` cannot open a socket, so the child that listens has to be another one; PowerShell
            // is on every Windows machine and adds no dependencies to the crate. The file path goes
            // in single quotes (so that `cmd` does not touch it) and the whole `-Command` in double quotes.
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

        /// The registry server pointing at that `.bat`.
        ///
        /// `port` is the one [`arrancar`] looks at **before** launching (it must be free for it to launch);
        /// the real one is known afterwards, and whoever calls must correct it.
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

        /// The port the child says it opened, or `None` if it does not say so in time.
        fn puerto_abierto(&self) -> Option<u16> {
            let limite = std::time::Instant::now() + Duration::from_secs(30);
            while std::time::Instant::now() < limite {
                // The file may be caught half-written; the retry covers it.
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

    /// A `cmd` launched by [`arrancar`], which has to die no matter what.
    ///
    /// It exists because an assertion that fails before `parar` would leave a `ping` alive for thirty
    /// seconds **and** its `.pid` in the data directory: the same damage as a red test,
    /// but silent. `parar` kills it the normal way; this is the safety net in case it is not reached.
    ///
    /// It looks in **two** places, and the second is the one that matters when something fails: what the `.pid`
    /// on disk says (it may point to a second launch the test never got to see) and the pid
    /// the test knew about. With `pid: None` it also serves tests where there is no known process
    /// of its own beforehand and only a file remains to be removed (M4 of review 1).
    struct Guardia {
        nombre: String,
        pid: Option<u32>,
    }

    impl Guardia {
        /// The guard for the ordinary case: it is known in advance which process was launched.
        fn nueva(nombre: &str, pid: u32) -> Self {
            Self { nombre: nombre.to_string(), pid: Some(pid) }
        }

        /// The guard in which only the file knows what has to be killed, and what has to be removed.
        fn del_fichero(nombre: &str) -> Self {
            Self { nombre: nombre.to_string(), pid: None }
        }
    }

    impl Drop for Guardia {
        fn drop(&mut self) {
            // What the file says is also checked: if a red test got to launch a
            // second process, `arrancar` recorded **its** pid, and that is the one that would be left orphaned.
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
        // A pid that cannot exist on Windows: 0 and huge ones are rejected.
        assert!(!pid_vivo(0));
        assert!(!pid_vivo(u32::MAX));
    }

    #[test]
    fn el_estado_cuenta_las_cuatro_situaciones() {
        let nombre = nombre("cuatro");

        // Without a `.pid` and with nobody on the port: it has not been started.
        assert_eq!(estado(&servidor(&nombre, PUERTO_SORDO)), Estado::SinArrancar);

        // With a real live child and the port silent: it is loading. The pid is not ours, so
        // the test cannot pass by "assuming the own pid".
        let mut hijo = Hijo::new();
        let pid = hijo.pid();
        escribir_pid(&nombre, pid).unwrap();
        assert_eq!(estado(&servidor(&nombre, PUERTO_SORDO)), Estado::Cargando);

        // With the port answering **and** the process alive: ready.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        assert_eq!(estado(&servidor(&nombre, l.local_addr().unwrap().port())), Estado::Listo);
        drop(l);

        // And once the child is dead: down, with the `.pid` still in place (`estado` does not delete it).
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

        // Without a `.pid` of ours (the ordinary case: `ollama`, a neighbour) the port is not ours, and it is not
        // disguised as `Listo`. The pid must be the one `netstat` gives; if `netstat` does not answer, 0,
        // because no pid is faked.
        match (estado(&s), proceso_de_puerto(puerto)) {
            (Estado::PuertoAjeno { pid }, Some(esperado)) => {
                assert_eq!(pid, esperado, "el estado ha de llevar el pid que da netstat");
                // And the listener belongs to this very process, so that pid must be ours.
                assert_eq!(esperado, std::process::id(), "el listener es de este mismo proceso");
            }
            (Estado::PuertoAjeno { pid }, None) => {
                assert_eq!(pid, 0, "sin netstat no se finge un pid");
                // Out loud, so that `--nocapture` records why nothing more was asserted.
                eprintln!("netstat no contestó por {puerto}: no se pudo afirmar el pid ajeno");
            }
            (otro, _) => panic!("un puerto ajeno ha de señalarse como tal, y salió {otro:?}"),
        }

        // A stale `.pid` does not change it: the port is still someone else's, and the file stays where it
        // is (cleaning it up is for whoever starts or stops, not whoever looks).
        escribir_pid(&nombre, u32::MAX).unwrap();
        let primero = estado(&s);
        assert!(matches!(primero, Estado::PuertoAjeno { .. }), "{primero:?}");
        assert_eq!(estado(&s), primero, "`estado` ha de ser idempotente");
        assert_eq!(pid_de(&nombre), Some(u32::MAX), "`estado` no ha de borrar el `.pid`");

        // And with a `.pid` of a live process and the port answering, the port is indeed ours.
        let mut hijo = Hijo::new();
        escribir_pid(&nombre, hijo.pid()).unwrap();
        assert_eq!(estado(&s), Estado::Listo);
        hijo.matar();
        borrar_pid(&nombre);
        let _ = l;
    }

    /// The invariant the redesign brings: `estado` reads and does not write, so two consecutive calls
    /// give the same in the two cases that used to change from tick to tick.
    #[test]
    fn el_estado_dos_veces_seguidas_da_lo_mismo() {
        // Deaf port with a stale `.pid`: it used to say `Caido` and the second time `SinArrancar`, because
        // it deleted the file on detecting it.
        let caido = nombre("idempotente-caido");
        escribir_pid(&caido, u32::MAX).unwrap();
        let sordo = servidor(&caido, PUERTO_SORDO);
        assert_eq!(estado(&sordo), Estado::Caido);
        assert_eq!(estado(&sordo), Estado::Caido, "la segunda llamada ha de decir lo mismo");
        assert_eq!(pid_de(&caido), Some(u32::MAX), "y el `.pid` caduco sigue donde estaba");
        borrar_pid(&caido);

        // Occupied port with a stale `.pid`: it used to say `PuertoAjeno` and the second time `Listo`.
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

    /// 0 is not a pid: a file that says it is read as if there were no pid. It is decided so (and not
    /// by distinguishing "zero" from "unreadable") because `pid_vivo(0)` is false by definition and
    /// `escribir_pid` never writes it: distinguishing it would force every caller to filter it again.
    #[test]
    fn un_pid_cero_en_el_fichero_no_cuenta_como_pid() {
        let nombre = nombre("cero");
        escribir_pid(&nombre, 0).unwrap();
        assert_eq!(pid_de(&nombre), None, "el 0 no es un pid utilizable");
        assert_eq!(estado(&servidor(&nombre, PUERTO_SORDO)), Estado::SinArrancar);

        // And with the port answering, a `.pid` that says 0 does not make us the owners of the port either.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let ocupado = servidor(&nombre, l.local_addr().unwrap().port());
        let visto = estado(&ocupado);
        assert!(matches!(visto, Estado::PuertoAjeno { .. }), "{visto:?}");
        borrar_pid(&nombre);
    }

    /// The output of `netstat -ano -p TCP` on this machine, with the registry's ports in place of
    /// its own: the header is literal (in Spanish, and `LISTENING` stays in English) and the lines
    /// have the exact form it prints. The last one, in brackets, is TCPv6: `-p TCP` does not
    /// print it today, but the same format appears as soon as someone changes flag or machine.
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
        // A port that only appears connected, not listening, belongs to nobody who is waiting there.
        assert_eq!(pid_en_netstat(NETSTAT, 52000), None);
        // And a port that is not there, neither.
        assert_eq!(pid_en_netstat(NETSTAT, 1111), None);
        // Matching is by the whole port, not its tail: `:8021` must not count as `:21`.
        assert_eq!(pid_en_netstat(NETSTAT, 21), None);
        assert_eq!(pid_en_netstat(NETSTAT, 800), None);
        // The header and short lines are skipped without breaking anything.
        assert_eq!(pid_en_netstat("Proto  Dirección local\n\n", 8009), None);
        assert_eq!(pid_en_netstat("", 8009), None);
    }

    /// Does `netstat` exist and answer something sensible? It is the only way to tell a `None` from
    /// [`proceso_de_puerto`] meaning "there is no `netstat` here" from one meaning "there is one and
    /// I could not read it", which would be a failure of ours.
    fn netstat_disponible() -> bool {
        std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("LISTENING"))
    }

    /// The real `proceso_de_puerto`: talks to `netstat`, which takes hundreds of milliseconds and may
    /// not be on the child process's PATH. That is why the test **does not require `netstat` to be there**: if it is not
    /// there, it says so and settles. But if it is (and here it is), a `None` on our own listener is
    /// no longer "could not": it is that we did not know how to read its output, and then the test must fail. Our own
    /// listener is deterministic: the pid must be ours.
    #[test]
    fn el_pid_de_un_puerto_lo_dice_netstat_si_esta() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let puerto = l.local_addr().unwrap().port();
        match (proceso_de_puerto(puerto), netstat_disponible()) {
            (Some(pid), _) => assert_eq!(pid, std::process::id(), "el listener es de este mismo proceso"),
            (None, false) => eprintln!("netstat no está en el PATH: no se pudo afirmar el pid de {puerto}"),
            (None, true) => panic!("netstat contesta, pero no supo decir el pid de {puerto}"),
        }
        // And on a deaf port there is nobody to attribute it to.
        assert_eq!(proceso_de_puerto(PUERTO_SORDO), None);
    }

    // ------------------------------------------------------------------------------------------
    // Arrancar, esperar y parar (tarea 3)
    // ------------------------------------------------------------------------------------------

    /// The plan said "if the port already answers, I launch nothing", and that stopped being enough when task
    /// 2 made a port without a `.pid` of ours a [`Estado::PuertoAjeno`]: one has to look at **whose**
    /// it is. If it is someone else's, nothing is launched (launching would be fighting over the port) and it is not said "it was
    /// already there" (that would be lying about a process we did not start). If it is ours, it really was already there
    /// and there is nothing to do. Both halves are tested here because the same port answering
    /// must give a different response depending on whose it is.
    #[test]
    fn arrancar_no_lanza_nada_si_el_puerto_ya_contesta() {
        // Foreign: this very test's listener, with no `.pid` claiming it.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let ajeno = format!("ya-vivo-ajeno-{}", std::process::id());
        let _guardia_ajeno = Guardia::del_fichero(&ajeno);
        let s = Servidor { nombre: ajeno.clone(), ..servidor_que_escucha(&ajeno, l.local_addr().unwrap().port()) };
        let r = arrancar(&s);
        assert!(r.is_err(), "sobre un puerto ajeno no se lanza: {r:?}");
        assert!(r.unwrap_err().contains("ajeno"), "y el motivo ha de ser que el puerto es de otro");
        assert!(pid_de(&ajeno).is_none(), "no se anota un pid que no hemos arrancado");

        // Ours: live `.pid` and the port answering. The `.pid` is not touched, which is the proof
        // that nothing was launched: a launch would have overwritten it with its own.
        let nuestro = format!("ya-vivo-nuestro-{}", std::process::id());
        let mut hijo = Hijo::new();
        escribir_pid(&nuestro, hijo.pid()).unwrap();
        // If an assertion fails, neither the `.pid` nor a `ping` that `arrancar` might have launched in excess
        // may be left behind: the guard looks at the file, which is what would know about that second process.
        let _guardia_nuestro = Guardia::del_fichero(&nuestro);
        let s = Servidor { nombre: nuestro.clone(), ..servidor_que_escucha(&nuestro, l.local_addr().unwrap().port()) };
        assert!(arrancar(&s).is_ok(), "un servidor ya listo no se vuelve a lanzar");
        assert_eq!(pid_de(&nuestro), Some(hijo.pid()), "y el pid anotado sigue siendo el suyo");
        hijo.matar();
        borrar_pid(&nuestro);
        let _ = l;
    }

    /// A `cwd` that does not exist is rejected **before** launching anything: the `cmd` would die on not being able to
    /// settle in it, and the reason that would reach the screen would be `spawn`'s, not the real one.
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
        // From here on the child must die no matter what; `parar` will do it first, the normal
        // way, and the guard's `Drop` is the safety net in case an assertion fails before.
        let _guardia = Guardia::nueva(&nombre, pid);
        assert!(pid_vivo(pid), "y el proceso vive");

        assert!(parar(&s).unwrap(), "y se puede parar");
        assert!(pid_de(&nombre).is_none(), "el `.pid` se borra al parar");
        // Give the system a moment to reap the process. The plan slept 500 ms and looked
        // once; with the machine loaded (another session compiling at the same time) that single glance fails
        // and the test falls over because of the clock, not the code. A deadline is granted and it is polled within
        // it: the assertion is the same (the process must die), but it stops being a race.
        let limite = std::time::Instant::now() + Duration::from_secs(5);
        while pid_vivo(pid) && std::time::Instant::now() < limite {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!pid_vivo(pid), "y el proceso ya no está");
    }

    /// The other half of Review Focus 2: "starting twice in a row". With a server that takes a while
    /// (Kev takes one or two minutes) the port is silent for most of that time, so the port
    /// guard is not enough: the second press of "start" must see that **there is already a live
    /// process of ours** and not launch another that fights for the port and for the machine's memory.
    #[test]
    fn arrancar_dos_veces_seguidas_no_lanza_un_segundo_proceso() {
        let nombre = format!("dos-veces-{}", std::process::id());
        let s = servidor_que_escucha(&nombre, puerto_libre());
        arrancar(&s).expect("se lanza");
        let pid = pid_de(&nombre).expect("queda anotado");
        let _guardia = Guardia::nueva(&nombre, pid);
        // The `ping` opens no port, so the server is "loading": it is exactly the window in which
        // the plan only looked at the port and launched a second process.
        assert_eq!(estado(&s), Estado::Cargando);

        arrancar(&s).expect("la segunda vez no ha de ser un error: ya está en marcha");
        assert_eq!(pid_de(&nombre), Some(pid), "el pid no ha cambiado: no se lanzó otro");
        assert!(parar(&s).unwrap(), "y el que estaba se puede parar");
    }

    /// The whole cycle over a process whose **child** holds the port, which is what a real
    /// `serve.bat` does: `arrancar` records the `cmd.exe`'s pid, the socket is opened by its child, and
    /// `estado` must say `Listo` and `parar` must stop it.
    ///
    /// It is the reproduction of Critical C1 inside the suite, and without it the fix is not
    /// protected: with `parar` comparing the `.pid`'s pid with the socket's owner, here a "foreign"
    /// `Err` comes out and the server we launched ourselves stays alive. It is also what
    /// spec §8 asks of `down` ("`up` + `estado` + `down` over a trivially long command").
    #[test]
    fn parar_mata_el_arbol_aunque_el_socket_lo_sostenga_su_hijo() {
        let nombre = format!("arbol-{}", std::process::id());
        let escucha = EscuchaDeVerdad::nueva("arbol");
        // The port `arrancar` looks at before launching must be free; the real one is chosen by the
        // child, so it starts with the deaf one and is corrected as soon as the child says it.
        let mut s = escucha.servidor(&nombre, puerto_libre());
        arrancar(&s).expect("el .bat se lanza");
        let pid_del_cmd = pid_de(&nombre).expect("queda anotado el pid del cmd");
        let _guardia = Guardia::nueva(&nombre, pid_del_cmd);

        let puerto = escucha.puerto_abierto().expect("el hijo ha de decir qué puerto abrió");
        s.port = puerto;

        // The two faces of the same instant: `estado` reports it as ready and `parar` must be able to stop it.
        assert_eq!(estado(&s), Estado::Listo, "el puerto contesta y el proceso es nuestro");
        // And the socket is **not** the `cmd.exe`'s: its child holds it. If that stopped being true, the
        // test would be talking about another case (task 2's) and it is worth knowing.
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

    /// [`parar`]'s share of the stale `.pid` cleanup: a file that points to a
    /// dead process, and a free port, are neither an error nor a "not touched" (there is nothing of anyone's
    /// to respect) but a file that is removed and a "it was not there" said plainly.
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
        // The port is occupied by a listener of ours, but the `.pid` points at nothing alive.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let nombre = format!("ajeno-{}", std::process::id());
        escribir_pid(&nombre, u32::MAX).unwrap();
        let _guardia = Guardia::del_fichero(&nombre);
        let s = Servidor { nombre: nombre.clone(), ..servidor_que_escucha(&nombre, l.local_addr().unwrap().port()) };
        let r = parar(&s);
        assert!(r.is_err(), "ha de negarse y decir por qué: {r:?}");
        assert!(r.unwrap_err().contains("ajeno"), "y el motivo ha de ser que el puerto es de otro");
        // And the dead one's `.pid` **is removed here too**: there is no process to respect (the
        // one it names no longer exists), and leaving it would be a phantom owner that nobody will clean up, because
        // the only moment at which it is known that this file is not valid is this one. (Minor M1 of the review.)
        assert!(pid_de(&nombre).is_none(), "el `.pid` caduco se retira aunque el puerto sea de otro");
        let _ = l;
    }

    #[test]
    fn esperar_se_rinde_cuando_vence_el_plazo() {
        // A free port and a short `ready_ms`: it must return `Caido`/`SinArrancar` without hanging.
        let s = Servidor { ready_ms: 400, ..servidor_que_escucha("nunca", puerto_libre()) };
        // `esperar`'s clock is not measured, which would be a race with the machine: a
        // **watchdog** is set up. The defect this test hunts is a hang, and a hang is not seen
        // by comparing a stopwatch (it is seen because the message never arrives). This way the test fails by itself
        // with a reason, instead of hanging the whole suite; and the watchdog's deadline is so wide
        // (ten seconds for a `ready_ms` of four hundred milliseconds) that a loaded machine
        // does not come near it.
        let (aviso, espera) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = aviso.send(esperar(&s));
        });
        match espera.recv_timeout(Duration::from_secs(10)) {
            Ok(e) => assert!(!matches!(e, Estado::Listo), "no puede estar listo: {e:?}"),
            Err(_) => panic!("`esperar` no volvió en 10 s con un `ready_ms` de 400 ms: se ha colgado"),
        }
    }

    /// [`esperar`]'s share of the stale `.pid` cleanup: [`estado`] is pure and writes nothing
    /// (task 2), so a dead process's file is removed here, as soon as it is known
    /// that there is nothing to wait for. Without this, an orphan `.pid` stays on disk forever.
    ///
    /// The test does **not** look at the clock: what distinguishes `esperar` noticing the death from
    /// running out the deadline is the `.pid` itself (both branches return `Caido`, but only the first
    /// removes the file), and that is the property asserted.
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

    /// `Listo` requires a live `.pid` of ours (task 2). [`esperar`] returns an [`Estado`], so it
    /// cannot be the back door through which a foreign port disguises itself as our server: the
    /// plan returned `Listo` as soon as the port answered, and that was the same mistake that task 2
    /// removed from [`estado`].
    #[test]
    fn esperar_no_da_por_listo_un_puerto_ajeno() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let nombre = format!("espera-ajeno-{}", std::process::id());
        let s = Servidor { ready_ms: 400, ..servidor_que_escucha(&nombre, l.local_addr().unwrap().port()) };
        let e = esperar(&s);
        assert!(!matches!(e, Estado::Listo), "un puerto ajeno no puede estar listo: {e:?}");
        // And without a `.pid` nothing was recorded along the way: `esperar` neither starts nor claims anything.
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
