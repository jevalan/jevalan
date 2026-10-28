//! Proveedor `exec`: proceso sin shell, entorno mínimo, salida acotada y muerte al agotar el plazo.

use jevalan_hub::capability::is_batch;
use jevalan_hub::classify::{ErrorCode, Status};
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const MAX_STDOUT: usize = 1024 * 1024;
const MAX_STDERR: usize = 2048;

fn failed(message: impl Into<String>) -> Status {
    Status::error(ErrorCode::ProviderFailed, message)
}

/// Lanza `command` con `input` por stdin y devuelve su stdout como JSON, o el `Status` de error.
pub async fn run(
    command: &[String],
    env: &[String],
    dotenv: &BTreeMap<String, String>,
    input: &Value,
    deadline: Duration,
) -> Result<Value, Status> {
    // Un `.cmd`/`.bat` se lanzaría a través de `cmd.exe`, que es un intérprete: ni hablar.
    if is_batch(&command[0]) {
        return Err(failed("un .cmd/.bat pasaría por cmd.exe; usa node con la ruta del script"));
    }
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..]).env_clear();
    for name in ["PATH", "SystemRoot"].into_iter().chain(env.iter().map(String::as_str)) {
        // Primero el entorno del proceso; si falta, el `.env` cargado al arrancar.
        if let Some(value) = std::env::var_os(name) {
            cmd.env(name, value);
        } else if let Some(value) = dotenv.get(name) {
            cmd.env(name, value);
        }
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // Grupo propio: el plazo mata al especialista y a toda su descendencia.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| failed(format!("no se pudo lanzar el especialista: {:?}", e.kind())))?;
    let (stdin, stdout, stderr) = match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
        (Some(i), Some(o), Some(e)) => (i, o, e),
        _ => return Err(failed("el especialista no abrió sus tuberías")),
    };
    let payload = input.to_string().into_bytes();

    let work = async {
        let feed = async {
            let mut stdin = stdin;
            // Un especialista que no lee su entrada cierra la tubería: no es culpa nuestra.
            let _ = stdin.write_all(&payload).await;
            Ok(())
        };
        let (_, out, err) =
            tokio::try_join!(feed, read_capped(stdout, MAX_STDOUT, true), read_capped(stderr, MAX_STDERR, false))?;
        let status = child.wait().await.map_err(|e| failed(format!("espera fallida: {:?}", e.kind())))?;
        Ok::<_, Status>((status, out, err))
    };
    let (status, out, err) = match tokio::time::timeout(deadline, work).await {
        Ok(Ok(done)) => done,
        Ok(Err(e)) => {
            kill_tree(&mut child).await;
            return Err(e);
        }
        Err(_) => {
            kill_tree(&mut child).await;
            return Err(Status::error(ErrorCode::DeadlineExceeded, "el especialista no respondió a tiempo"));
        }
    };
    let detail = String::from_utf8_lossy(&err).trim().to_string();
    if !status.success() {
        let code = status.code().map_or("señal".to_string(), |c| c.to_string());
        return Err(failed(format!("el especialista terminó con {code}: {detail}")));
    }
    serde_json::from_slice(&out).map_err(|_| failed(format!("la salida del especialista no es JSON: {detail}")))
}

/// Mata el árbol entero (a lo bruto y sin esperar resultado) y después al hijo directo.
async fn kill_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        let mut killer = if cfg!(windows) {
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            let mut k = Command::new(std::path::Path::new(&root).join("System32").join("taskkill.exe"));
            k.args(["/T", "/F", "/PID"]).arg(pid.to_string());
            k
        } else {
            let mut k = Command::new("kill");
            k.args(["-KILL", "--"]).arg(format!("-{pid}"));
            k
        };
        killer.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let _ = killer.status().await;
    }
    let _ = child.kill().await;
}

/// Lee hasta el final; más de `cap` bytes es un fallo si `strict`, y si no se descarta el exceso.
async fn read_capped(mut r: impl AsyncRead + Unpin, cap: usize, strict: bool) -> Result<Vec<u8>, Status> {
    let (mut buf, mut chunk) = (Vec::new(), [0u8; 8192]);
    loop {
        let n = r.read(&mut chunk).await.map_err(|e| failed(format!("lectura fallida: {:?}", e.kind())))?;
        if n == 0 {
            return Ok(buf);
        }
        let room = cap - buf.len();
        if n > room && strict {
            return Err(failed("la salida del especialista supera 1 MiB"));
        }
        buf.extend_from_slice(&chunk[..n.min(room)]);
    }
}
