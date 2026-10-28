//! Cliente HTTP del daemon: localiza la dirección como el hook y traduce los fallos a motivos legibles.

use jevalan_daemon::config::DEFAULT_ADDR;
use jevalan_daemon::discovery;
use serde_json::Value;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(35);

/// Lo que el catálogo necesita del daemon; las pruebas lo sustituyen por un simulacro.
pub trait Http {
    fn get(&self, path: &str) -> Result<Value, String>;
    fn post(&self, path: &str, body: &Value) -> Result<Value, String>;
}

/// A quién se llama, por orden: la dirección explícita (y solo ella), o la que anuncia `daemon.json`
/// y después la de por defecto. Un token explícito manda sobre el anunciado.
pub fn candidates(
    addr: Option<&str>,
    token: Option<&str>,
    discovered: Option<(String, Option<String>)>,
) -> Vec<(String, Option<String>)> {
    if let Some(addr) = addr {
        return vec![(addr.to_string(), token.map(str::to_string))];
    }
    let (found, announced) = match discovered {
        Some((a, t)) => (Some(a), t),
        None => (None, None),
    };
    let token = token.map(str::to_string).or(announced);
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for a in found.into_iter().chain([DEFAULT_ADDR.to_string()]) {
        if !out.iter().any(|(x, _)| *x == a) {
            out.push((a, token.clone()));
        }
    }
    out
}

pub struct HttpDaemon {
    addr: Option<String>,
    token: Option<String>,
    runtime: tokio::runtime::Runtime,
    http: reqwest::Client,
}

impl HttpDaemon {
    pub fn new(addr: Option<String>, token: Option<String>) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime de tokio");
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TOTAL_TIMEOUT)
            .build()
            .expect("cliente HTTP");
        Self { addr, token, runtime, http }
    }

    /// Prueba cada candidato por orden; solo un fallo de conexión pasa al siguiente.
    fn send(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let list = candidates(self.addr.as_deref(), self.token.as_deref(), discovery::read());
        let mut last = String::from("sin daemon");
        for (addr, token) in list {
            let mut req =
                self.http.request(method.clone(), format!("http://{addr}{path}")).header("X-Jevalan-Client", "mcp");
            if let Some(token) = &token {
                req = req.bearer_auth(token);
            }
            if let Some(body) = body {
                req = req.json(body);
            }
            let sent = self.runtime.block_on(async {
                let resp = req.send().await?;
                let status = resp.status();
                let text = resp.text().await?;
                Ok::<_, reqwest::Error>((status, text))
            });
            match sent {
                Ok((status, text)) => {
                    let value: Value =
                        serde_json::from_str(&text).map_err(|_| format!("respuesta ilegible (HTTP {status})"))?;
                    if status.is_success() {
                        return Ok(value);
                    }
                    let why = value
                        .get("error")
                        .and_then(Value::as_str)
                        .map_or_else(|| format!("HTTP {status}"), str::to_string);
                    return Err(why);
                }
                Err(e) if e.is_connect() || e.is_timeout() => {
                    last = format!("daemon inalcanzable en {addr}: {}", e.without_url())
                }
                Err(e) => return Err(format!("fallo hablando con el daemon: {}", e.without_url())),
            }
        }
        Err(last)
    }
}

impl Http for HttpDaemon {
    fn get(&self, path: &str) -> Result<Value, String> {
        self.send(reqwest::Method::GET, path, None)
    }
    fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.send(reqwest::Method::POST, path, Some(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = jevalan_daemon::config::DEFAULT_ADDR;

    fn pair(a: &str, t: Option<&str>) -> (String, Option<String>) {
        (a.to_string(), t.map(str::to_string))
    }

    #[test]
    fn candidates_with_explicit_addr_is_only_that() {
        let found = Some(pair("127.0.0.1:9000", Some("anunciado")));
        assert_eq!(candidates(Some("10.0.0.1:1"), Some("t"), found), vec![pair("10.0.0.1:1", Some("t"))]);
    }

    #[test]
    fn candidates_orders_discovered_before_default_and_dedups() {
        let found = Some(pair("127.0.0.1:9000", Some("anunciado")));
        assert_eq!(
            candidates(None, None, found),
            vec![pair("127.0.0.1:9000", Some("anunciado")), pair(DEFAULT, Some("anunciado"))]
        );
        let found = Some(pair(DEFAULT, None));
        assert_eq!(candidates(None, None, found), vec![pair(DEFAULT, None)]);
    }

    #[test]
    fn candidates_without_anything_is_default() {
        assert_eq!(candidates(None, None, None), vec![pair(DEFAULT, None)]);
        assert_eq!(candidates(None, Some("t"), None), vec![pair(DEFAULT, Some("t"))]);
    }

    #[test]
    fn explicit_token_wins_over_discovered_one() {
        let found = Some(pair("127.0.0.1:9000", Some("anunciado")));
        assert_eq!(candidates(None, Some("mio"), found)[0], pair("127.0.0.1:9000", Some("mio")));
    }

    #[test]
    fn unreachable_daemon_error_has_no_token() {
        let d = HttpDaemon::new(Some("127.0.0.1:9".into()), Some("sekreto".into()));
        let err = d.get("/healthz").unwrap_err();
        assert!(!err.contains("sekreto"), "{err}");
        assert!(err.contains("daemon inalcanzable"), "{err}");
    }
}
