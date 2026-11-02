//! Kick: portada de directos, búsqueda, canales y grabaciones, por la API REST de Kick.
//! Portado de `Sanson-FLinux/plugins/kick/script.js`.
//!
//! Kick está detrás de Cloudflare. Si responde con un desafío HTML, **no se esquiva** (nada de
//! imitar huellas TLS): se avisa y Kick queda desactivado en esta sesión.

use std::sync::atomic::{AtomicBool, Ordering};

use media_lite_core::http::{self, Agent};
use media_lite_core::{
    Caps, Details, Error, ErrorKind, Item, Kind, Page, Platform, Result, Season, Section, Source,
};
use serde_json::Value;

use crate::util::{jarr, jnum, jstr};
use crate::Provider;

const KICK: &str = "https://kick.com";
const WEB_API: &str = "https://web.kick.com";
const REFERER: &str = "https://kick.com/";
const PAGE: u64 = 24;
const MAX_PAGES: u64 = 10;
const MAX_JSON: u64 = 4 * 1024 * 1024;
pub const BLOCKED: &str =
    "Kick bloqueó el acceso desde esta red. Kick queda desactivado hasta reiniciar la app.";

pub struct Kick {
    agent: Agent,
    blocked: AtomicBool,
}

impl Default for Kick {
    fn default() -> Kick {
        Kick::new()
    }
}

impl Kick {
    pub fn new() -> Kick {
        Kick {
            agent: http::agent(),
            blocked: AtomicBool::new(false),
        }
    }

    /// GET JSON. Un desafío de Cloudflare desactiva Kick por la sesión; un corte de conexión se
    /// reintenta una vez.
    fn get(&self, url: &str) -> Result<Value> {
        if self.blocked.load(Ordering::Relaxed) {
            return Err(blocked_error());
        }
        let headers = [("Accept", "application/json"), ("Referer", REFERER)];
        let send = || http::request(&self.agent, "GET", url, &headers, None, MAX_JSON);
        let response = send().or_else(|_| send())?;
        if is_challenge(response.status, &response.body) {
            self.blocked.store(true, Ordering::Relaxed);
            return Err(blocked_error());
        }
        if response.status == 404 {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "Kick no encontró este contenido",
            ));
        }
        if !response.ok() {
            return Err(Error::network(format!(
                "Kick respondió con error {}",
                response.status
            )));
        }
        serde_json::from_slice(&response.body)
            .map_err(|_| Error::parse("Kick: la respuesta no se entiende"))
    }

    fn channel(&self, slug: &str) -> Result<Value> {
        self.get(&format!("{KICK}/api/v2/channels/{slug}"))
    }

    fn live_details(&self, slug: &str) -> Result<Details> {
        let channel = self.channel(slug)?;
        let name = username(&channel);
        if channel["livestream"].is_null() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                format!("{name} no está en vivo ahora."),
            ));
        }
        let playback = jstr(&channel, "playback_url");
        if playback.is_empty() {
            return Err(Error::network("Kick no entregó el directo"));
        }
        Ok(Details {
            item: live_item(&channel["livestream"], &channel),
            description: jstr(&channel["user"], "bio"),
            sources: vec![Source {
                url: playback,
                label: "Directo".into(),
                height: None,
                live: true,
                audio_url: None,
                drop: Vec::new(),
                twitch_ads: false,
            }],
        })
    }

    fn vod_details(&self, uuid: &str) -> Result<Details> {
        let video = self.get(&format!("{KICK}/api/v1/video/{uuid}"))?;
        let source = jstr(&video, "source");
        if source.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "La grabación no está disponible.",
            ));
        }
        let stream = &video["livestream"];
        let owner = &stream["channel"]["user"];
        Ok(Details {
            item: vod_item(&vod_fields(&video), owner, uuid),
            description: jstr(owner, "bio"),
            sources: vec![Source {
                url: source,
                label: "Grabación".into(),
                height: None,
                live: false,
                audio_url: None,
                drop: Vec::new(),
                twitch_ads: false,
            }],
        })
    }

    /// Las grabaciones del canal: la lista nueva de la API v2 o, si falla, `previous_livestreams`.
    fn vods(&self, slug: &str, channel: &Value) -> Vec<Item> {
        let owner = &channel["user"];
        match self.get(&format!("{KICK}/api/v2/channels/{slug}/videos")) {
            Ok(list) => {
                let entries = list
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| list["data"].as_array().cloned().unwrap_or_default());
                entries
                    .iter()
                    .map(|v| vod_item(v, owner, &uuid_of(v)))
                    .collect()
            }
            Err(_) => match self.get(&format!("{KICK}/api/v1/channels/{slug}")) {
                Ok(old) => jarr(&old, "previous_livestreams")
                    .iter()
                    .map(|v| vod_item(v, owner, &uuid_of(v)))
                    .collect(),
                Err(_) => Vec::new(),
            },
        }
    }
}

impl Provider for Kick {
    fn platform(&self) -> Platform {
        Platform::Kick
    }

    fn caps(&self) -> Caps {
        Caps {
            live: true,
            search: true,
            ..Caps::default()
        }
    }

    fn list(
        &self,
        section: Section,
        _category: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Page> {
        if section != Section::Live {
            return Err(Error::unsupported());
        }
        let page: u64 = cursor.and_then(|c| c.parse().ok()).unwrap_or(1);
        let url = format!(
            "{WEB_API}/api/v1/livestreams?page={page}&limit={PAGE}&sort=featured_home&language=en"
        );
        let json = self.get(&url)?;
        let list = livestreams(&json);
        let next = (list.len() as u64 >= PAGE && page < MAX_PAGES).then(|| (page + 1).to_string());
        Ok(Page {
            items: list.iter().map(|s| live_item(s, &Value::Null)).collect(),
            next,
        })
    }

    fn search(&self, query: &str, _cursor: Option<&str>) -> Result<Page> {
        let url = format!(
            "{KICK}/api/search?searched_word={}",
            crate::util::pct_encode(query)
        );
        let json = self.get(&url)?;
        let channels: Vec<&Value> = jarr(&json, "channels")
            .iter()
            .filter(|c| !jstr(c, "slug").is_empty())
            .collect();
        let mut items: Vec<Item> = channels.iter().map(|c| channel_item(c)).collect();
        items.extend(
            channels
                .iter()
                .filter(|c| {
                    c["livestream"]["is_live"].as_bool() == Some(true)
                        || c["livestream"]["id"].is_number()
                })
                .map(|c| live_item(&c["livestream"], c)),
        );
        Ok(Page { items, next: None })
    }

    fn details(&self, item: &Item) -> Result<Details> {
        match item.kind {
            Kind::Live => self.live_details(&item.id),
            Kind::Video => self.vod_details(&item.id),
            _ => Err(Error::unsupported()),
        }
    }

    /// Un canal se abre como una serie: «En vivo» (si está en directo) y «Transmisiones anteriores».
    fn seasons(&self, channel_item: &Item) -> Result<Vec<Season>> {
        if channel_item.kind != Kind::Series {
            return Err(Error::unsupported());
        }
        let slug = channel_item.id.as_str();
        let channel = self.channel(slug)?;
        let mut seasons = Vec::new();
        if !channel["livestream"].is_null() {
            seasons.push(Season {
                number: 1,
                name: "En vivo".into(),
                episodes: vec![live_item(&channel["livestream"], &channel)],
            });
        }
        let vods = self.vods(slug, &channel);
        if !vods.is_empty() {
            seasons.push(Season {
                number: seasons.len() as u32 + 1,
                name: "Transmisiones anteriores".into(),
                episodes: vods,
            });
        }
        if seasons.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                format!(
                    "{} no está en vivo ni tiene transmisiones guardadas.",
                    username(&channel)
                ),
            ));
        }
        Ok(seasons)
    }
}

fn blocked_error() -> Error {
    Error::network(BLOCKED)
}

/// Un desafío de Cloudflare: 403 o 503 con una página HTML.
pub fn is_challenge(status: u16, body: &[u8]) -> bool {
    matches!(status, 403 | 503)
        && body
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|b| *b == b'<')
}

/// Lista de directos: `data.livestreams`, o `data` si viene directo.
fn livestreams(json: &Value) -> Vec<Value> {
    let list = if json["data"]["livestreams"].is_array() {
        &json["data"]["livestreams"]
    } else {
        &json["data"]
    };
    list.as_array().cloned().unwrap_or_default()
}

/// Nombre de usuario de un canal.
fn username(channel: &Value) -> String {
    let name = jstr(&channel["user"], "username");
    if name.is_empty() {
        jstr(channel, "slug")
    } else {
        name
    }
}

/// Miniatura: `thumbnail.src`, `thumbnail.url` o `thumbnail` si es texto.
fn thumbnail(v: &Value) -> Option<String> {
    let url = ["src", "url"]
        .iter()
        .map(|k| jstr(&v["thumbnail"], k))
        .find(|u| !u.is_empty())
        .or_else(|| v["thumbnail"].as_str().map(str::to_string))?;
    (!url.is_empty()).then_some(url)
}

/// Un directo. `stream` es el livestream; `channel`, el canal (si el livestream no lo trae).
pub fn live_item(stream: &Value, channel: &Value) -> Item {
    let c = if stream["channel"].is_object() {
        &stream["channel"]
    } else {
        channel
    };
    let slug = jstr(c, "slug");
    let name = username(c);
    let title = [jstr(stream, "session_title"), jstr(stream, "title")]
        .into_iter()
        .find(|t| !t.is_empty())
        .unwrap_or_else(|| name.clone());
    let mut item = Item::new(Platform::Kick, Kind::Live, slug, title);
    item.subtitle = name;
    item.thumb = thumbnail(stream);
    item.live = true;
    let viewers = jnum(stream, "viewer_count").max(jnum(stream, "viewers"));
    if viewers > 0.0 {
        item.description = format!("{} espectadores", viewers as u64);
    }
    item
}

/// Una grabación. `video` tiene los campos del livestream (título, miniatura, duración en
/// milisegundos y vistas); `owner` es el usuario del canal.
pub fn vod_item(video: &Value, owner: &Value, uuid: &str) -> Item {
    let mut item = Item::new(
        Platform::Kick,
        Kind::Video,
        uuid,
        jstr(video, "session_title"),
    );
    item.subtitle = if owner.is_object() {
        jstr(owner, "username")
    } else {
        String::new()
    };
    item.thumb = thumbnail(video);
    let duration_ms = jnum(video, "duration");
    if duration_ms > 0.0 {
        item.duration_secs = Some((duration_ms / 1000.0) as u64);
    }
    let views = jnum(video, "views").max(jnum(video, "viewer_count"));
    if views > 0.0 {
        item.description = format!("{} vistas", views as u64);
    }
    item
}

/// Un canal.
pub fn channel_item(c: &Value) -> Item {
    let slug = jstr(c, "slug");
    // Como en Twitch, un canal es una serie con temporadas (ver `twitch::channel_item`).
    let mut item = Item::new(Platform::Kick, Kind::Series, slug.clone(), username(c));
    let pic = jstr(&c["user"], "profile_pic");
    item.thumb = (!pic.is_empty()).then_some(pic);
    if let Some(n) = c["followers_count"].as_u64() {
        item.subtitle = format!("{n} seguidores");
    }
    item
}

/// El uuid de una grabación: `video.uuid` o `uuid`.
fn uuid_of(v: &Value) -> String {
    let uuid = jstr(&v["video"], "uuid");
    if uuid.is_empty() {
        jstr(v, "uuid")
    } else {
        uuid
    }
}

/// Campos de una grabación de `/api/v1/video/<uuid>`: los del livestream más las vistas.
fn vod_fields(video: &Value) -> Value {
    let mut fields = video["livestream"].clone();
    if !fields.is_object() {
        fields = Value::Object(Default::default());
    }
    fields["views"] = video["views"].clone();
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn channel() -> Value {
        json!({
            "slug": "xqc", "id": 7,
            "user": { "username": "xQc", "profile_pic": "https://img/x.png", "bio": "hola" },
            "followers_count": 900,
            "playback_url": "https://stream/x.m3u8",
            "livestream": {
                "id": 1, "session_title": "Directo", "viewer_count": 12000,
                "thumbnail": { "src": "https://img/t.jpg" }
            }
        })
    }

    #[test]
    fn live_item_takes_the_channel_slug_as_id() {
        let c = channel();
        let item = live_item(&c["livestream"], &c);
        assert_eq!(item.kind, Kind::Live);
        assert_eq!(item.id, "xqc");
        assert_eq!(item.title, "Directo");
        assert_eq!(item.subtitle, "xQc");
        assert_eq!(item.thumb.as_deref(), Some("https://img/t.jpg"));
        assert_eq!(item.description, "12000 espectadores");
    }

    #[test]
    fn list_entries_carry_their_own_channel() {
        let json = json!({ "data": { "livestreams": [
            { "session_title": "A", "channel": { "slug": "a", "user": { "username": "Ana" } } }
        ]}});
        let list = livestreams(&json);
        assert_eq!(list.len(), 1);
        assert_eq!(live_item(&list[0], &Value::Null).id, "a");
    }

    #[test]
    fn cloudflare_html_is_a_challenge_and_json_is_not() {
        assert!(is_challenge(403, b"  <!DOCTYPE html><html>"));
        assert!(is_challenge(503, b"<html>"));
        assert!(!is_challenge(403, br#"{"message":"forbidden"}"#));
        assert!(!is_challenge(200, b"<html>"));
    }

    #[test]
    fn vod_uses_the_video_uuid() {
        let v = json!({ "video": { "uuid": "abc-123" }, "uuid": "other", "session_title": "Stream", "duration": 3600000 });
        assert_eq!(uuid_of(&v), "abc-123");
        let item = vod_item(&v, &Value::Null, &uuid_of(&v));
        assert_eq!(item.id, "abc-123");
        assert_eq!(item.duration_secs, Some(3600));
    }

    #[test]
    fn channel_item_has_slug_and_followers() {
        let item = channel_item(&channel());
        assert_eq!(item.kind, Kind::Series);
        assert_eq!(item.id, "xqc");
        assert_eq!(item.subtitle, "900 seguidores");
    }
}
