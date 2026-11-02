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
        // Los códigos de error llegan como respuesta, para que cada plataforma decida (por ejemplo, un 401).
        .http_status_as_error(false)
        .build();
    config.into()
}

/// Respuesta completa: código y cuerpo.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Pedido con cabeceras y, opcionalmente, un cuerpo. `limit` acota la descarga en bytes.
pub fn request(
    agent: &Agent,
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
    limit: u64,
) -> Result<Response> {
    let network =
        |e: ureq::Error| Error::new(ErrorKind::Network, format!("No se pudo conectar: {e}"));
    let mut response = match (method, body) {
        ("POST", Some(text)) => {
            let mut req = agent.post(url);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            req.send(text).map_err(network)?
        }
        ("POST", None) => {
            let mut req = agent.post(url);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            req.send_empty().map_err(network)?
        }
        _ => {
            let mut req = agent.get(url);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            req.call().map_err(network)?
        }
    };
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::network(format!("Conexión cortada: {e}")))?;
    Ok(Response {
        status,
        body: bytes,
    })
}

/// GET con cabeceras. Un código de error (no 2xx) es un `Error`.
pub fn get(agent: &Agent, url: &str, headers: &[(&str, &str)], limit: u64) -> Result<Response> {
    let response = request(agent, "GET", url, headers, None, limit)?;
    if response.ok() {
        Ok(response)
    } else {
        Err(Error::new(
            ErrorKind::Network,
            format!("Error {} del servidor", response.status),
        ))
    }
}

/// Descarga el cuerpo de una URL, con un tope de `limit` bytes para no llenar la RAM.
pub fn get_bytes(agent: &Agent, url: &str, limit: u64) -> Result<Vec<u8>> {
    get(agent, url, &[], limit).map(|r| r.body)
}

/// Mensaje que se muestra cuando Kick (detrás de Cloudflare) bloquea la red. Kick no se esquiva:
/// queda desactivado hasta reiniciar la app.
pub const KICK_BLOCKED: &str =
    "Kick bloqueó el acceso desde esta red. Kick queda desactivado hasta reiniciar la app.";

/// Un desafío de Cloudflare: 403 o 503 con una página HTML en vez de JSON.
pub fn is_cloudflare_challenge(status: u16, body: &[u8]) -> bool {
    matches!(status, 403 | 503)
        && body
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|b| *b == b'<')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflare_html_is_a_challenge_and_json_is_not() {
        assert!(is_cloudflare_challenge(403, b"  <!DOCTYPE html><html>"));
        assert!(is_cloudflare_challenge(503, b"<html>"));
        assert!(!is_cloudflare_challenge(403, br#"{"message":"forbidden"}"#));
        assert!(!is_cloudflare_challenge(200, b"<html>"));
    }
}
