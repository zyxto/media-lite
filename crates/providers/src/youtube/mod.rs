//! YouTube: catálogo (portada por temas, búsqueda, canales con sus pestañas) por la API interna
//! de YouTube (innertube), con el cliente web. Portado de `Sanson-FLinux/plugins/youtube/script.js`.
//!
//! Los videos se reproducen en la fase de los streams (`/player`, ver el roadmap, sección 4b).

pub mod catalog;
pub mod clients;
pub mod innertube;

use std::path::Path;

use media_lite_core::{
    Caps, Category, Details, Error, ErrorKind, Item, Kind, Page, Platform, Result, Season, Section,
};

use crate::Provider;
use catalog::{collect, Owner};
use innertube::Innertube;

/// Pestañas de un canal (el parámetro `params` de `browse`), como las usa Sanson.
const TAB_VIDEOS: &str = "EgZ2aWRlb3PyBgQKAjoA";
const TAB_LIVE: &str = "EgdzdHJlYW1z8gYECgJ6AA%3D%3D";
const TAB_SHORTS: &str = "EgZzaG9ydHPyBgUKA5oBAA%3D%3D";

/// Canales temáticos de YouTube que son la «portada» sin cuenta. Sanson tenía cinco; hoy dos de
/// ellos (Música, que solo trae mezclas, y Videojuegos, que llega vacío) no dan videos, así que
/// quedan los que sí. Música pasa a la fase de YouTube Music (6).
const TOPICS: [(&str, &str); 2] = [
    ("UCYfdidRxbB8Qhf0Nx7ioOYw", "Noticias"),
    ("UCEgdi0XIXXZ-qJOFPf4JSKw", "Deportes"),
];
/// El tema «En vivo» de YouTube: la sección de directos.
const LIVE_TOPIC: &str = "UC4R8DWoMoI7CAwX8_LjQHig";

pub struct YouTube {
    api: Innertube,
}

impl YouTube {
    pub fn new(config_dir: &Path) -> YouTube {
        YouTube {
            api: Innertube::new(config_dir),
        }
    }

    /// Una página de un canal temático (o la de una pestaña ya conocida, con su token).
    fn browse(&self, browse_id: &str, cursor: Option<&str>) -> Result<Page> {
        let body = match cursor {
            Some(token) => serde_json::json!({ "continuation": token }),
            None => serde_json::json!({ "browseId": browse_id }),
        };
        let response = self.api.post("browse", body)?;
        let collected = collect(&response, None);
        Ok(Page {
            items: collected.items,
            next: collected.token,
        })
    }
}

impl Provider for YouTube {
    fn platform(&self) -> Platform {
        Platform::YouTube
    }

    fn caps(&self) -> Caps {
        Caps {
            live: true,
            videos: true,
            search: true,
            ..Caps::default()
        }
    }

    fn categories(&self, section: Section) -> Result<Vec<Category>> {
        match section {
            Section::Videos => Ok(TOPICS
                .iter()
                .map(|(id, title)| Category {
                    id: (*id).to_string(),
                    title: (*title).to_string(),
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    fn list(&self, section: Section, category: Option<&str>, cursor: Option<&str>) -> Result<Page> {
        let topic = match section {
            // Sin tema elegido, la portada es el primero.
            Section::Videos => category.unwrap_or(TOPICS[0].0),
            Section::Live => LIVE_TOPIC,
            _ => return Err(Error::unsupported()),
        };
        self.browse(topic, cursor)
    }

    fn search(&self, query: &str, cursor: Option<&str>) -> Result<Page> {
        let body = match cursor {
            Some(token) => serde_json::json!({ "continuation": token }),
            None => serde_json::json!({ "query": query }),
        };
        let response = self.api.post("search", body)?;
        let collected = collect(&response, None);
        Ok(Page {
            items: collected.items,
            next: collected.token,
        })
    }

    fn details(&self, _item: &Item) -> Result<Details> {
        // Los streams llegan con la fase 4b (`/player`).
        Err(Error::unsupported())
    }

    /// Un canal se abre como serie: «Videos», «En vivo» y «Shorts», cada pestaña como temporada.
    fn seasons(&self, channel: &Item) -> Result<Vec<Season>> {
        if channel.kind != Kind::Series {
            return Err(Error::unsupported());
        }
        let tabs = [
            ("Videos", TAB_VIDEOS),
            ("En vivo", TAB_LIVE),
            ("Shorts", TAB_SHORTS),
        ];
        let mut seasons = Vec::new();
        let mut owner: Option<Owner> = None;
        for (name, params) in tabs {
            let response = match self.api.post(
                "browse",
                serde_json::json!({ "browseId": channel.id, "params": params }),
            ) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("pestaña {name} de YouTube: {e}");
                    continue;
                }
            };
            if owner.is_none() {
                owner = Some(Owner {
                    name: channel.title.clone(),
                    channel_id: channel.id.clone(),
                });
            }
            let episodes = collect(&response, owner.as_ref()).items;
            if !episodes.is_empty() {
                seasons.push(Season {
                    number: seasons.len() as u32 + 1,
                    name: name.to_string(),
                    episodes,
                });
            }
        }
        if seasons.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                format!("{} no tiene videos para mostrar.", channel.title),
            ));
        }
        Ok(seasons)
    }
}
