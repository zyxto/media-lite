//! Proxy local de playlists. Escucha solo en `127.0.0.1`, en un puerto libre, y exige un token
//! aleatorio en la ruta: otro proceso de la máquina no lo puede usar como proxy abierto.
//!
//! Pedido: `GET /<token>/playlist?u=<url>&d=<patrones>&h=<altura máxima>`, con `u`, `d` en base64url.
//! Respuesta: la playlist filtrada y reescrita (ver `playlist`).

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use media_lite_core::http::{self, Agent};
use url::Url;

use crate::playlist;

/// Tamaño máximo de una playlist. Las listas de un canal en vivo pesan algunos KB.
const MAX_PLAYLIST: u64 = 2 * 1024 * 1024;

/// Qué se le quita a la playlist antes de entregarla a mpv.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    /// Altura máxima de las variantes (la calidad elegida). `None`: sin límite.
    pub max_height: Option<u32>,
    /// Fragmentos de URL de segmentos que se quitan (anuncios).
    pub drop: Vec<String>,
    /// Quitar los cortes publicitarios de Twitch (ver `playlist::strip_twitch_ads`).
    pub twitch_ads: bool,
}

pub struct Proxy {
    port: u16,
    token: String,
}

impl Proxy {
    /// Arranca el proxy en un hilo propio. `agent` lo usa para bajar las playlists.
    pub fn start(agent: Agent) -> io::Result<Proxy> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let token = random_token()?;
        let state = Shared {
            port,
            token: token.clone(),
            agent,
        };
        thread::Builder::new()
            .name("hls-proxy".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    handle(stream, &state);
                }
            })?;
        Ok(Proxy { port, token })
    }

    /// URL para que mpv abra `upstream` a través del proxy, con los filtros indicados.
    pub fn url_for(&self, upstream: &str, filters: &Filters) -> String {
        format!(
            "http://127.0.0.1:{}/{}/playlist?{}",
            self.port,
            self.token,
            query(upstream, filters)
        )
    }
}

fn query(upstream: &str, filters: &Filters) -> String {
    let mut q = format!(
        "u={}&d={}",
        URL_SAFE_NO_PAD.encode(upstream),
        URL_SAFE_NO_PAD.encode(filters.drop.join("\n"))
    );
    if let Some(h) = filters.max_height {
        q.push_str(&format!("&h={h}"));
    }
    if filters.twitch_ads {
        q.push_str("&a=1");
    }
    q
}

struct Shared {
    port: u16,
    token: String,
    agent: Agent,
}

fn handle(stream: TcpStream, state: &Shared) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        log::debug!("proxy: una conexión llegó sin pedido");
        return;
    }
    log::debug!("proxy: pedido de mpv recibido");
    // Se descartan las cabeceras: no hacen falta.
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) if header == "\r\n" || header == "\n" => break,
            Ok(_) => {}
            Err(_) => return,
        }
    }
    let mut stream = reader.into_inner();
    let target = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    match serve(&target, state) {
        Ok(body) => {
            log::debug!("proxy: playlist entregada a mpv ({} bytes)", body.len());
            respond(&mut stream, 200, "OK", body.as_bytes());
        }
        Err(status) => {
            log::warn!("proxy: pedido de mpv rechazado con {status}");
            respond(&mut stream, status, "Error", b"");
        }
    }
}

/// Atiende un pedido. Devuelve el cuerpo, o el código HTTP de error.
fn serve(target: &str, state: &Shared) -> Result<String, u16> {
    let (path, query_string) = target.split_once('?').unwrap_or((target, ""));
    if path != format!("/{}/playlist", state.token) {
        return Err(404);
    }
    let mut upstream = None;
    let mut drop = Vec::new();
    let mut max_height = None;
    let mut twitch_ads = false;
    for pair in query_string.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "u" => upstream = decode(value),
            "d" => {
                drop = decode(value)
                    .unwrap_or_default()
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect()
            }
            "h" => max_height = value.parse::<u32>().ok(),
            "a" => twitch_ads = value == "1",
            _ => {}
        }
    }
    let upstream = upstream.ok_or(400u16)?;
    let base = Url::parse(&upstream).map_err(|_| 400u16)?;
    if base.scheme() != "http" && base.scheme() != "https" {
        return Err(400);
    }
    let text = fetch_text(&state.agent, &upstream).map_err(|e| {
        log::warn!("proxy: no se pudo bajar la playlist: {e}");
        502u16
    })?;
    let text = playlist::limit_variants(&text, max_height);
    let text = if twitch_ads {
        let (text, removed) = playlist::strip_twitch_ads(&text);
        if removed > 0 {
            log::debug!("proxy: {removed} segmentos de anuncio de Twitch quitados");
        }
        text
    } else {
        text
    };

    // Las listas hijas siguen con los mismos filtros.
    let filters = Filters {
        max_height,
        drop,
        twitch_ads,
    };
    let prefix = format!("http://127.0.0.1:{}/{}/playlist?", state.port, state.token);
    let proxy = move |child: &str| format!("{prefix}{}", query(child, &filters));
    let drop_list = decode_drop(query_string);
    Ok(playlist::rewrite(&text, &base, &drop_list, &proxy))
}

fn decode_drop(query_string: &str) -> Vec<String> {
    query_string
        .split('&')
        .filter_map(|p| p.strip_prefix("d="))
        .filter_map(decode)
        .flat_map(|s| s.lines().map(str::to_string).collect::<Vec<_>>())
        .filter(|s| !s.is_empty())
        .collect()
}

fn decode(value: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
    String::from_utf8(bytes).ok()
}

fn fetch_text(agent: &Agent, url: &str) -> Result<String, media_lite_core::Error> {
    let bytes = http::get_bytes(agent, url, MAX_PLAYLIST)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn respond(stream: &mut TcpStream, status: u16, reason: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

/// Token de 128 bits sacado de `/dev/urandom`, en hexadecimal.
fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_for_encodes_upstream_and_filters() {
        let proxy = Proxy {
            port: 4242,
            token: "abc".into(),
        };
        let filters = Filters {
            max_height: Some(480),
            drop: vec!["/ads/".to_string()],
            twitch_ads: true,
        };
        let url = proxy.url_for("https://x.test/a.m3u8?b=1", &filters);
        assert!(url.starts_with("http://127.0.0.1:4242/abc/playlist?u="));
        assert!(url.ends_with("&h=480&a=1"), "{url}");
        let u = url.split("u=").nth(1).unwrap().split('&').next().unwrap();
        assert_eq!(decode(u).as_deref(), Some("https://x.test/a.m3u8?b=1"));
        assert_eq!(
            decode_drop(url.split_once('?').unwrap().1),
            vec!["/ads/".to_string()]
        );
    }

    #[test]
    fn random_tokens_differ() {
        assert_ne!(random_token().unwrap(), random_token().unwrap());
        assert_eq!(random_token().unwrap().len(), 32);
    }

    #[test]
    fn wrong_token_is_rejected() {
        let state = Shared {
            port: 1,
            token: "secreto".into(),
            agent: http::agent(),
        };
        assert_eq!(serve("/otro/playlist?u=x", &state), Err(404));
    }
}
