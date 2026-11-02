//! YouTube Music: canciones, álbumes y artistas, sin cuenta. Usa el cliente `WEB_REMIX` de innertube
//! y el mismo solucionador que el video, pero solo pide el audio. Portado de
//! `Sanson-FLinux/plugins/music/script.js`.
//!
//! Un álbum y un artista se abren como serie (sus canciones son las «temporadas»), igual que los canales.

use std::path::Path;
use std::sync::Arc;

use media_lite_core::{
    Caps, Details, Error, ErrorKind, Item, Kind, Page, Platform, Result, Season, Section, Source,
};
use serde_json::{json, Value};

use crate::util::jstr;
use crate::youtube::catalog::{find_key, text};
use crate::youtube::innertube::Innertube;
use crate::youtube::resolve::Resolver;
use crate::Provider;

const MUSIC: &str = "https://music.youtube.com";
/// Filtro de «canciones» de la búsqueda (el parámetro `params`, como lo usa Sanson).
const SONGS_FILTER: &str = "EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D";
const HOME: &str = "FEmusic_home";

pub struct YouTubeMusic {
    api: Arc<Innertube>,
    resolver: Arc<Resolver>,
}

impl YouTubeMusic {
    pub fn new(config_dir: &Path) -> YouTubeMusic {
        let api = Arc::new(Innertube::new(config_dir));
        let exe = std::env::current_exe().unwrap_or_default();
        let resolver = Arc::new(Resolver::new(
            config_dir,
            &media_lite_core::paths::cache_dir(),
            Arc::clone(&api),
            exe,
        ));
        YouTubeMusic { api, resolver }
    }

    /// Un pedido a YouTube Music: `browse`, `search` o `next`.
    fn post(&self, endpoint: &str, body: Value) -> Result<Value> {
        self.api.post_as("ytmusic", MUSIC, endpoint, body)
    }

    /// Una página: sus elementos y el token de la siguiente.
    fn page(&self, endpoint: &str, body: Value, cursor: Option<&str>) -> Result<Page> {
        let body = match cursor {
            Some(token) => json!({ "continuation": token }),
            None => body,
        };
        let response = self.post(endpoint, body)?;
        Ok(Page {
            items: items(&response),
            next: continuation(&response),
        })
    }
}

impl Provider for YouTubeMusic {
    fn platform(&self) -> Platform {
        Platform::YouTubeMusic
    }

    fn caps(&self) -> Caps {
        Caps {
            music: true,
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
        match section {
            Section::Music => self.page("browse", json!({ "browseId": HOME }), cursor),
            _ => Err(Error::unsupported()),
        }
    }

    fn search(&self, query: &str, cursor: Option<&str>) -> Result<Page> {
        self.page(
            "search",
            json!({ "query": query, "params": SONGS_FILTER }),
            cursor,
        )
    }

    fn details(&self, item: &Item) -> Result<Details> {
        if item.kind != Kind::Track {
            return Err(Error::unsupported());
        }
        let resolved = self.resolver.resolve(&item.id)?;
        let audio = resolved.audio.ok_or_else(|| {
            Error::new(
                ErrorKind::Unavailable,
                "YouTube no entregó el audio de esta canción.",
            )
        })?;
        Ok(Details {
            item: item.clone(),
            description: item.subtitle.clone(),
            sources: vec![Source {
                url: audio,
                label: "Audio".into(),
                height: None,
                live: false,
                audio_url: None,
                drop: Vec::new(),
                twitch_ads: false,
                available_at: Some(resolved.available_at),
            }],
        })
    }

    /// Un álbum: sus canciones. Un artista: sus canciones y sus álbumes.
    fn seasons(&self, series: &Item) -> Result<Vec<Season>> {
        if series.kind != Kind::Series {
            return Err(Error::unsupported());
        }
        let response = self.post("browse", json!({ "browseId": series.id }))?;
        let all = items(&response);
        let tracks: Vec<Item> = all
            .iter()
            .filter(|i| i.kind == Kind::Track)
            .cloned()
            .collect();
        // En un álbum, los otros álbumes de la página son recomendaciones: no son parte del álbum.
        let albums: Vec<Item> = if series.id.starts_with("MPRE") {
            Vec::new()
        } else {
            all.iter()
                .filter(|i| i.kind == Kind::Series)
                .cloned()
                .collect()
        };
        let mut seasons = Vec::new();
        if !tracks.is_empty() {
            seasons.push(Season {
                number: 1,
                name: "Canciones".into(),
                episodes: tracks,
            });
        }
        if !albums.is_empty() {
            seasons.push(Season {
                number: seasons.len() as u32 + 1,
                name: "Álbumes".into(),
                episodes: albums,
            });
        }
        if seasons.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                format!("{} no tiene canciones para mostrar.", series.title),
            ));
        }
        Ok(seasons)
    }

    /// La radio de una canción: canciones parecidas, para seguir escuchando al final de la cola.
    fn radio(&self, item: &Item) -> Result<Vec<Item>> {
        let response = self.post(
            "next",
            json!({ "videoId": item.id, "playlistId": format!("RDAMVM{}", item.id) }),
        )?;
        Ok(radio_items(&response)
            .into_iter()
            .filter(|i| i.id != item.id)
            .collect())
    }
}

/// Los elementos de una respuesta de YouTube Music, sin repetir.
pub fn items(root: &Value) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    collect(root, 0, &mut out);
    let mut seen = std::collections::HashSet::new();
    out.retain(|i| seen.insert(format!("{:?}:{}", i.kind, i.id)));
    out
}

fn collect(value: &Value, depth: usize, out: &mut Vec<Item>) {
    if depth > 60 {
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                match key.as_str() {
                    "musicResponsiveListItemRenderer" => out.extend(responsive(v)),
                    "musicTwoRowItemRenderer" => out.extend(two_row(v)),
                    _ => collect(v, depth + 1, out),
                }
            }
        }
        Value::Array(list) => {
            for v in list {
                collect(v, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// El token de la página siguiente, si la respuesta lo trae.
pub fn continuation(root: &Value) -> Option<String> {
    find_key(root, "continuationCommand", 0)
        .and_then(|c| c["token"].as_str())
        .map(str::to_string)
}

/// Un elemento de una lista (`musicResponsiveListItemRenderer`): una canción, un álbum o un artista.
fn responsive(r: &Value) -> Option<Item> {
    let column0 = &r["flexColumns"][0]["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"];
    let title_run = &column0[0];
    let title = text(&title_run["text"]);
    let video_id = r["playlistItemData"]["videoId"]
        .as_str()
        .or_else(|| title_run["navigationEndpoint"]["watchEndpoint"]["videoId"].as_str());
    let browse = title_run["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str();
    let subtitle_runs =
        &r["flexColumns"][1]["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"];
    let mut meta = Meta::from_runs(subtitle_runs);
    // En un álbum, la duración va en la columna fija de la derecha, no en el texto de abajo.
    if meta.duration.is_none() {
        meta.duration = secs_of(&text(
            &r["fixedColumns"][0]["musicResponsiveListItemFixedColumnRenderer"]["text"],
        ));
    }
    let thumb = thumb_of(&r["thumbnail"]);
    build(&title, video_id, browse, &meta, thumb)
}

/// Un elemento de carrusel (`musicTwoRowItemRenderer`): un álbum, un artista o una canción en tarjeta.
fn two_row(r: &Value) -> Option<Item> {
    let title_run = &r["title"]["runs"][0];
    let title = text(&title_run["text"]);
    let video_id = title_run["navigationEndpoint"]["watchEndpoint"]["videoId"].as_str();
    let browse = title_run["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str();
    let meta = Meta::from_runs(&r["subtitle"]["runs"]);
    let thumb = thumb_of(&r["thumbnailRenderer"]);
    build(&title, video_id, browse, &meta, thumb)
}

/// Una cola de radio (`playlistPanelVideoRenderer`).
fn radio_items(root: &Value) -> Vec<Item> {
    let mut out = Vec::new();
    radio_walk(root, 0, &mut out);
    out
}

fn radio_walk(value: &Value, depth: usize, out: &mut Vec<Item>) {
    if depth > 60 {
        return;
    }
    match value {
        Value::Object(map) => {
            if let Some(panel) = map.get("playlistPanelVideoRenderer") {
                let title = text(&panel["title"]);
                let meta = Meta::from_runs(&panel["longBylineText"]["runs"]);
                if let Some(id) = panel["videoId"].as_str() {
                    let mut item = Item::new(Platform::YouTubeMusic, Kind::Track, id, title);
                    item.subtitle = meta.subtitle();
                    item.duration_secs = secs_of(&text(&panel["lengthText"]));
                    item.thumb = thumb_of(&panel["thumbnail"]);
                    out.push(item);
                }
                return;
            }
            for v in map.values() {
                radio_walk(v, depth + 1, out);
            }
        }
        Value::Array(list) => {
            for v in list {
                radio_walk(v, depth + 1, out);
            }
        }
        _ => {}
    }
}

fn build(
    title: &str,
    video_id: Option<&str>,
    browse: Option<&str>,
    meta: &Meta,
    thumb: Option<String>,
) -> Option<Item> {
    if title.is_empty() {
        return None;
    }
    if let Some(id) = video_id {
        let mut item = Item::new(Platform::YouTubeMusic, Kind::Track, id, title);
        item.subtitle = meta.subtitle();
        item.duration_secs = meta.duration;
        item.thumb = thumb;
        item.extra = meta.album_id.clone();
        return Some(item);
    }
    let browse = browse?;
    let kind_subtitle = |label: &str| {
        let mut parts = vec![label.to_string()];
        if !meta.artists.is_empty() {
            parts.push(meta.artists.join(", "));
        }
        parts.join(" · ")
    };
    if browse.starts_with("MPRE") {
        // Álbum: se abre como serie (sus canciones son las temporadas).
        let mut item = Item::new(Platform::YouTubeMusic, Kind::Series, browse, title);
        item.subtitle = kind_subtitle("Álbum");
        item.thumb = thumb;
        return Some(item);
    }
    if browse.starts_with("UC") {
        let mut item = Item::new(Platform::YouTubeMusic, Kind::Series, browse, title);
        item.subtitle = "Artista".into();
        item.thumb = thumb;
        return Some(item);
    }
    None
}

/// Lo que dice el texto de abajo de un elemento: artistas, álbum y duración.
#[derive(Default)]
struct Meta {
    artists: Vec<String>,
    album: Option<String>,
    album_id: Option<String>,
    duration: Option<u64>,
}

impl Meta {
    fn from_runs(runs: &Value) -> Meta {
        let mut meta = Meta::default();
        for run in runs.as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let name = jstr(run, "text");
            let name = name.trim();
            if name.is_empty() || name == "•" {
                continue;
            }
            let page = run["navigationEndpoint"]["browseEndpoint"]
                ["browseEndpointContextSupportedConfigs"]["browseEndpointContextMusicConfig"]
                ["pageType"]
                .as_str()
                .unwrap_or("");
            match page {
                "MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL" => {
                    meta.artists.push(name.to_string())
                }
                "MUSIC_PAGE_TYPE_ALBUM" => {
                    meta.album = Some(name.to_string());
                    meta.album_id = run["navigationEndpoint"]["browseEndpoint"]["browseId"]
                        .as_str()
                        .map(str::to_string);
                }
                _ => {
                    if meta.duration.is_none() {
                        meta.duration = secs_of(name);
                    }
                    // Un texto sin enlace que no es el tipo ni la duración suele ser el artista.
                    if meta.artists.is_empty() && meta.album.is_none() && secs_of(name).is_none() {
                        let kind = name.to_lowercase();
                        if !matches!(
                            kind.as_str(),
                            "canción"
                                | "song"
                                | "video"
                                | "vídeo"
                                | "álbum"
                                | "album"
                                | "ep"
                                | "single"
                        ) {
                            meta.artists.push(name.to_string());
                        }
                    }
                }
            }
        }
        meta
    }

    fn subtitle(&self) -> String {
        let mut parts = Vec::new();
        if !self.artists.is_empty() {
            parts.push(self.artists.join(", "));
        }
        if let Some(album) = &self.album {
            parts.push(album.clone());
        }
        parts.join(" · ")
    }
}

/// Segundos de una duración «3:33» o «1:02:03». `None` si el texto no es una duración.
fn secs_of(text: &str) -> Option<u64> {
    crate::youtube::catalog::secs_of(text)
}

/// La miniatura más grande de un objeto con `thumbnails` (o `musicThumbnailRenderer`).
fn thumb_of(v: &Value) -> Option<String> {
    let found = find_key(v, "thumbnails", 0)?;
    let url = found.as_array()?.last()?["url"].as_str()?;
    Some(if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song_row() -> Value {
        json!({ "musicResponsiveListItemRenderer": {
            "playlistItemData": { "videoId": "lYBUbBu4W08" },
            "thumbnail": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
                { "url": "//lh3.example/a=w60", "width": 60 }, { "url": "//lh3.example/a=w226", "width": 226 } ] } } },
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                    { "text": "Never Gonna Give You Up", "navigationEndpoint": { "watchEndpoint": { "videoId": "lYBUbBu4W08" } } } ] } } },
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                    { "text": "Rick Astley", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCwZ", "browseEndpointContextSupportedConfigs": { "browseEndpointContextMusicConfig": { "pageType": "MUSIC_PAGE_TYPE_ARTIST" } } } } },
                    { "text": " • " },
                    { "text": "Whenever", "navigationEndpoint": { "browseEndpoint": { "browseId": "MPREb_1", "browseEndpointContextSupportedConfigs": { "browseEndpointContextMusicConfig": { "pageType": "MUSIC_PAGE_TYPE_ALBUM" } } } } },
                    { "text": " • " },
                    { "text": "3:33" } ] } } }
            ]
        }})
    }

    #[test]
    fn a_song_row_has_artist_album_and_duration() {
        let items = items(&json!({ "contents": [song_row()] }));
        assert_eq!(items.len(), 1);
        let song = &items[0];
        assert_eq!(song.kind, Kind::Track);
        assert_eq!(song.id, "lYBUbBu4W08");
        assert_eq!(song.title, "Never Gonna Give You Up");
        assert_eq!(song.subtitle, "Rick Astley · Whenever");
        assert_eq!(song.duration_secs, Some(213));
        assert_eq!(song.extra.as_deref(), Some("MPREb_1"));
        assert_eq!(song.thumb.as_deref(), Some("https://lh3.example/a=w226"));
    }

    #[test]
    fn album_and_artist_cards_open_as_series() {
        let card = json!({ "musicTwoRowItemRenderer": {
            "title": { "runs": [ { "text": "Whenever You Need Somebody", "navigationEndpoint": { "browseEndpoint": { "browseId": "MPREb_1" } } } ] },
            "subtitle": { "runs": [ { "text": "Álbum" }, { "text": " • " }, { "text": "1987" } ] }
        }});
        let artist = json!({ "musicTwoRowItemRenderer": {
            "title": { "runs": [ { "text": "Rick Astley", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCwZ" } } } ] }
        }});
        let items = items(&json!({ "a": [card, artist] }));
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.kind == Kind::Series));
        assert_eq!(items[0].id, "MPREb_1");
        assert_eq!(items[1].id, "UCwZ");
    }

    #[test]
    fn radio_queue_comes_from_playlist_panel_videos() {
        let next = json!({ "contents": { "playlistPanelRenderer": { "contents": [
            { "playlistPanelVideoRenderer": { "videoId": "AAAAAAAAAAA", "title": { "runs": [ { "text": "Otra" } ] },
              "longBylineText": { "runs": [ { "text": "Artista" } ] }, "lengthText": { "runs": [ { "text": "2:30" } ] } } }
        ] } } });
        let queue = radio_items(&next);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].id, "AAAAAAAAAAA");
        assert_eq!(queue[0].duration_secs, Some(150));
    }

    #[test]
    fn continuation_token_is_found_anywhere() {
        let root = json!({ "x": { "continuationItems": [ { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "TOKEN" } } } } ] } });
        assert_eq!(continuation(&root).as_deref(), Some("TOKEN"));
    }
}
