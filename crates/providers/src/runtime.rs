//! Runtime (plataforma OTTera): las filas de sus secciones (en vivo, películas, series) salen de
//! `getconfiguration`. El video: un MP4 de Kaltura para películas y episodios, y HLS para los canales.
//! Portado de `lumiere/src/runtime.cpp`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use media_lite_core::http::{self, Agent};
use media_lite_core::{
    Caps, Category, Details, Error, ErrorKind, Item, Kind, Page, Platform, Result, Season, Section,
    Source,
};
use serde_json::Value;

use crate::util::{jarr, jbool, jnum, jstr, now, pct_encode};
use crate::Provider;

const API: &str = "https://api-ott.runtime.tv/";
const BASE: &str = "platform=web&partner=runtime&version=11.0&language=es";
const MAX_JSON: u64 = 8 * 1024 * 1024;
const CONF_TTL: u64 = 3600;
const LISTS_TTL: u64 = 1800;
const PAGE: u64 = 200;

#[derive(Clone)]
struct Row {
    cat: Category,
    endpoint: String,
    params: Vec<(String, String)>,
}

#[derive(Default)]
struct Cache {
    sections: HashMap<String, Vec<Row>>,
    conf_at: u64,
    lists: HashMap<String, Vec<Item>>,
    lists_at: u64,
}

pub struct Runtime {
    agent: Agent,
    cache: Mutex<Cache>,
}

impl Default for Runtime {
    fn default() -> Runtime {
        Runtime::new()
    }
}

impl Runtime {
    pub fn new() -> Runtime {
        Runtime {
            agent: http::agent(),
            cache: Mutex::new(Cache::default()),
        }
    }

    fn call(&self, action: &str, query: &str) -> Result<String> {
        let url = format!("{API}{action}?{BASE}{query}&timestamp={}", now());
        let response = http::get(&self.agent, &url, &[], MAX_JSON)
            .map_err(|e| Error::network(format!("Runtime: {}", e.message)))?;
        Ok(response.text())
    }

    fn call_json(&self, action: &str, query: &str) -> Result<Value> {
        let text = self.call(action, query)?;
        serde_json::from_str(&text).map_err(|_| Error::parse("Runtime: respuesta ilegible"))
    }

    /// Las filas de cada sección. Se cachean una hora.
    fn load_conf(&self) -> Result<HashMap<String, Vec<Row>>> {
        {
            let c = self
                .cache
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
            if !c.sections.is_empty() && now() - c.conf_at < CONF_TTL {
                return Ok(c.sections.clone());
            }
        }
        let v = self.call_json("getconfiguration", "")?;
        let mut sections: HashMap<String, Vec<Row>> = HashMap::new();
        for s in jarr(&v, "sections") {
            let name = jstr(s, "section");
            for (n, i) in jarr(s, "items").iter().enumerate() {
                let endpoint = jstr(i, "endpoint");
                let title = jstr(i, "title");
                let params: Vec<(String, String)> = i
                    .get("parameters")
                    .and_then(Value::as_object)
                    .map(|obj| {
                        obj.iter()
                            .map(|(k, val)| (k.clone(), val.as_str().unwrap_or("").to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                // Solo las filas que listan cosas: no "Favoritos" ni "Recientes" (piden cuenta).
                if endpoint == "getreferencedobjects"
                    && !jbool(i, "authenticated")
                    && !title.is_empty()
                {
                    sections.entry(name.clone()).or_default().push(Row {
                        cat: Category {
                            id: format!("{name}/{n}"),
                            title,
                        },
                        endpoint,
                        params,
                    });
                }
            }
        }
        if sections.is_empty() {
            return Err(Error::parse("Runtime no mandó sus secciones"));
        }
        let mut c = self
            .cache
            .lock()
            .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
        c.sections = sections.clone();
        c.conf_at = now();
        Ok(sections)
    }

    /// Objetos de una consulta paginada (hasta 1000), filtrados por tipo.
    fn objects(&self, endpoint: &str, query: &str, want: Option<Kind>) -> Result<Vec<Item>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for start in (0..1000).step_by(PAGE as usize) {
            let v = self.call_json(endpoint, &format!("{query}&max={PAGE}&start={start}"))?;
            let got = jarr(&v, "objects");
            for o in got {
                if !playable(o) {
                    continue;
                }
                let item = object_item(o);
                let kind_ok = want.is_none_or(|k| item.kind == k);
                if kind_ok && !item.id.is_empty() && seen.insert(item.id.clone()) {
                    out.push(item);
                }
            }
            let total = jnum(&v, "total_results") as u64;
            if (got.len() as u64) < PAGE || start + PAGE >= total {
                break;
            }
        }
        Ok(out)
    }
}

fn section_name(section: Section) -> &'static str {
    match section {
        Section::Live => "epgchannelssection",
        Section::Movies => "moviessection",
        _ => "seriessection",
    }
}

/// Sin DRM y sin suscripción: lo que no se puede reproducir gratis se oculta.
fn playable(o: &Value) -> bool {
    let kind = jstr(o, "type");
    (kind == "video" || kind == "show")
        && !jbool(o, "is_premium")
        && !jbool(o, "premium_content")
        && !jbool(o, "drm")
        && !jbool(o, "reg_users_only")
}

fn object_item(o: &Value) -> Item {
    let kind_str = jstr(o, "type");
    let video_type = jstr(o, "video_type");
    let (kind, live) = if kind_str == "show" {
        (Kind::Series, false)
    } else if video_type == "linear" || video_type == "live" {
        (Kind::Channel, true)
    } else if video_type == "episode" {
        (Kind::Episode, false)
    } else {
        (Kind::Movie, false)
    };
    let mut item = Item::new(Platform::Runtime, kind, jstr(o, "id"), jstr(o, "name"));
    item.live = live;
    item.description = {
        let long = jstr(o, "long_description");
        if long.is_empty() {
            jstr(o, "short_description")
        } else {
            long
        }
    };
    let thumb = {
        let wide = jstr(o, "widescreen_thumbnail_url");
        if wide.is_empty() {
            jstr(o, "thumbnail_url")
        } else {
            wide
        }
    };
    item.thumb = Some(thumb).filter(|s| !s.is_empty());
    if !live {
        let secs = jnum(o, "duration");
        item.duration_secs = Some(secs as u64).filter(|s| *s > 0);
    }
    let mut parts: Vec<String> = Vec::new();
    let year = jstr(o, "year");
    if !live && !year.is_empty() && year != "0" {
        parts.push(year);
    }
    let rating = jstr(o, "mpaa_rating");
    if !live && !rating.is_empty() {
        parts.push(rating);
    }
    if live {
        parts.push("En vivo".into());
    }
    item.subtitle = parts.join(" · ");
    item
}

/// Recorre un texto (JavaScript del reproductor) y saca las direcciones que contienen `needle`.
/// Las direcciones van entre comillas y con `\/` y `&` escapados.
fn urls_in(js: &str, needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = js[from..].find(needle) {
        let at = from + found;
        let quote = js[..at].rfind(['"', '\'']);
        let start = quote.and_then(|q| js[q..].find("http").map(|i| q + i));
        let end = js[at..].find(['"', '\'', ' ']).map(|i| at + i);
        match (start, end) {
            (Some(a), Some(e)) if a <= at && at - a <= 4000 => {
                out.push(unescape(&js[a..e]));
                from = e;
            }
            _ => from = at + 1,
        }
    }
    out
}

fn unescape(raw: &str) -> String {
    raw.replace("\\/", "/").replace("\\u0026", "&")
}

impl Provider for Runtime {
    fn platform(&self) -> Platform {
        Platform::Runtime
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
            return Ok(vec![Category {
                id: "live".into(),
                title: "Canales en vivo".into(),
            }]);
        }
        let sections = self.load_conf()?;
        let rows = sections
            .get(section_name(section))
            .cloned()
            .unwrap_or_default();
        if rows.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "Runtime no tiene esta sección",
            ));
        }
        Ok(rows.into_iter().map(|r| r.cat).collect())
    }

    fn list(
        &self,
        section: Section,
        category: Option<&str>,
        _cursor: Option<&str>,
    ) -> Result<Page> {
        let key = format!("{}|{}", section.key(), category.unwrap_or(""));
        {
            let c = self
                .cache
                .lock()
                .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
            if now() - c.lists_at < LISTS_TTL {
                if let Some(items) = c.lists.get(&key) {
                    return Ok(Page {
                        items: items.clone(),
                        next: None,
                    });
                }
            }
        }
        let items = if section == Section::Live {
            self.objects(
                "getobjects",
                "&object_type=video&video_type=linear&image_width=366&image_format=widescreen",
                Some(Kind::Channel),
            )?
        } else {
            let sections = self.load_conf()?;
            let rows = sections
                .get(section_name(section))
                .cloned()
                .unwrap_or_default();
            let row = rows
                .iter()
                .find(|r| category.is_none_or(|c| c.is_empty() || r.cat.id == c))
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::Unavailable,
                        "Runtime: no se encontró la categoría",
                    )
                })?;
            let query: String = row
                .params
                .iter()
                .filter(|(k, _)| k != "max" && k != "start")
                .map(|(k, v)| format!("&{k}={}", pct_encode(v)))
                .collect();
            let want = if section == Section::Movies {
                Kind::Movie
            } else {
                Kind::Series
            };
            self.objects(&row.endpoint, &query, Some(want))?
        };
        let mut c = self
            .cache
            .lock()
            .map_err(|_| Error::new(ErrorKind::Other, "caché dañada"))?;
        if now() - c.lists_at >= LISTS_TTL {
            c.lists.clear();
            c.lists_at = now();
        }
        c.lists.insert(key, items.clone());
        Ok(Page { items, next: None })
    }

    fn search(&self, query: &str, _cursor: Option<&str>) -> Result<Page> {
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let key = format!("&key={}&image_width=366", pct_encode(query));
        for kind in [
            "&object_type=video&video_type=linear",
            "&object_type=show",
            "&object_type=video&video_type=non_episode",
        ] {
            for item in self.objects("search", &format!("{key}{kind}"), None)? {
                if item.kind != Kind::Episode && seen.insert(item.id.clone()) {
                    items.push(item);
                }
            }
        }
        Ok(Page { items, next: None })
    }

    fn details(&self, item: &Item) -> Result<Details> {
        let text = self.call(
            "embeddedVideoPlayer",
            &format!("&div_id=video_player&image_width=1280&mute=false&object_type=video&object_id={0}&id={0}", item.id),
        )?;
        let mut sources = Vec::new();
        if item.live {
            if let Some(url) = urls_in(&text, "playlist.m3u8").into_iter().next() {
                sources.push(Source {
                    url,
                    label: "HLS".into(),
                    height: None,
                    live: true,
                    audio_url: None,
                    drop: Vec::new(),
                });
            }
        } else if let Some(url) = urls_in(&text, "/a.mp4").into_iter().next() {
            sources.push(Source {
                url,
                label: "MP4".into(),
                height: Some(540),
                live: false,
                audio_url: None,
                drop: Vec::new(),
            });
        }
        if sources.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "Runtime no dio el video (puede ser solo para suscriptores)",
            ));
        }
        Ok(Details {
            item: item.clone(),
            description: item.description.clone(),
            sources,
        })
    }

    fn seasons(&self, series: &Item) -> Result<Vec<Season>> {
        let mut episodes: Vec<Item> = Vec::new();
        let mut seen = HashSet::new();
        for start in (0..2000).step_by(PAGE as usize) {
            let v = self.call_json(
                "getreferencedobjects",
                &format!(
                    "&parent_type=show&parent_id={}&r_video_type=episode&parent_meta=1&image_width=366&image_format=widescreen&max={PAGE}&start={start}",
                    series.id
                ),
            )?;
            let got = jarr(&v, "objects");
            for o in got {
                if !playable(o) {
                    continue;
                }
                let mut ep = object_item(o);
                ep.kind = Kind::Episode;
                let season = jnum(o, "season_number").max(jnum(o, "season")).max(1.0) as u32;
                let number = jnum(o, "episode_number")
                    .max(jnum(o, "episode"))
                    .max((episodes.len() + 1) as f64) as u32;
                ep.season = Some(season);
                ep.episode = Some(number);
                ep.subtitle = format!("T{season} · E{number}");
                if ep.thumb.is_none() {
                    ep.thumb = series.thumb.clone();
                }
                if seen.insert(ep.id.clone()) {
                    episodes.push(ep);
                }
            }
            if (got.len() as u64) < PAGE {
                break;
            }
        }
        let mut by_season: BTreeMap<u32, Vec<Item>> = BTreeMap::new();
        for ep in episodes {
            by_season
                .entry(ep.season.unwrap_or(1))
                .or_default()
                .push(ep);
        }
        let out: Vec<Season> = by_season
            .into_iter()
            .map(|(number, episodes)| Season {
                number,
                name: format!("Temporada {number}"),
                episodes,
            })
            .collect();
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
    fn finds_quoted_urls_and_unescapes_them() {
        let js = r#"var p = {src:"https:\/\/cdn.test\/a.mp4?x=1\u0026y=2", other:"ok"};"#;
        assert_eq!(
            urls_in(js, "/a.mp4"),
            vec!["https://cdn.test/a.mp4?x=1&y=2".to_string()]
        );
    }

    #[test]
    fn drm_and_premium_are_hidden() {
        let premium: Value = serde_json::from_str(r#"{"type":"video","drm":true}"#).unwrap();
        let free: Value = serde_json::from_str(r#"{"type":"video","video_type":"movie"}"#).unwrap();
        assert!(!playable(&premium));
        assert!(playable(&free));
    }

    #[test]
    fn linear_objects_are_channels() {
        let o: Value = serde_json::from_str(
            r#"{"type":"video","video_type":"linear","id":"c1","name":"Canal"}"#,
        )
        .unwrap();
        let item = object_item(&o);
        assert_eq!(item.kind, Kind::Channel);
        assert!(item.live);
    }
}
