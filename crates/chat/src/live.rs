//! Conexiones en vivo: un hilo por chat. Los eventos llegan por un canal que se vacía con
//! `drain`. Soltar el `LiveChat` avisa al hilo, que cierra el socket en el próximo sondeo.

use std::fs::File;
use std::io::{self, Read};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{unbounded, Receiver, Sender};
use media_lite_core::http::{self, Agent, Response};
use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message as WsMessage, WebSocket};

use crate::irc::{self, Line};
use crate::kick;
use crate::Event;

/// Cada cuánto el hilo revisa si lo soltaron (el socket espera este tiempo por cada lectura).
const POLL: Duration = Duration::from_millis(400);
const LIMIT: u64 = 4 * 1024 * 1024;
const KICK_REFERER: &str = "https://kick.com/";
const REALTIME: &str = "https://web.kick.com/api/v1/realtime";

/// Un chat conectado. Se cierra al soltarlo.
pub struct LiveChat {
    rx: Receiver<Event>,
    stop: Arc<AtomicBool>,
}

impl LiveChat {
    /// Chat de un canal de Twitch (por su login), anónimo y de solo lectura.
    pub fn twitch(login: &str) -> LiveChat {
        let login = login.to_string();
        spawn("chat-twitch", move |tx, stop| {
            run_twitch(&login, &tx, &stop)
        })
    }

    /// Chat de un canal de Kick (por su slug).
    pub fn kick(slug: &str) -> LiveChat {
        let slug = slug.to_string();
        spawn("chat-kick", move |tx, stop| run_kick(&slug, &tx, &stop))
    }

    /// Los eventos que llegaron desde la última vez.
    pub fn drain(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }
}

impl Drop for LiveChat {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn spawn<F>(name: &str, work: F) -> LiveChat
where
    F: FnOnce(Sender<Event>, Arc<AtomicBool>) -> Result<(), String> + Send + 'static,
{
    let (tx, rx) = unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let spawned = thread::Builder::new().name(name.into()).spawn(move || {
        let end = match work(tx.clone(), flag) {
            Ok(()) => "El chat se cerró".to_string(),
            Err(e) => e,
        };
        // Si ya se soltó el chat, nadie escucha: el error no importa.
        let _ = tx.send(Event::Closed(end));
    });
    if spawned.is_err() {
        log::warn!("chat: no se pudo crear el hilo");
    }
    LiveChat { rx, stop }
}

/// Abre el WebSocket y deja corto el tiempo de lectura, para revisar `stop`.
fn open(request: impl IntoClientRequest) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
    let (mut socket, _) =
        tungstenite::connect(request).map_err(|e| format!("No se pudo abrir el chat: {e}"))?;
    if let MaybeTlsStream::Rustls(stream) = socket.get_mut() {
        let _ = stream.get_ref().set_read_timeout(Some(POLL));
    }
    Ok(socket)
}

fn send(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>, text: &str) -> Result<(), String> {
    socket
        .send(WsMessage::text(text))
        .map_err(|e| format!("El chat se cortó: {e}"))
}

fn timed_out(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
}

fn run_twitch(login: &str, tx: &Sender<Event>, stop: &AtomicBool) -> Result<(), String> {
    // Un usuario anónimo: `justinfan` y un número, como hace el cliente web sin sesión.
    let nick = format!("justinfan{}", 1000 + random_bytes()[0] as u32 * 256 % 89000);
    let mut socket = open("wss://irc-ws.chat.twitch.tv")?;
    send(&mut socket, "CAP REQ :twitch.tv/tags twitch.tv/commands")?;
    send(&mut socket, "PASS SCHMOOPIIE")?;
    send(&mut socket, &format!("NICK {nick}"))?;
    send(&mut socket, &format!("JOIN #{login}"))?;
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = socket.close(None);
            return Ok(());
        }
        match socket.read() {
            Ok(WsMessage::Text(text)) => {
                for line in text.as_str().lines() {
                    match irc::parse_line(line.trim_end()) {
                        Line::Reply(reply) => send(&mut socket, &reply)?,
                        Line::Event(event) => {
                            if tx.send(event).is_err() {
                                return Ok(());
                            }
                        }
                        Line::Ignored => {}
                    }
                }
            }
            Ok(WsMessage::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(e) if timed_out(&e) => {}
            Err(e) => return Err(format!("El chat se cortó: {e}")),
        }
    }
}

fn run_kick(slug: &str, tx: &Sender<Event>, stop: &AtomicBool) -> Result<(), String> {
    let agent = http::agent();
    let channel = get_json(&agent, &format!("https://kick.com/api/v2/channels/{slug}"))?;
    let channel_id = channel["id"].to_string();
    let room = channel["chatroom"]["id"].to_string();
    let history = get_json(
        &agent,
        &format!("https://kick.com/api/v2/channels/{channel_id}/messages"),
    )?;
    for event in kick::history(&history) {
        if tx.send(event).is_err() {
            return Ok(());
        }
    }

    // La conexión en tiempo real: primero un id de cliente, después su conexión y su token.
    let client = random_id();
    let connection = post_json(
        &agent,
        &format!("{REALTIME}/channels/{channel_id}/chat/connection"),
        &json!({
            "client": { "id": client, "type": "web" },
            "capabilities": { "accepted_providers": [{ "provider": "centrifugo" }] }
        }),
    )?;
    let url = connection["data"]["connections"]
        .as_array()
        .and_then(|list| list.iter().find(|c| c["provider"] == "centrifugo"))
        .and_then(|c| c["credentials"]["url"].as_str())
        .ok_or("Kick no dio la dirección del chat")?
        .to_string();
    let auth = post_json(
        &agent,
        &format!("{REALTIME}/auth/connection"),
        &json!({ "client_id": client }),
    )?;
    let token = auth["data"]["token"]
        .as_str()
        .ok_or("Kick no dio el token del chat")?
        .to_string();

    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| format!("Dirección de chat no válida: {e}"))?;
    request
        .headers_mut()
        .insert("Origin", HeaderValue::from_static("https://kick.com"));
    let mut socket = open(request)?;
    send(
        &mut socket,
        &format!(
            "{}\n{}",
            json!({ "connect": { "token": token, "name": "js" }, "id": 1 }),
            json!({ "subscribe": { "channel": format!("chatrooms.{room}.v2") }, "id": 2 })
        ),
    )?;
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = socket.close(None);
            return Ok(());
        }
        match socket.read() {
            Ok(WsMessage::Text(text)) => {
                let frame = kick::parse_frame(text.as_str());
                if frame.ping {
                    send(&mut socket, "{}")?;
                }
                for event in frame.events {
                    if tx.send(event).is_err() {
                        return Ok(());
                    }
                }
            }
            Ok(WsMessage::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(e) if timed_out(&e) => {}
            Err(e) => return Err(format!("El chat se cortó: {e}")),
        }
    }
}

/// Respuesta JSON de Kick. Un desafío de Cloudflare se avisa como bloqueo.
fn check(response: Response) -> Result<Value, String> {
    if http::is_cloudflare_challenge(response.status, &response.body) {
        return Err(http::KICK_BLOCKED.to_string());
    }
    if !response.ok() {
        return Err(format!("Kick respondió con error {}", response.status));
    }
    serde_json::from_slice(&response.body).map_err(|_| "Kick: la respuesta no se entiende".into())
}

fn kick_headers() -> [(&'static str, &'static str); 3] {
    [
        ("Accept", "application/json"),
        ("Content-Type", "application/json"),
        ("Referer", KICK_REFERER),
    ]
}

pub(crate) fn get_json(agent: &Agent, url: &str) -> Result<Value, String> {
    let headers = kick_headers();
    let response =
        http::request(agent, "GET", url, &headers, None, LIMIT).map_err(|e| e.message)?;
    check(response)
}

fn post_json(agent: &Agent, url: &str, body: &Value) -> Result<Value, String> {
    let headers = kick_headers();
    let text = body.to_string();
    let response =
        http::request(agent, "POST", url, &headers, Some(&text), LIMIT).map_err(|e| e.message)?;
    check(response)
}

fn random_bytes() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    let read = File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if read.is_err() {
        // Sin /dev/urandom (no debería pasar en Linux): la hora basta para un id de cliente.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        bytes = nanos.to_le_bytes().repeat(2)[..16]
            .try_into()
            .unwrap_or([7; 16]);
    }
    bytes
}

fn random_id() -> String {
    random_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_ids_differ_and_have_the_right_size() {
        let a = random_id();
        let b = random_id();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn dropping_the_chat_sets_the_stop_flag() {
        let (tx, rx) = unbounded::<Event>();
        let stop = Arc::new(AtomicBool::new(false));
        let chat = LiveChat {
            rx,
            stop: Arc::clone(&stop),
        };
        drop(tx);
        drop(chat);
        assert!(stop.load(Ordering::Relaxed));
    }
}
