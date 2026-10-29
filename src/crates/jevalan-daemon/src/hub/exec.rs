//! `exec` provider: process without a shell, minimal environment, bounded output and killed when the deadline expires.

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

/// Launches `command` with `input` on stdin and returns its stdout as JSON, or the error `Status`.
pub async fn run(
    command: &[String],
    env: &[String],
    dotenv: &BTreeMap<String, String>,
    input: &Value,
    deadline: Duration,
) -> Result<Value, Status> {
    // A `.cmd`/`.bat` would be launched through `cmd.exe`, which is an interpreter: no way.
    if is_batch(&command[0]) {
        return Err(failed("un .cmd/.bat pasaría por cmd.exe; usa node con la ruta del script"));
    }
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..]).env_clear();
    for name in ["PATH", "SystemRoot"].into_iter().chain(env.iter().map(String::as_str)) {
        // First the process environment; if missing, the `.env` loaded at start-up.
        if let Some(value) = std::env::var_os(name) {
            cmd.env(name, value);
        } else if let Some(value) = dotenv.get(name) {
            cmd.env(name, value);
        }
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // Own group: the deadline kills the specialist and all its descendants.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| failed(format!("no se pudo lanzar el especialista: {:?}", e.kind())))?;
    let mut tree = Tree::adopt(&child);
    let (stdin, stdout, stderr) = match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
        (Some(i), Some(o), Some(e)) => (i, o, e),
        _ => return Err(failed("el especialista no abrió sus tuberías")),
    };
    let payload = input.to_string().into_bytes();

    let work = async {
        let feed = async {
            let mut stdin = stdin;
            // A specialist that does not read its input closes the pipe: not our fault.
            let _ = stdin.write_all(&payload).await;
            Ok(())
        };
        let exit = async {
            let status = child.wait().await.map_err(|e| failed(format!("espera fallida: {:?}", e.kind())))?;
            // The specialist is gone: anything it left behind would keep its pipes open until the deadline.
            tree.reap();
            Ok::<_, Status>(status)
        };
        let (_, status, out, err) = tokio::try_join!(
            feed,
            exit,
            read_capped(stdout, MAX_STDOUT, true),
            read_capped(stderr, MAX_STDERR, false)
        )?;
        Ok::<_, Status>((status, out, err))
    };
    let (status, out, err) = match tokio::time::timeout(deadline, work).await {
        Ok(Ok(done)) => done,
        Ok(Err(e)) => {
            tree.kill(&mut child).await;
            return Err(e);
        }
        Err(_) => {
            tree.kill(&mut child).await;
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

/// Handle on everything the specialist starts: dropping it from the world is how its descendants die.
struct Tree {
    pid: Option<u32>,
    /// Windows has no process groups: the specialist and its descendants live in a job that kills them
    /// when it is closed. `None` if the job could not be set up, and then `taskkill` is the fallback.
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

impl Tree {
    fn adopt(child: &tokio::process::Child) -> Self {
        Self {
            pid: child.id(),
            #[cfg(windows)]
            job: Self::job_for(child),
        }
    }

    /// A descendant that the specialist starts before this runs would escape the job, but the window is
    /// the few instructions between the spawn and the assignment.
    #[cfg(windows)]
    fn job_for(child: &tokio::process::Child) -> Option<win32job::Job> {
        let handle = child.raw_handle()? as isize;
        let mut limits = win32job::ExtendedLimitInfo::new();
        limits.limit_kill_on_job_close();
        let job = win32job::Job::create_with_limit_info(&limits).ok()?;
        job.assign_process(handle).ok()?;
        Some(job)
    }

    /// Kills what the specialist left behind once it has exited.
    fn reap(&mut self) {
        #[cfg(windows)]
        drop(self.job.take());
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--"])
                .arg(format!("-{pid}"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    /// Kills the whole tree (bluntly and without waiting for a result) and then the direct child.
    async fn kill(&mut self, child: &mut tokio::process::Child) {
        #[cfg(windows)]
        let closed_job = self.job.take().is_some();
        #[cfg(not(windows))]
        let closed_job = false;
        if let (Some(pid), false) = (self.pid, closed_job) {
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
}

/// Reads to the end; more than `cap` bytes is a failure if `strict`, otherwise the excess is discarded.
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
