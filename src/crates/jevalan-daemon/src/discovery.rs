//! Fichero de descubrimiento: cómo encuentran al daemon los clientes que no heredan su entorno (el hook de Claude Code).

use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Directorio de datos del usuario: `%LOCALAPPDATA%/jevalan` en Windows; en otro caso, `$HOME/.jevalan`.
pub fn data_dir() -> Option<PathBuf> {
    let var = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) { var("LOCALAPPDATA").map(|d| d.join("jevalan")) } else { var("HOME").map(|d| d.join(".jevalan")) }
}

/// Ruta de `daemon.json`.
pub fn path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("daemon.json"))
}

/// Contenido del fichero: `{addr, token, pid}`.
pub fn render(addr: SocketAddr, token: Option<&str>, pid: u32) -> Value {
    json!({ "addr": addr.to_string(), "token": token, "pid": pid })
}

/// Cabe señalar que el token viaja en claro porque el hook se ejecuta desde un directorio cualquiera, sin `.env`;
/// el fichero reside en el perfil del usuario. Los fallos se ignoran: es un auxilio, no una obligación.
pub fn write(addr: SocketAddr, token: Option<&str>) {
    let (Some(dir), Some(file)) = (data_dir(), path()) else { return };
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(file, render(addr, token, std::process::id()).to_string());
}

/// Retira el fichero al apagarse; si ya no existe, nada ocurre.
pub fn remove() {
    if let Some(file) = path() {
        let _ = std::fs::remove_file(file);
    }
}

/// Dirección y token de `daemon.json`; `None` si falta o no se entiende.
pub fn read() -> Option<(String, Option<String>)> {
    read_from(&path()?)
}

fn read_from(file: &Path) -> Option<(String, Option<String>)> {
    let text = std::fs::read_to_string(file).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let addr = v.get("addr")?.as_str()?.to_string();
    let token = v.get("token").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
    Some((addr, token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_has_addr_token_and_pid() {
        let v = render("127.0.0.1:8080".parse().unwrap(), Some("t"), 7);
        assert_eq!(v, json!({ "addr": "127.0.0.1:8080", "token": "t", "pid": 7 }));
        assert!(render("127.0.0.1:1".parse().unwrap(), None, 1)["token"].is_null());
    }

    #[test]
    fn read_from_parses_addr_and_token_from_file() {
        let file = std::env::temp_dir().join(format!("jevalan-discovery-test-{}.json", std::process::id()));
        std::fs::write(&file, render("127.0.0.1:9000".parse().unwrap(), Some("secreto"), 3).to_string()).unwrap();
        assert_eq!(read_from(&file), Some(("127.0.0.1:9000".into(), Some("secreto".into()))));
        std::fs::write(&file, render("127.0.0.1:9000".parse().unwrap(), Some(""), 3).to_string()).unwrap();
        assert_eq!(read_from(&file), Some(("127.0.0.1:9000".into(), None)));
        std::fs::write(&file, "no es json").unwrap();
        assert_eq!(read_from(&file), None);
        let _ = std::fs::remove_file(&file);
        assert_eq!(read_from(&file), None);
    }
}
