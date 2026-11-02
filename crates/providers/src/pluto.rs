//! Pluto TV: una sesión de `boot.pluto.tv`, los canales con su guía, y el catálogo de películas y
//! series. El video sale del "stitcher" en HLS; el proxy quita los segmentos de anuncios.
//! Portado de `lumiere/src/pluto.cpp`.

use std::collections::HashMap;
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
use crate::util::{iso_hour, jarr, jnum, jstr, matches, now, parse_iso, pct_encode, uuid4};
use crate::Provider;

const BOOT: &str = "https://boot.pluto.tv";
const DEFAULT_CHANNELS: &str = "https://service-channels.clusters.pluto.tv";
const DEFAULT_VOD: &str = "https://service-vod.clusters.pluto.tv";
const DEFAULT_STITCHER: &str = "https://cfd-v4-service-channel-stitcher-use1-1.prd.pluto.tv";
/// Límite de una respuesta JSON. El catálogo entero pesa varios MB.
const MAX_JSON: u64 = 16 * 1024 * 1024;
const CHANNELS_TTL: u64 = 3600;
const VOD_TTL: u64 = 1800;
const TIMELINES_TTL: u64 = 600;

/// Los anuncios de Pluto TV van en la misma playlist, entre `#EXT-X-DISCONTINUITY`: sus segmentos
/// tienen estas rutas.
pub const AD_PATTERNS: [&str; 4] = ["_ad%2F", "%2Fad%2F", "/ad/creative", "ad%2Fcreative"];

#[derive(Default)]
struct Session {
    token: String,
    until: u64,
    stitcher_params: String,
    servers: HashMap<String, String>,
}

/// Categoría de canales y los ids de sus canales.
type ChannelCats = Vec<(Category, Vec<String>)>;

struct VodCat {
    cat: Category,
    movies: Vec<Item>,
    series: Vec<Item>,
}

#[derive(Clone)]
struct Program {
    start: u64,
    stop: u64,
    title: String,
}

#[derive(Default)]
struct Cache {
    channels: Vec<Item>,
    channel_cats: Vec<(Category, Vec<String>)>,
    channels_at: u64,
    vod: Vec<VodCat>,
    vod_at: u64,
    timelines: HashMap<String, Vec<Program>>,
    timelines_at: u64,
}

pub struct Pluto {
    agent: Agent,
    dir: PathBuf,
    session: Mutex<Session>,
    cache: Mutex<Cache>,
}

impl Pluto {
    /// `dir`: carpeta de configuración, donde se guarda el identificador de cliente.
    pub fn new(dir: &Path) -> Pluto {
        Pluto {
            agent: http::agent(),
            dir: dir.to_path_buf(),
            session: Mutex::new(Session::default()),
            cache: Mutex::new(Cache::default()),
        }
    }

    /// Identificador de este equipo para Pluto: se crea una vez y se guarda.
    fn client_id(&self) -> String {
        let path = self.dir.join("pluto.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                let id = jstr(&v, "client");
                if !id.is_empty() {
                    return id;
                }
            }
        }
        let id = uuid4();
        let _ = write_atomic(&path, json!({ "client": id }).to_string().as_bytes(), 0o600);
        id
    }

    /// Token de sesión, renovado cuando vence. La red se usa sin tener el candado.
    fn session(&self) -> Result<String> {
        {
            let s = self
                .session
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "estado dañado"))?;
            if !s.token.is_empty() && now() < s.until {
                return Ok(s.token.clone());
            }
        }
        let client = self.client_id();
        let url = format!(
            "{BOOT}/v4/start?appName=web&appVersion=9&deviceVersion=128&deviceModel=web&deviceMake=firefox&deviceType=web&clientID={client}&clientModelNumber=1.0.0&serverSideAds=false&drmCapabilities="
        );
        let response = http::get(&self.agent, &url, &[], MAX_JSON)
            .map_err(|e| Error::network(format!("Pluto TV: {}", e.message)))?;
        let v: Value = serde_json::from_slice(&response.body)
            .map_err(|_| Error::parse("Pluto TV: sesión ilegible"))?;
        let token = jstr(&v, "sessionToken");
        if token.is_empty() {
            return Err(Error::new(ErrorKind::Login, "Pluto TV no abrió la sesión"));
        }
        let refresh = v
            .get("refreshInSec")
            .and_then(Value::as_f64)
            .unwrap_or(3600.0 * 8.0);
        let mut servers = HashMap::new();
        if let Some(obj) = v.get("servers").and_then(Value::as_object) {
            for (k, val) in obj {
                if let Some(s) = val.as_str() {
                    servers.insert(k.clone(), s.to_string());
                }
            }
        }
        let mut s = self
            .session
            .lock()
            .map_err(|_| Error::new(ErrorKind::Other, "estado dañado"))?;
        s.token = token.clone();
        s.until = now() + (600f64).max(refresh - 300.0) as u64;
        s.stitcher_params = jstr(&v, "stitcherParams");
        s.servers = servers;
        Ok(token)
    }

    fn server(&self, name: &str, default: &str) -> String {
        self.session
            .lock()
            .ok()
            .and_then(|s| s.servers.get(name).cloned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default.to_string())
    }

    /// GET autenticado. Si la sesión venció antes de tiempo (401), pide una nueva y reintenta una vez.
    fn api(&self, url: &str) -> Result<Value> {
        let mut token = self.session()?;
        let mut response = http::request(
            &self.agent,
            "GET",
            url,
            &[("Authorization", &format!("Bearer {token}"))],
            None,
            MAX_JSON,
        )?;
        if response.status == 401 {
            if let Ok(mut s) = self.session.lock() {
                s.token.clear();
            }
            token = self.session()?;
            response = http::request(
                &self.agent,
                "GET",
                url,
                &[("Authorization", &format!("Bearer {token}"))],
                None,
                MAX_JSON,
            )?;
        }
        if !response.ok() {
            return Err(Error::network(format!(
                "Pluto TV: error {} del servidor",
                response.status
            )));
        }
        serde_json::from_slice(&response.body)
            .map_err(|_| Error::parse("Pluto TV: respuesta ilegible"))
    }

    fn channels_base(&self) -> String {
        self.server("channels", DEFAULT_CHANNELS)
    }

    /// Canales (con su categoría). Se cachean una hora.
    fn load_channels(&self) -> Result<(Vec<Item>, ChannelCats)> {
        {
            let c = self
                .cache
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
            if !c.channels.is_empty() && now() - c.channels_at < CHANNELS_TTL {
                return Ok((c.channels.clone(), c.channel_cats.clone()));
            }
        }
        let base = self.channels_base();
        let v = self.api(&format!(
            "{base}/v2/guide/channels?channelIds=&offset=0&limit=1000&sort=number%3Aasc"
        ))?;
        let mut channels = Vec::new();
        for c in jarr(&v, "data") {
            let id = jstr(c, "id");
            if id.is_empty() {
                continue;
            }
            let mut item = Item::new(Platform::Pluto, Kind::Channel, id, jstr(c, "name"));
            item.live = true;
            item.description = jstr(c, "summary");
            let mut thumb = image_of(c, "featuredImage");
            // Un logo no sirve de fondo: solo la imagen destacada.
            item.backdrop = Some(big(&thumb)).filter(|s| !s.is_empty());
            if thumb.is_empty() {
                thumb = image_of(c, "colorLogoPNG");
            }
            item.thumb = Some(small(&thumb)).filter(|s| !s.is_empty());
            let num = jnum(c, "number") as u64;
            item.subtitle = if num > 0 {
                format!("Canal {num}")
            } else {
                String::new()
            };
            channels.push(item);
        }
        if channels.is_empty() {
            return Err(Error::parse("Pluto TV no mandó canales"));
        }
        let mut cats = Vec::new();
        if let Ok(categories) = self.api(&format!("{base}/v2/guide/categories")) {
            for c in jarr(&categories, "data") {
                let cat = Category {
                    id: jstr(c, "id"),
                    title: jstr(c, "name"),
                };
                let ids: Vec<String> = jarr(c, "channelIDs")
                    .iter()
                    .filter_map(|i| i.as_str().map(str::to_string))
                    .collect();
                if !ids.is_empty() && !cat.title.is_empty() {
                    cats.push((cat, ids));
                }
            }
        }
        let mut c = self
            .cache
            .lock()
            .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
        c.channels = channels.clone();
        c.channel_cats = cats.clone();
        c.channels_at = now();
        Ok((channels, cats))
    }

    /// Qué dan los canales ahora y más tarde: una petición por cada 60 canales, cacheada 10 minutos.
    fn load_timelines(&self, ids: &[String], hours: u64) {
        let base = self.channels_base();
        for chunk in ids.chunks(60) {
            let list = chunk.join(",");
            let url = format!(
                "{base}/v2/guide/timelines?start={}&channelIds={list}&duration={}",
                pct_encode(&iso_hour(now())),
                hours * 60
            );
            let Ok(v) = self.api(&url) else {
                return;
            };
            let Ok(mut c) = self.cache.lock() else {
                return;
            };
            for ch in jarr(&v, "data") {
                let programs: Vec<Program> = jarr(ch, "timelines")
                    .iter()
                    .map(|t| Program {
                        start: parse_iso(&jstr(t, "start")),
                        stop: parse_iso(&jstr(t, "stop")),
                        title: jstr(t, "title"),
                    })
                    .collect();
                c.timelines.insert(jstr(ch, "channelId"), programs);
            }
            c.timelines_at = now();
        }
    }

    fn now_on(&self, id: &str) -> String {
        let Ok(c) = self.cache.lock() else {
            return String::new();
        };
        let at = now();
        c.timelines
            .get(id)
            .and_then(|programs| programs.iter().find(|p| p.start <= at && at < p.stop))
            .map(|p| p.title.clone())
            .unwrap_or_default()
    }

    /// Catálogo de películas y series, por categoría. Se cachea media hora.
    fn load_vod(&self) -> Result<Vec<VodCat>> {
        {
            let c = self
                .cache
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
            if !c.vod.is_empty() && now() - c.vod_at < VOD_TTL {
                return Ok(c
                    .vod
                    .iter()
                    .map(|v| VodCat {
                        cat: v.cat.clone(),
                        movies: v.movies.clone(),
                        series: v.series.clone(),
                    })
                    .collect());
            }
        }
        let base = self.server("vod", DEFAULT_VOD);
        let v = self.api(&format!(
            "{base}/v4/vod/categories?includeItems=true&offset=1000&page=1"
        ))?;
        let mut cats = Vec::new();
        for c in jarr(&v, "categories") {
            let mut vc = VodCat {
                cat: Category {
                    id: jstr(c, "_id"),
                    title: jstr(c, "name"),
                },
                movies: Vec::new(),
                series: Vec::new(),
            };
            for o in jarr(c, "items") {
                let item = vod_item(o);
                if item.id.is_empty() {
                    continue;
                }
                if item.kind == Kind::Series {
                    vc.series.push(item);
                } else {
                    vc.movies.push(item);
                }
            }
            cats.push(vc);
        }
        if cats.is_empty() {
            return Err(Error::parse("Pluto TV no mandó el catálogo"));
        }
        let mut c = self
            .cache
            .lock()
            .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
        c.vod = cats
            .iter()
            .map(|v| VodCat {
                cat: v.cat.clone(),
                movies: v.movies.clone(),
                series: v.series.clone(),
            })
            .collect();
        c.vod_at = now();
        Ok(cats)
    }
}

/// Imagen de un tipo dado (`featuredImage`, `colorLogoPNG`…) dentro de `images`.
fn image_of(o: &Value, kind: &str) -> String {
    jarr(o, "images")
        .iter()
        .find(|im| jstr(im, "type") == kind)
        .map(|im| jstr(im, "url"))
        .unwrap_or_default()
}

/// Las imágenes llegan a 1600×900 (300 KB); una tarjeta necesita 480×270 (35 KB).
fn small(url: &str) -> String {
    if !url.contains("images.pluto.tv") {
        return url.to_string();
    }
    let base = url.split('?').next().unwrap_or(url);
    format!("{base}?fm=jpg&w=480&h=270&fit=fill&q=70")
}

/// Pluto escribe los saltos de línea de las sinopsis como el texto literal «U+000A».
fn clean_text(text: &str) -> String {
    text.replace("U+000A", "\n").trim().to_string()
}

/// El fondo de la pantalla de detalle: la misma imagen a 960×540 (unos 90 KB).
fn big(url: &str) -> String {
    if !url.contains("images.pluto.tv") {
        return url.to_string();
    }
    let base = url.split('?').next().unwrap_or(url);
    format!("{base}?fm=jpg&w=960&h=540&fit=fill&q=75")
}

fn vod_item(o: &Value) -> Item {
    let kind = if jstr(o, "type") == "series" {
        Kind::Series
    } else {
        Kind::Movie
    };
    let mut item = Item::new(Platform::Pluto, kind, jstr(o, "_id"), jstr(o, "name"));
    let desc = jstr(o, "description");
    item.description = clean_text(&if desc.is_empty() {
        jstr(o, "summary")
    } else {
        desc
    });
    let genre = genre_es(&jstr(o, "genre"));
    let rating = jstr(o, "rating");
    let mut subtitle: Vec<String> = Vec::new();
    if !genre.is_empty() {
        subtitle.push(genre);
    }
    if !rating.is_empty() && rating != "Not Rated" {
        subtitle.push(rating);
    }
    item.subtitle = subtitle.join(" · ");
    // Sin los anuncios, que es la duración que cuenta.
    let ms = if jnum(o, "originalContentDuration") > 0.0 {
        jnum(o, "originalContentDuration")
    } else {
        jnum(o, "duration")
    };
    item.duration_secs = Some((ms / 1000.0) as u64).filter(|s| *s > 0);
    item.extra =
        Some(jstr(o.get("stitched").unwrap_or(&Value::Null), "path")).filter(|s| !s.is_empty());
    let mut thumb = String::new();
    for c in jarr(o, "covers") {
        if jstr(c, "aspectRatio") == "16:9" {
            thumb = jstr(c, "url");
        }
    }
    if thumb.is_empty() {
        thumb = jstr(o.get("featuredImage").unwrap_or(&Value::Null), "path");
    }
    item.thumb = Some(small(&thumb)).filter(|s| !s.is_empty());
    item.backdrop = Some(big(&thumb)).filter(|s| !s.is_empty());
    item
}

impl Provider for Pluto {
    fn platform(&self) -> Platform {
        Platform::Pluto
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
            let (_, cats) = self.load_channels()?;
            let mut out = vec![Category {
                id: String::new(),
                title: "Todos los canales".into(),
            }];
            out.extend(cats.into_iter().map(|(c, _)| c));
            return Ok(out);
        }
        let vod = self.load_vod()?;
        Ok(vod
            .into_iter()
            .filter(|c| {
                if section == Section::Movies {
                    !c.movies.is_empty()
                } else {
                    !c.series.is_empty()
                }
            })
            .map(|c| c.cat)
            .collect())
    }

    fn list(
        &self,
        section: Section,
        category: Option<&str>,
        _cursor: Option<&str>,
    ) -> Result<Page> {
        if section == Section::Live {
            let (channels, cats) = self.load_channels()?;
            let wanted: Option<Vec<String>> = category.filter(|c| !c.is_empty()).map(|c| {
                cats.iter()
                    .filter(|(cat, _)| cat.id == c)
                    .flat_map(|(_, ids)| ids.clone())
                    .collect()
            });
            let items: Vec<Item> = channels
                .into_iter()
                .filter(|ch| wanted.as_ref().is_none_or(|w| w.contains(&ch.id)))
                .collect();
            let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
            let missing: Vec<String> = {
                let c = self
                    .cache
                    .lock()
                    .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
                if now() - c.timelines_at > TIMELINES_TTL {
                    ids.clone()
                } else {
                    ids.iter()
                        .filter(|id| !c.timelines.contains_key(*id))
                        .cloned()
                        .collect()
                }
            };
            if !missing.is_empty() {
                self.load_timelines(&missing, 4);
            }
            let items = items
                .into_iter()
                .map(|mut it| {
                    let now_title = self.now_on(&it.id);
                    if !now_title.is_empty() {
                        it.subtitle = if it.subtitle.is_empty() {
                            format!("Ahora: {now_title}")
                        } else {
                            format!("{} · Ahora: {now_title}", it.subtitle)
                        };
                    }
                    it
                })
                .collect();
            return Ok(Page { items, next: None });
        }
        let vod = self.load_vod()?;
        let mut seen = std::collections::HashSet::new();
        let mut items = Vec::new();
        for c in vod
            .iter()
            .filter(|c| category.is_none_or(|id| id.is_empty() || c.cat.id == id))
        {
            for it in if section == Section::Movies {
                &c.movies
            } else {
                &c.series
            } {
                if seen.insert(it.id.clone()) {
                    items.push(it.clone());
                }
            }
        }
        Ok(Page { items, next: None })
    }

    fn search(&self, query: &str, _cursor: Option<&str>) -> Result<Page> {
        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();
        if let Ok((channels, _)) = self.load_channels() {
            for ch in channels.into_iter().filter(|c| matches(&c.title, query)) {
                if seen.insert(ch.id.clone()) {
                    items.push(ch);
                }
            }
        }
        if let Ok(vod) = self.load_vod() {
            for c in &vod {
                for it in c.movies.iter().chain(c.series.iter()) {
                    if matches(&it.title, query) && seen.insert(it.id.clone()) {
                        items.push(it.clone());
                    }
                }
            }
        }
        Ok(Page { items, next: None })
    }

    fn details(&self, item: &Item) -> Result<Details> {
        let token = self.session()?;
        // Se lee el candado y se suelta antes de llamar a `server`, que también lo toma.
        let stitcher_params = self
            .session
            .lock()
            .map_err(|_| Error::new(ErrorKind::Other, "estado dañado"))?
            .stitcher_params
            .clone();
        let base = self.server("stitcher", DEFAULT_STITCHER);
        let path = if item.kind == Kind::Live || item.kind == Kind::Channel {
            format!("/stitch/hls/channel/{}/master.m3u8", item.id)
        } else {
            match item.extra.as_deref() {
                Some(p) if !p.is_empty() => p.to_string(),
                _ => format!("/stitch/hls/episode/{}/master.m3u8", item.id),
            }
        };
        let master =
            format!("{base}/v2{path}?{stitcher_params}&jwt={token}&masterJWTPassthrough=true");
        Ok(Details {
            item: item.clone(),
            description: item.description.clone(),
            sources: vec![Source {
                url: master,
                label: "HLS (automática)".into(),
                height: None,
                live: item.live,
                audio_url: None,
                drop: AD_PATTERNS.iter().map(|s| s.to_string()).collect(),
                twitch_ads: false,
                available_at: None,
            }],
        })
    }

    fn seasons(&self, series: &Item) -> Result<Vec<Season>> {
        let base = self.server("vod", DEFAULT_VOD);
        let v = self.api(&format!(
            "{base}/v4/vod/series/{}/seasons?offset=1000&page=1",
            series.id
        ))?;
        let mut out = Vec::new();
        for s in jarr(&v, "seasons") {
            let number = jnum(s, "number") as u32;
            let mut episodes = Vec::new();
            for e in jarr(s, "episodes") {
                let mut ep = vod_item(e);
                ep.kind = Kind::Episode;
                ep.season = Some(number);
                ep.episode = Some(jnum(e, "number") as u32);
                ep.subtitle = format!("T{number} · E{}", ep.episode.unwrap_or(0));
                if ep.thumb.is_none() {
                    let path = jstr(e.get("thumbnail").unwrap_or(&Value::Null), "path");
                    ep.thumb = Some(small(&path)).filter(|s| !s.is_empty());
                    ep.backdrop = Some(big(&path)).filter(|s| !s.is_empty());
                }
                if ep.description.is_empty() {
                    ep.description = series.description.clone();
                }
                episodes.push(ep);
            }
            if !episodes.is_empty() {
                out.push(Season {
                    number,
                    name: format!("Temporada {number}"),
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
    fn vod_items_map_fields() {
        let v: Value = serde_json::from_str(
            r#"{"_id":"x1","type":"series","name":"Show","genre":"Comedy","rating":"TV-14",
                "originalContentDuration":1800000,"covers":[{"aspectRatio":"16:9","url":"https://images.pluto.tv/a.jpg?w=1600"}]}"#,
        )
        .unwrap();
        let item = vod_item(&v);
        assert_eq!(item.kind, Kind::Series);
        assert_eq!(item.subtitle, "Comedia · TV-14");
        assert_eq!(item.duration_secs, Some(1800));
        assert_eq!(
            item.thumb.as_deref(),
            Some("https://images.pluto.tv/a.jpg?fm=jpg&w=480&h=270&fit=fill&q=70")
        );
    }

    #[test]
    fn literal_line_breaks_in_synopses_become_real_ones() {
        assert_eq!(clean_text("Uno.U+000A¿Dos? "), "Uno.\n¿Dos?");
        assert_eq!(clean_text("Sin saltos"), "Sin saltos");
    }

    #[test]
    fn big_asks_for_a_larger_image_than_small() {
        let url = "https://images.pluto.tv/foo/bar.jpg?fm=jpg&w=1600";
        assert!(small(url).contains("w=480&h=270"));
        assert!(big(url).contains("w=960&h=540"));
        assert_eq!(big("https://otro.test/a.jpg"), "https://otro.test/a.jpg");
        assert_eq!(big(""), "");
    }

    #[test]
    fn small_leaves_other_hosts_alone() {
        assert_eq!(small("https://otro.test/a.jpg"), "https://otro.test/a.jpg");
    }

    #[test]
    fn ad_patterns_match_lumiere_rules() {
        assert!(AD_PATTERNS
            .iter()
            .any(|p| "https://x/seg_ad%2F1.ts".contains(p)));
        assert!(!AD_PATTERNS
            .iter()
            .any(|p| "https://x/show/seg1.ts".contains(p)));
    }
}
