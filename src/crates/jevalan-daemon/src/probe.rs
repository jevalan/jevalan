//! Sondeo de `/healthz`: ¿hay un daemon de jevalan en esa dirección?

use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum Probe {
    /// Responde `ok`: es un jevalan.
    Jevalan,
    /// Nadie escucha.
    Down,
    /// Algo escucha, pero no es jevalan.
    Foreign,
}

pub async fn probe(base: &str) -> Probe {
    // Windows tarda unos 2 s en dar por rechazada una conexión local: el plazo de conexión ha de holgar.
    let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(3)).build().expect("cliente HTTP");
    match client.get(format!("{base}/healthz")).timeout(Duration::from_secs(4)).send().await {
        Ok(resp) => {
            let success = resp.status().is_success();
            if success && resp.text().await.is_ok_and(|t| t == "ok") { Probe::Jevalan } else { Probe::Foreign }
        }
        Err(e) if e.is_connect() => Probe::Down,
        Err(_) => Probe::Foreign,
    }
}

/// Espera hasta `wait` a que el daemon conteste `ok`.
pub async fn wait_healthy(base: &str, wait: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        if probe(base).await == Probe::Jevalan {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_free_port_is_down() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(probe(&format!("http://127.0.0.1:{port}")).await, Probe::Down);
    }
}
