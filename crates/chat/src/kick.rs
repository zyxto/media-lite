//! Chat de Kick: los frames de su WebSocket (Centrifugo), el historial y los emotes `[emote:id:nombre]`.

use serde_json::Value;

use crate::{Emote, Event, Message};

/// Imagen de un emote de Kick.
pub fn emote_url(id: &str) -> String {
    format!("https://files.kick.com/emotes/{id}/fullsize")
}

/// Un frame del WebSocket: los eventos y si hay que responder con un ping.
#[derive(Debug, Default, PartialEq)]
pub struct Frame {
    pub events: Vec<Event>,
    pub ping: bool,
}

/// Lee un frame. Pueden venir varias líneas JSON, separadas por salto de línea.
pub fn parse_frame(frame: &str) -> Frame {
    let mut out = Frame::default();
    for line in frame.split('\n').filter(|l| !l.is_empty()) {
        if line == "{}" {
            out.ping = true;
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let data = &value["push"]["pub"]["data"];
        if data["event"].as_str() != Some("App\\Events\\ChatMessageEvent") {
            continue;
        }
        // `data.data` llega como texto JSON (o ya como objeto, según la versión).
        let inner = match data["data"].as_str() {
            Some(text) => serde_json::from_str(text).unwrap_or(Value::Null),
            None => data["data"].clone(),
        };
        if let Some(m) = message(&inner) {
            out.events.push(Event::Message(m));
        }
    }
    out
}

/// Un mensaje de Kick (`sender` y `content`).
pub fn message(m: &Value) -> Option<Message> {
    let content = m["content"].as_str()?;
    let (text, emotes) = emotes_in(content);
    let author = m["sender"]["username"].as_str().unwrap_or("").to_string();
    let color = m["sender"]["identity"]["color"]
        .as_str()
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    Some(Message {
        author,
        text,
        color,
        emotes,
        time_ms: None,
    })
}

/// El historial de `/api/v2/channels/<id>/messages`: la API lo da del más nuevo al más viejo.
pub fn history(json: &Value) -> Vec<Event> {
    json["data"]["messages"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .rev()
        .filter(|m| m["type"].as_str() == Some("message"))
        .filter_map(message)
        .map(Event::Message)
        .collect()
}

/// Cambia `[emote:37233:PogU]` por `PogU` y junta los emotes.
pub fn emotes_in(content: &str) -> (String, Vec<Emote>) {
    let mut text = String::with_capacity(content.len());
    let mut emotes = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("[emote:") {
        text.push_str(&rest[..start]);
        let after = &rest[start + "[emote:".len()..];
        let parsed = after.find(']').and_then(|end| {
            let (id, name) = after[..end].split_once(':')?;
            let numeric = !id.is_empty() && id.chars().all(|c| c.is_ascii_digit());
            numeric.then_some((id, name, end))
        });
        match parsed {
            Some((id, name, end)) => {
                text.push_str(name);
                emotes.push(Emote {
                    name: name.to_string(),
                    url: emote_url(id),
                });
                rest = &after[end + 1..];
            }
            None => {
                text.push_str("[emote:");
                rest = after;
            }
        }
    }
    text.push_str(rest);
    (text, emotes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn emotes_become_their_names() {
        let (text, emotes) = emotes_in("hola [emote:37233:PogU] y [emote:x:no] fin");
        assert_eq!(text, "hola PogU y [emote:x:no] fin");
        assert_eq!(emotes.len(), 1);
        assert_eq!(emotes[0].url, emote_url("37233"));
    }

    #[test]
    fn chat_event_frame_becomes_a_message() {
        let inner = json!({ "content": "buenas [emote:1:Kek]", "sender": { "username": "ana", "identity": { "color": "#00FF00" } } });
        let frame = json!({ "push": { "pub": { "data": {
            "event": "App\\Events\\ChatMessageEvent",
            "data": inner.to_string()
        }}}});
        let parsed = parse_frame(&format!("{frame}\n"));
        assert!(!parsed.ping);
        let Event::Message(m) = &parsed.events[0] else {
            panic!("se esperaba un mensaje");
        };
        assert_eq!(m.author, "ana");
        assert_eq!(m.text, "buenas Kek");
        assert_eq!(m.color.as_deref(), Some("#00FF00"));
    }

    #[test]
    fn empty_object_is_a_ping() {
        assert!(parse_frame("{}").ping);
    }

    #[test]
    fn history_is_oldest_first_and_skips_system_lines() {
        let json = json!({ "data": { "messages": [
            { "type": "message", "content": "dos", "sender": { "username": "b" } },
            { "type": "subscription", "content": "x", "sender": { "username": "c" } },
            { "type": "message", "content": "uno", "sender": { "username": "a" } }
        ]}});
        let events = history(&json);
        let texts: Vec<String> = events
            .iter()
            .map(|e| match e {
                Event::Message(m) => m.text.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(texts, vec!["uno", "dos"]);
    }
}
