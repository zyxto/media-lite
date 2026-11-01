//! HTTP bloqueante con ureq + rustls (sin OpenSSL, para que el AppImage ande en Debian 11 y 12).

use std::io::Read;
use std::time::Duration;

use crate::error::{Error, ErrorKind, Result};

/// Agente de navegador liviano. Las plataformas que piden un user agent propio lo cambian en su cliente.
pub const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0 media-lite/0.1";

pub type Agent = ureq::Agent;

pub fn agent() -> Agent {
    let config = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(USER_AGENT)
        .build();
    config.into()
}

/// Descarga el cuerpo de una URL, con un tope de `limit` bytes para no llenar la RAM.
pub fn get_bytes(agent: &Agent, url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| Error::new(ErrorKind::Network, format!("No se pudo conectar: {e}")))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::network(format!("Conexión cortada: {e}")))?;
    Ok(bytes)
}
