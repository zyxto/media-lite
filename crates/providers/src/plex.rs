//! Plex: una cuenta anónima (plex.tv), los canales en vivo (epg.provider.plex.tv) y las películas y
//! series gratis (vod.provider.plex.tv). Solo lo que no tiene DRM; por defecto, solo lo que está en
//! español. Portado de `lumiere/src/plex.cpp`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use media_lite_core::http::{self, Agent};
use media_lite_core::store::write_atomic;
use media_lite_core::{
    Caps, Category, Details, Error, ErrorKind, Item, Kind, Page, Platform, Result, Season, Section,
    Source,
};
use serde_json::{json, Value};

use crate::text::genre_es;
use crate::util::{jarr, jbool, jnum, jstr, matches, now, pct_encode, uuid4};
use crate::Provider;

const EPG: &str = "https://epg.provider.plex.tv";
const VOD: &str = "https://vod.provider.plex.tv";
const DISCOVER: &str = "https://discover.provider.plex.tv";
const MAX_JSON: u64 = 8 * 1024 * 1024;
const CHANNELS_TTL: u64 = 3600;
const HUBS_TTL: u64 = 3600;
const VERSION: &str = "4.140.0";

#[derive(Clone)]
struct Hub {
    cat: Category,
    key: String,
}

#[derive(Default)]
struct Cache {
    channels: Vec<Item>,
    channels_at: u64,
    hubs: Vec<Hub>,
    hubs_at: u64,
    hub_items: HashMap<String, Vec<Item>>,
}

pub struct Plex {
    agent: Agent,
    dir: PathBuf,
    /// Muestra también lo que está en inglés (opción de Ajustes).
    english: bool,
    token: Mutex<String>,
    client: String,
    cache: Mutex<Cache>,
}

impl Plex {
    /// `dir`: carpeta de configuración, donde se guardan el identificador y el token anónimo (0600).
    pub fn new(dir: &Path, english: bool) -> Plex {
        let (client, token) = load_identity(dir);
        Plex {
            agent: http::agent(),
            dir: dir.to_path_buf(),
            english,
            token: Mutex::new(token),
            client,
            cache: Mutex::new(Cache::default()),
        }
    }

    fn headers(&self, token: &str) -> Vec<(String, String)> {
        vec![
            ("Accept".into(), "application/json".into()),
            ("X-Plex-Token".into(), token.into()),
            ("X-Plex-Client-Identifier".into(), self.client.clone()),
            ("X-Plex-Product".into(), "Plex Web".into()),
            ("X-Plex-Version".into(), VERSION.into()),
            ("X-Plex-Language".into(), "es".into()),
        ]
    }

    /// Token anónimo: si no hay, se pide uno nuevo a plex.tv y se guarda.
    fn login(&self) -> Result<String> {
        {
            let token = self
                .token
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "estado dañado"))?;
            if !token.is_empty() {
                return Ok(token.clone());
            }
        }
        let headers = [
            ("Accept", "application/json"),
            ("X-Plex-Product", "Plex Web"),
            ("X-Plex-Version", VERSION),
            ("X-Plex-Client-Identifier", self.client.as_str()),
        ];
        let response = http::request(
            &self.agent,
            "POST",
            "https://plex.tv/api/v2/users/anonymous",
            &headers,
            Some(""),
            MAX_JSON,
        )?;
        let v: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
        let token = jstr(&v, "authToken");
        if token.is_empty() {
            return Err(Error::new(
                ErrorKind::Login,
                format!("Plex no abrió la sesión (error {})", response.status),
            ));
        }
        save_identity(&self.dir, &self.client, &token);
        *self
            .token
            .lock()
            .map_err(|_| Error::new(ErrorKind::Other, "estado dañado"))? = token.clone();
        Ok(token)
    }

    /// GET autenticado. Si el token guardado venció (401), se pide uno nuevo y se reintenta una vez.
    fn api(&self, url: &str) -> Result<Value> {
        let mut token = self.login()?;
        let mut response = http::request(
            &self.agent,
            "GET",
            url,
            &borrow(&self.headers(&token)),
            None,
            MAX_JSON,
        )?;
        if response.status == 401 {
            *self
                .token
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "estado dañado"))? = String::new();
            token = self.login()?;
            response = http::request(
                &self.agent,
                "GET",
                url,
                &borrow(&self.headers(&token)),
                None,
                MAX_JSON,
            )?;
        }
        if !response.ok() {
            return Err(Error::network(format!(
                "Plex: error {} del servidor",
                response.status
            )));
        }
        serde_json::from_slice(&response.body).map_err(|_| Error::parse("Plex: respuesta ilegible"))
    }

    fn channels(&self) -> Result<Vec<Item>> {
        {
            let c = self
                .cache
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
            if !c.channels.is_empty() && now() - c.channels_at < CHANNELS_TTL {
                return Ok(c.channels.clone());
            }
        }
        let v = self.api(&format!("{EPG}/lineups/plex/channels"))?;
        let mut list = Vec::new();
        for ch in jarr(v.get("MediaContainer").unwrap_or(&Value::Null), "Channel") {
            let Some(media) = jarr(ch, "Media").first() else {
                continue;
            };
            if jbool(media, "drm") {
                continue;
            }
            let part = jstr(jarr(media, "Part").first().unwrap_or(&Value::Null), "key");
            let id = jstr(ch, "id");
            if id.is_empty() || part.is_empty() {
                continue;
            }
            let mut item = Item::new(Platform::Plex, Kind::Channel, id, jstr(ch, "title"));
            item.live = true;
            item.description = jstr(ch, "summary");
            let mut thumb = jstr(ch, "art");
            if thumb.is_empty() {
                thumb = jstr(ch, "thumb");
            }
            item.thumb = Some(small(&thumb)).filter(|s| !s.is_empty());
            let language = jstr(ch, "language");
            item.subtitle = if language == "es" {
                "En español".into()
            } else {
                format!("Idioma: {language}")
            };
            // Guardados juntos: la parte del stream y la clave de la guía.
            item.extra = Some(format!("{part}\n{}", jstr(ch, "gridKey")));
            list.push(item);
        }
        if list.is_empty() {
            return Err(Error::parse("Plex no mandó canales"));
        }
        let mut c = self
            .cache
            .lock()
            .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
        c.channels = list.clone();
        c.channels_at = now();
        Ok(list)
    }

    fn hubs(&self) -> Result<Vec<Hub>> {
        {
            let c = self
                .cache
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
            if !c.hubs.is_empty() && now() - c.hubs_at < HUBS_TTL {
                return Ok(c.hubs.clone());
            }
        }
        let v = self.api(&format!("{VOD}/hubs/sections/movies"))?;
        let mut hubs: Vec<Hub> = Vec::new();
        for h in jarr(v.get("MediaContainer").unwrap_or(&Value::Null), "Hub") {
            let key = jstr(h, "key");
            let title = jstr(h, "title");
            if key.is_empty() || title.is_empty() {
                continue;
            }
            let spanish = key.contains("espanol") || key.contains("latino");
            if !spanish && !self.english {
                continue;
            }
            let shown = if spanish {
                "En español".to_string()
            } else {
                format!("{} (inglés)", genre_es(&title))
            };
            hubs.push(Hub {
                cat: Category {
                    id: key.clone(),
                    title: shown,
                },
                key,
            });
        }
        // Primero las de español.
        hubs.sort_by_key(|h| h.cat.title.ends_with("(inglés)"));
        if hubs.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "Plex no tiene películas en español ahora",
            ));
        }
        let mut c = self
            .cache
            .lock()
            .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
        c.hubs = hubs.clone();
        c.hubs_at = now();
        c.hub_items.clear();
        Ok(hubs)
    }

    /// Los títulos de una fila, de hasta 600. Se cachean junto con la fila.
    fn hub(&self, key: &str) -> Result<Vec<Item>> {
        {
            let c = self
                .cache
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
            if let Some(items) = c.hub_items.get(key) {
                return Ok(items.clone());
            }
        }
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let sep = if key.contains('?') { "&" } else { "?" };
        for start in (0..600).step_by(100) {
            let v = self.api(&format!(
                "{VOD}{key}{sep}count=100&X-Plex-Container-Start={start}"
            ))?;
            let got = jarr(v.get("MediaContainer").unwrap_or(&Value::Null), "Metadata");
            for m in got {
                if !free_of_drm(m) {
                    continue;
                }
                let item = meta_item(m);
                if !item.id.is_empty() && seen.insert(item.id.clone()) {
                    out.push(item);
                }
            }
            if got.len() < 100 {
                break;
            }
        }
        if !out.is_empty() {
            let mut c = self
                .cache
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
            c.hub_items.insert(key.to_string(), out.clone());
        }
        Ok(out)
    }
}

/// Lee el identificador y el token guardados. Si no hay, se generan (el token queda vacío).
fn load_identity(dir: &Path) -> (String, String) {
    let text = std::fs::read_to_string(dir.join("plex.json")).unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let client = jstr(&v, "client");
    let client = if client.is_empty() { uuid4() } else { client };
    (client, jstr(&v, "token"))
}

/// Guarda identificador y token en un archivo que solo puede leer el usuario.
fn save_identity(dir: &Path, client: &str, token: &str) {
    let body = json!({ "client": client, "token": token }).to_string();
    if let Err(e) = write_atomic(&dir.join("plex.json"), body.as_bytes(), 0o600) {
        log::warn!("no se pudo guardar la sesión de Plex: {e}");
    }
}

fn borrow(pairs: &[(String, String)]) -> Vec<(&str, &str)> {
    pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

/// Plex no da miniaturas chicas: su redimensionador da 480×270 en unos 20 KB.
fn small(url: &str) -> String {
    if !url.starts_with("http") {
        return url.to_string();
    }
    format!(
        "https://images.plex.tv/photo?width=480&height=270&minSize=1&upscale=1&url={}",
        pct_encode(url)
    )
}

/// "Season 2" → "Temporada 2", "Episode 3" → "Episodio 3".
fn spanish(title: &str) -> String {
    if let Some(n) = title.strip_prefix("Season ") {
        format!("Temporada {n}")
    } else if let Some(n) = title.strip_prefix("Episode ") {
        format!("Episodio {n}")
    } else if title == "Specials" {
        "Especiales".into()
    } else {
        title.to_string()
    }
}

fn meta_item(m: &Value) -> Item {
    let kind = match jstr(m, "type").as_str() {
        "show" => Kind::Series,
        "episode" => Kind::Episode,
        _ => Kind::Movie,
    };
    let mut item = Item::new(
        Platform::Plex,
        kind,
        jstr(m, "ratingKey"),
        spanish(&jstr(m, "title")),
    );
    item.description = jstr(m, "summary");
    let mut thumb = jstr(m, "art");
    if thumb.is_empty() {
        thumb = jstr(m, "thumb");
    }
    if kind == Kind::Episode {
        let own = jstr(m, "thumb");
        if !own.is_empty() {
            thumb = own;
        }
    }
    // Las imágenes de TMDB originales pesan megas: la de 500 px alcanza para una tarjeta.
    let thumb = thumb.replace("image.tmdb.org/t/p/original/", "image.tmdb.org/t/p/w500/");
    item.thumb = Some(small(&thumb)).filter(|s| !s.is_empty());
    if kind != Kind::Series {
        let secs = jnum(m, "duration") / 1000.0;
        item.duration_secs = Some(secs as u64).filter(|s| *s > 0);
    }
    let mut subtitle: Vec<String> = Vec::new();
    let year = jstr(m, "year");
    if !year.is_empty() {
        subtitle.push(year);
    }
    let rating = jstr(m, "contentRating");
    if !rating.is_empty() && rating != "Not Rated" && rating != "NR" {
        subtitle.push(rating);
    }
    if let Some(genre) = jarr(m, "Genre").first() {
        subtitle.push(genre_es(&jstr(genre, "tag")));
    }
    item.subtitle = subtitle.join(" · ");
    // El stream HLS sin DRM, si lo tiene.
    item.extra = jarr(m, "Media")
        .iter()
        .find(|md| !jbool(md, "drm") && jstr(md, "protocol") == "hls")
        .and_then(|md| jarr(md, "Part").first())
        .map(|p| jstr(p, "key"))
        .filter(|k| !k.is_empty());
    item
}

/// Un título sin DRM: las series no tienen parte propia (sus episodios sí), así que cuentan.
fn free_of_drm(m: &Value) -> bool {
    match m.get("Media").and_then(Value::as_array) {
        None => true,
        Some(media) => media
            .iter()
            .any(|md| !jbool(md, "drm") && jstr(md, "protocol") == "hls"),
    }
}

impl Provider for Plex {
    fn platform(&self) -> Platform {
        Platform::Plex
    }

    fn caps(&self) -> Caps {
        Caps {
            live: true,
            movies: true,
            series: true,
            search: true,
            ..Caps::default()
        }
    }

    fn categories(&self, section: Section) -> Result<Vec<Category>> {
        if section == Section::Live {
            let mut out = vec![Category {
                id: "es".into(),
                title: "En español".into(),
            }];
            if self.english {
                out.push(Category {
                    id: String::new(),
                    title: "Todos los canales".into(),
                });
            }
            return Ok(out);
        }
        let mut out = Vec::new();
        for hub in self.hubs()? {
            if out.len() >= 40 {
                break;
            }
            let wanted = if section == Section::Movies {
                Kind::Movie
            } else {
                Kind::Series
            };
            let any = self.hub(&hub.key)?.iter().any(|i| i.kind == wanted);
            if any {
                out.push(hub.cat);
            }
        }
        if out.is_empty() {
            let msg = if section == Section::Series {
                "Plex no tiene series en español ahora"
            } else {
                "Plex no tiene películas en español ahora"
            };
            return Err(Error::new(ErrorKind::Unavailable, msg));
        }
        Ok(out)
    }

    fn list(
        &self,
        section: Section,
        category: Option<&str>,
        _cursor: Option<&str>,
    ) -> Result<Page> {
        if section == Section::Live {
            let items = self
                .channels()?
                .into_iter()
                .filter(|c| match category {
                    Some("") => true,
                    Some("es") | None => c.subtitle == "En español",
                    Some(_) => true,
                })
                .collect();
            return Ok(Page { items, next: None });
        }
        let hubs = self.hubs()?;
        let key = match category {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => hubs.first().map(|h| h.key.clone()).unwrap_or_default(),
        };
        let wanted = if section == Section::Movies {
            Kind::Movie
        } else {
            Kind::Series
        };
        let items = self
            .hub(&key)?
            .into_iter()
            .filter(|i| i.kind == wanted)
            .collect();
        Ok(Page { items, next: None })
    }

    fn search(&self, query: &str, _cursor: Option<&str>) -> Result<Page> {
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        if let Ok(channels) = self.channels() {
            for ch in channels.into_iter().filter(|c| {
                (self.english || c.subtitle == "En español") && matches(&c.title, query)
            }) {
                if seen.insert(ch.id.clone()) {
                    items.push(ch);
                }
            }
        }
        let url = format!(
            "{DISCOVER}/library/search?query={}&limit=40&searchTypes=movies,tv&searchProviders=discover,plexAVOD&includeMetadata=1",
            pct_encode(query)
        );
        if let Ok(v) = self.api(&url) {
            for sr in jarr(
                v.get("MediaContainer").unwrap_or(&Value::Null),
                "SearchResults",
            ) {
                if jstr(sr, "id") != "plex" {
                    continue;
                }
                for res in jarr(sr, "SearchResult") {
                    let Some(m) = res.get("Metadata") else {
                        continue;
                    };
                    if !free_of_drm(m) {
                        continue;
                    }
                    let item = meta_item(m);
                    if item.kind != Kind::Episode
                        && !item.id.is_empty()
                        && seen.insert(item.id.clone())
                    {
                        items.push(item);
                    }
                }
            }
        }
        Ok(Page { items, next: None })
    }

    fn details(&self, item: &Item) -> Result<Details> {
        let token = self.login()?;
        let mut item = item.clone();
        // Desde una búsqueda o una fila falta la parte o la sinopsis: se pide la ficha completa.
        let needs_part = item.extra.as_deref().is_none_or(str::is_empty);
        if !item.live && (needs_part || item.description.is_empty()) {
            let v = self.api(&format!("{VOD}/library/metadata/{}", item.id))?;
            let full = meta_item(
                jarr(v.get("MediaContainer").unwrap_or(&Value::Null), "Metadata")
                    .first()
                    .unwrap_or(&Value::Null),
            );
            if needs_part {
                item.extra = full.extra;
            }
            if item.description.is_empty() {
                item.description = full.description;
            }
        }
        let part = if item.live {
            item.extra
                .as_deref()
                .and_then(|e| e.split('\n').next())
                .unwrap_or("")
                .to_string()
        } else {
            item.extra.clone().unwrap_or_default()
        };
        if part.is_empty() {
            return Err(Error::new(
                ErrorKind::Drm,
                "Plex no tiene este título sin DRM",
            ));
        }
        let base = if item.live { EPG } else { VOD };
        let url = format!("{base}{part}?X-Plex-Token={token}");
        Ok(Details {
            description: item.description.clone(),
            item,
            sources: vec![Source {
                url,
                label: "HLS (automática)".into(),
                height: None,
                live: false,
                audio_url: None,
                drop: Vec::new(),
            }],
        })
    }

    fn seasons(&self, series: &Item) -> Result<Vec<Season>> {
        let v = self.api(&format!("{VOD}/library/metadata/{}/children", series.id))?;
        let mut out = Vec::new();
        for s in jarr(v.get("MediaContainer").unwrap_or(&Value::Null), "Metadata") {
            let number = jnum(s, "index").max((out.len() + 1) as f64) as u32;
            let Ok(children) = self.api(&format!(
                "{VOD}/library/metadata/{}/children",
                jstr(s, "ratingKey")
            )) else {
                continue;
            };
            let mut episodes = Vec::new();
            for e in jarr(
                children.get("MediaContainer").unwrap_or(&Value::Null),
                "Metadata",
            ) {
                if !free_of_drm(e) {
                    continue;
                }
                let mut ep = meta_item(e);
                ep.kind = Kind::Episode;
                ep.season = Some(number);
                ep.episode = Some(jnum(e, "index") as u32);
                ep.subtitle = format!("T{number} · E{}", ep.episode.unwrap_or(0));
                if ep.thumb.is_none() {
                    ep.thumb = series.thumb.clone();
                }
                episodes.push(ep);
            }
            if !episodes.is_empty() {
                out.push(Season {
                    number,
                    name: spanish(&jstr(s, "title")),
                    episodes,
                });
            }
        }
        if out.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "Esta serie no tiene episodios disponibles",
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_titles() {
        assert_eq!(spanish("Season 2"), "Temporada 2");
        assert_eq!(spanish("Episode 3"), "Episodio 3");
        assert_eq!(spanish("Specials"), "Especiales");
        assert_eq!(spanish("Otro"), "Otro");
    }

    #[test]
    fn drm_items_are_hidden() {
        let drm: Value =
            serde_json::from_str(r#"{"Media":[{"drm":true,"protocol":"hls"}]}"#).unwrap();
        let hls: Value =
            serde_json::from_str(r#"{"Media":[{"protocol":"hls","Part":[{"key":"/p/1"}]}]}"#)
                .unwrap();
        assert!(!free_of_drm(&drm));
        assert!(free_of_drm(&hls));
        assert_eq!(meta_item(&hls).extra.as_deref(), Some("/p/1"));
    }

    #[test]
    fn small_images_go_through_the_resizer() {
        assert!(small("https://x.test/a.jpg").starts_with("https://images.plex.tv/photo?width=480"));
    }
}
