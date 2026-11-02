//! Repetición del chat de una grabación: los mensajes del tramo del video que se está viendo.
//! Se piden a medida que avanza `time-pos`, en ventanas, y se guardan solo unos segundos atrás.
//! Portado de `TwitchReplayPager` y `KickReplayPager` en Sanson.

use media_lite_core::http::{self, Agent};
use media_lite_core::time::parse_iso;
use serde_json::{json, Value};

use crate::irc::emote_url;
use crate::live::get_json;
use crate::{kick as kick_chat, Emote, Message};

const GQL: &str = "https://gql.twitch.tv/gql";
const CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
const SITE: &str = "https://www.twitch.tv";
const KICK: &str = "https://kick.com";
const LIMIT: u64 = 4 * 1024 * 1024;
/// Mensajes de Kick por pedido (la API da 25 por página).
const KICK_PAGE: usize = 25;
/// Cuánto se pide de Kick de una vez, en ms del video.
const KICK_STEP_MS: u64 = 8000;
/// Hasta cuántos ms por delante del video se mantiene el chat leído.
const AHEAD_MS: u64 = 5000;
/// Cuánto se guarda de atrás, en ms del video.
const BEHIND_MS: u64 = 1000;

enum Source {
    Twitch { video: String },
    Kick { channel_id: String, start_ms: u64 },
}

/// Chat de una grabación, por tiempo del video.
pub struct Replay {
    source: Source,
    agent: Agent,
    /// Mensajes con su id (para no repetirlos), ordenados por tiempo.
    cache: Vec<(String, Message)>,
    /// Hasta qué tiempo del video ya se pidió (ms). `None` si no se pidió nada.
    covered_from: Option<u64>,
    covered_to: Option<u64>,
}

impl Replay {
    /// Chat de una grabación de Twitch (id del video).
    pub fn twitch(video_id: &str) -> Replay {
        Replay::new(Source::Twitch {
            video: video_id.to_string(),
        })
    }

    /// Chat de una grabación de Kick (uuid). Pide la fecha de inicio del directo.
    pub fn kick(video_uuid: &str) -> Result<Replay, String> {
        let agent = http::agent();
        let video = get_json(&agent, &format!("{KICK}/api/v1/video/{video_uuid}"))?;
        let stream = &video["livestream"];
        let channel_id = stream["channel_id"].to_string();
        let start_ms = iso_ms(stream["start_time"].as_str().unwrap_or(""));
        if channel_id == "null" || start_ms == 0 {
            return Err("No se pudo leer la fecha de la grabación de Kick".into());
        }
        Ok(Replay::new(Source::Kick {
            channel_id,
            start_ms,
        }))
    }

    fn new(source: Source) -> Replay {
        Replay {
            source,
            agent: http::agent(),
            cache: Vec::new(),
            covered_from: None,
            covered_to: None,
        }
    }

    /// Los mensajes desde un segundo antes de `at` (ms del video) hasta lo que ya se leyó. Pide
    /// lo que falta; si el usuario saltó a otro tramo, empieza de nuevo.
    pub fn around(&mut self, at: u64) -> Vec<Message> {
        let jumped = match (self.covered_from, self.covered_to) {
            (Some(from), Some(to)) => at < from || at > to + 1000,
            _ => true,
        };
        if jumped {
            self.cache.clear();
            self.covered_from = Some(at);
            self.covered_to = Some(at.saturating_sub(1));
        }
        self.cache
            .retain(|(_, m)| m.time_ms.unwrap_or(0) + BEHIND_MS >= at);
        for _ in 0..3 {
            if self.covered_to.unwrap_or(0) >= at + AHEAD_MS {
                break;
            }
            if let Err(e) = self.fetch_next(at) {
                log::warn!("chat de la grabación: {e}");
                break;
            }
        }
        self.cache.iter().map(|(_, m)| m.clone()).collect()
    }

    fn fetch_next(&mut self, at: u64) -> Result<(), String> {
        let to = self.covered_to.unwrap_or(0);
        match &self.source {
            Source::Twitch { video } => {
                let secs = (to + 1) / 1000;
                let found = self.twitch_comments(video, secs)?;
                let last = found.iter().filter_map(|(_, m)| m.time_ms).max();
                self.add(found);
                // Un tramo vacío: sin comentarios por medio minuto (como Sanson).
                let next = match last {
                    Some(t) if t >= secs * 1000 => t + 999,
                    _ => secs * 1000 + 30_000,
                };
                self.covered_to = Some((to + 1000).max(next));
            }
            Source::Kick {
                channel_id,
                start_ms,
            } => {
                let (channel_id, start_ms) = (channel_id.clone(), *start_ms);
                let window_end = (to + KICK_STEP_MS).max(at + AHEAD_MS);
                let found = self.kick_window(&channel_id, start_ms, to, window_end)?;
                self.add(found);
                self.covered_to = Some(window_end);
            }
        }
        Ok(())
    }

    /// Agrega los mensajes que no estén, y los deja ordenados por tiempo.
    fn add(&mut self, found: Vec<(String, Message)>) {
        for (id, message) in found {
            if !self.cache.iter().any(|(old, _)| *old == id) {
                self.cache.push((id, message));
            }
        }
        self.cache.sort_by_key(|(_, m)| m.time_ms.unwrap_or(0));
    }

    /// Comentarios de Twitch desde un segundo del video (una página).
    fn twitch_comments(&self, video: &str, secs: u64) -> Result<Vec<(String, Message)>, String> {
        const QUERY: &str = "query($id:ID!,$o:Int){video(id:$id){comments(contentOffsetSeconds:$o){edges{node{id contentOffsetSeconds commenter{displayName login} message{fragments{text emote{emoteID}} userColor}}}}}}";
        let headers = [
            ("Client-Id", CLIENT_ID),
            ("Content-Type", "application/json"),
            ("Origin", SITE),
            ("Referer", "https://www.twitch.tv/"),
        ];
        let body = json!({ "query": QUERY, "variables": { "id": video, "o": secs } }).to_string();
        let response = http::request(&self.agent, "POST", GQL, &headers, Some(&body), LIMIT)
            .map_err(|e| e.message)?;
        if !response.ok() {
            return Err(format!("Twitch respondió con error {}", response.status));
        }
        let value: Value = serde_json::from_slice(&response.body)
            .map_err(|_| "Twitch: la respuesta no se entiende".to_string())?;
        let edges = value["data"]["video"]["comments"]["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(edges
            .iter()
            .filter_map(|e| twitch_comment(&e["node"]))
            .collect())
    }

    /// Mensajes de Kick entre `from` y `to` (ms del video), leyendo desde el final hacia atrás.
    fn kick_window(
        &self,
        channel_id: &str,
        start_ms: u64,
        from: u64,
        to: u64,
    ) -> Result<Vec<(String, Message)>, String> {
        let mut out = Vec::new();
        // La API pide el cursor en microsegundos y devuelve los mensajes anteriores a él.
        let mut cursor = (start_ms + to) * 1000;
        for _ in 0..6 {
            let url = format!("{KICK}/api/v2/channels/{channel_id}/messages?cursor={cursor}");
            let page = get_json(&self.agent, &url)?;
            let list = page["data"]["messages"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let mut oldest = u64::MAX;
            for m in &list {
                let t = iso_ms(m["created_at"].as_str().unwrap_or("")).saturating_sub(start_ms);
                oldest = oldest.min(t);
                if m["type"].as_str() != Some("message") || t < from || t >= to {
                    continue;
                }
                if let Some(mut message) = kick_chat::message(m) {
                    message.time_ms = Some(t);
                    out.push((m["id"].to_string(), message));
                }
            }
            let next = page["data"]["cursor"].as_u64();
            if list.len() < KICK_PAGE || oldest < from || next.is_none() {
                break;
            }
            cursor = next.unwrap_or(cursor);
        }
        Ok(out)
    }
}

/// Un comentario de Twitch: el texto con sus emotes, el autor y el momento del video.
fn twitch_comment(node: &Value) -> Option<(String, Message)> {
    let id = node["id"].as_str()?.to_string();
    let mut text = String::new();
    let mut emotes: Vec<Emote> = Vec::new();
    for fragment in node["message"]["fragments"].as_array()? {
        let piece = fragment["text"].as_str().unwrap_or("");
        if let Some(emote_id) = fragment["emote"]["emoteID"].as_str() {
            emotes.push(Emote {
                name: piece.to_string(),
                url: emote_url(emote_id),
            });
        }
        text.push_str(piece);
    }
    let author = node["commenter"]["displayName"]
        .as_str()
        .or_else(|| node["commenter"]["login"].as_str())
        .unwrap_or("")
        .to_string();
    let message = Message {
        author,
        text: text.trim().to_string(),
        color: node["message"]["userColor"]
            .as_str()
            .filter(|c| !c.is_empty())
            .map(str::to_string),
        emotes,
        time_ms: Some((node["contentOffsetSeconds"].as_f64().unwrap_or(0.0) * 1000.0) as u64),
    };
    Some((id, message))
}

/// Milisegundos desde la época de una fecha ISO-8601 (con o sin fracción de segundo).
fn iso_ms(text: &str) -> u64 {
    let secs = parse_iso(text);
    if secs == 0 {
        return 0;
    }
    let fraction = text
        .split_once('.')
        .map(|(_, rest)| {
            rest.chars()
                .take_while(|c| c.is_ascii_digit())
                .take(3)
                .collect::<String>()
        })
        .unwrap_or_default();
    let millis = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<3}").parse::<u64>().unwrap_or(0)
    };
    secs * 1000 + millis
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates_keep_their_milliseconds() {
        assert_eq!(iso_ms("2026-11-01T11:30:00.250000Z"), 1_793_532_600_250);
        assert_eq!(iso_ms("2026-11-01T11:30:00Z"), 1_793_532_600_000);
        assert_eq!(iso_ms("nada"), 0);
    }

    #[test]
    fn twitch_comment_marks_emotes_and_offset() {
        let node = json!({
            "id": "c1", "contentOffsetSeconds": 12.5,
            "commenter": { "displayName": "Ana", "login": "ana" },
            "message": { "fragments": [
                { "text": "hola ", "emote": null },
                { "text": "Kappa", "emote": { "emoteID": "25" } }
            ], "userColor": "#FF00FF" }
        });
        let (id, m) = twitch_comment(&node).unwrap();
        assert_eq!(id, "c1");
        assert_eq!(m.author, "Ana");
        assert_eq!(m.text, "hola Kappa");
        assert_eq!(m.time_ms, Some(12_500));
        assert_eq!(m.emotes[0].name, "Kappa");
        assert_eq!(m.color.as_deref(), Some("#FF00FF"));
    }
}
