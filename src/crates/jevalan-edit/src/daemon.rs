//! Cliente del daemon: validar el borrador sin publicarlo, con el parser local como caída.

use jevalan_flow::Flow;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::runtime::Runtime;

/// Tiempos del cliente: cortos, porque el editor no puede quedarse esperando a nadie.
const CONECTAR: Duration = Duration::from_millis(200);
const TOTAL: Duration = Duration::from_secs(1);

/// Veredicto de una validación contra el daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct Veredicto {
    pub ok: bool,
    /// El mensaje del `FlowError` real, tal como lo redacta el daemon.
    pub error: Option<String>,
    /// El flujo ya validado, con los valores por defecto aplicados.
    pub flow: Option<Flow>,
}

/// Cliente de las rutas de borrador. Si el daemon no responde, el editor sigue: la validación cae al
/// parser local —que es la misma autoridad (D6)— y la banda inferior lo avisa.
pub struct Cliente {
    runtime: Runtime,
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Cliente {
    /// `base` es `http://<addr>`, como lo lee `jevalan_daemon::discovery::read`.
    pub fn nuevo(base: &str, token: Option<String>) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("sin runtime para el daemon: {e}"))?;
        let http = reqwest::Client::builder()
            .connect_timeout(CONECTAR)
            .timeout(TOTAL)
            .build()
            .map_err(|e| format!("cliente HTTP inválido: {e}"))?;
        Ok(Self { runtime, http, base: base.trim_end_matches('/').to_string(), token })
    }

    /// `POST /v1/flows/validate`: el veredicto del daemon sobre el borrador. `Err` si no responde.
    pub fn validar(&self, flow: &str, yaml: &str) -> Result<Veredicto, String> {
        let respuesta = self.pedir("/v1/flows/validate", &json!({ "flow": flow, "yaml": yaml }))?;
        Ok(Veredicto {
            ok: respuesta["ok"].as_bool().unwrap_or(false),
            error: respuesta["error"].as_str().map(str::to_string),
            flow: serde_json::from_value(respuesta["flow"].clone()).ok(),
        })
    }

    /// Un POST con el token, si lo hay; error con el motivo si el daemon no contesta.
    fn pedir(&self, ruta: &str, cuerpo: &Value) -> Result<Value, String> {
        let (http, token, url) = (self.http.clone(), self.token.clone(), format!("{}{ruta}", self.base));
        self.runtime.block_on(async move {
            let mut peticion = http.post(&url).json(cuerpo);
            if let Some(token) = &token {
                peticion = peticion.bearer_auth(token);
            }
            let respuesta = peticion.send().await.map_err(|e| format!("el daemon no responde: {}", e.without_url()))?;
            let estado = respuesta.status();
            if !estado.is_success() {
                return Err(format!("el daemon contesta {estado}"));
            }
            respuesta.json::<Value>().await.map_err(|e| format!("respuesta ilegible: {e}"))
        })
    }
}

/// Servidor de mentira y puerto muerto, para las pruebas de este módulo y de quien usa el cliente.
#[cfg(test)]
pub(crate) mod pruebas {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    /// Servidor de mentira: responde `cuerpo` a cualquier POST y cierra la conexión.
    ///
    /// La petición se lee **entera** antes de contestar: un solo `read` puede traer la cabecera sin el
    /// cuerpo, y responder entonces cortaría la conexión antes de que el cliente acabara de enviarlo
    /// —`error sending request`—, que es una carrera y no lo que se quiere probar.
    pub fn servidor(cuerpo: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for conexion in listener.incoming() {
                let Ok(mut s) = conexion else { continue };
                s.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                let mut pedido = Vec::new();
                let mut trozo = [0u8; 4096];
                loop {
                    match s.read(&mut trozo) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => pedido.extend_from_slice(&trozo[..n]),
                    }
                    // Con la cabecera se sabe cuántos bytes de cuerpo faltan; se espera a tenerlos.
                    if let Some(corte) = pedido.windows(4).position(|v| v == b"\r\n\r\n") {
                        let largo = String::from_utf8_lossy(&pedido[..corte])
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_string())
                            })
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(0);
                        if pedido.len() >= corte + 4 + largo {
                            break;
                        }
                    }
                }
                let respuesta = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{cuerpo}",
                    cuerpo.len()
                );
                let _ = s.write_all(respuesta.as_bytes());
            }
        });
        url
    }

    /// Un puerto donde no escucha nadie: se abre y se suelta.
    pub fn puerto_muerto() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}")
    }
}

#[cfg(test)]
mod tests {
    use super::pruebas::{puerto_muerto, servidor};
    use super::*;

    const LOCAL: &str = include_str!("../../jevalan-flow/fixtures/tool-risk-local-v1.yaml");

    #[test]
    fn validar_lee_el_veredicto_del_daemon() {
        let cliente =
            Cliente::nuevo(&servidor(r#"{"ok":false,"error":"risks.route: expresión CEL inválida: x"}"#), None)
                .unwrap();
        let veredicto = cliente.validar("tool-risk-local", LOCAL).unwrap();
        assert!(!veredicto.ok);
        assert_eq!(veredicto.error.as_deref(), Some("risks.route: expresión CEL inválida: x"));
        assert!(veredicto.flow.is_none(), "un flujo inválido no vuelve");
    }

    #[test]
    fn validar_trae_el_flujo_cuando_el_daemon_lo_aprueba() {
        let flujo = jevalan_flow::parse("tool-risk-local", LOCAL, &["kev"], "kev").unwrap();
        let cuerpo = format!(r#"{{"ok":true,"flow":{}}}"#, serde_json::to_string(&flujo).unwrap());
        let cliente = Cliente::nuevo(&servidor(Box::leak(cuerpo.into_boxed_str())), None).unwrap();
        let veredicto = cliente.validar("tool-risk-local", LOCAL).unwrap();
        assert!(veredicto.ok);
        assert_eq!(veredicto.flow.map(|f| f.name), Some("tool-risk-local".to_string()));
    }

    #[test]
    fn sin_daemon_el_cliente_lo_dice_en_vez_de_colgarse() {
        let cliente = Cliente::nuevo(&puerto_muerto(), None).unwrap();
        let error = cliente.validar("tool-risk-local", LOCAL).unwrap_err();
        assert!(error.contains("el daemon no responde"), "{error}");
    }
}
