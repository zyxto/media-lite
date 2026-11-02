//! Proveedor de datos fijos. Sirve para desarrollar la interfaz sin red y para probar la caché.
//! Las miniaturas son imágenes PNG reales, embebidas como `data:` URL.

use base64::Engine;
use media_lite_core::{
    Caps, Category, Details, Error, Item, Kind, Page, Platform, Result, Season, Section, Source,
};

use crate::Provider;

/// Cantidad de elementos de cada sección.
const TOTAL: usize = 200;
/// Tamaño de página, igual que en las plataformas reales.
const PAGE: usize = 60;
/// Stream HLS público de muestra (Apple, con variantes H.264 y audio).
const DEMO_STREAM: &str = "https://devstreaming-cdn.apple.com/videos/streaming/examples/img_bipbop_adv_example_ts/master.m3u8";

pub struct Demo;

impl Demo {
    pub fn new() -> Demo {
        Demo
    }
}

impl Default for Demo {
    fn default() -> Demo {
        Demo::new()
    }
}

impl Provider for Demo {
    fn platform(&self) -> Platform {
        Platform::Demo
    }

    fn caps(&self) -> Caps {
        Caps {
            live: true,
            movies: true,
            series: true,
            videos: true,
            music: true,
            search: true,
            ..Caps::default()
        }
    }

    fn categories(&self, _section: Section) -> Result<Vec<Category>> {
        Ok(vec![Category {
            id: "todas".into(),
            title: "Todas".into(),
        }])
    }

    fn list(
        &self,
        section: Section,
        _category: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Page> {
        let start = parse_cursor(cursor)?;
        let end = (start + PAGE).min(TOTAL);
        let items = (start..end).map(|n| make_item(section, n)).collect();
        Ok(Page {
            items,
            next: (end < TOTAL).then(|| end.to_string()),
        })
    }

    fn search(&self, query: &str, cursor: Option<&str>) -> Result<Page> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(Page::default());
        }
        let start = parse_cursor(cursor)?;
        let matches: Vec<usize> = (0..TOTAL)
            .filter(|n| title(*n).to_lowercase().contains(&q))
            .collect();
        let end = (start + PAGE).min(matches.len());
        let items = matches[start.min(end)..end]
            .iter()
            .map(|n| make_item(Section::Videos, *n))
            .collect();
        Ok(Page {
            items,
            next: (end < matches.len()).then(|| end.to_string()),
        })
    }

    fn seasons(&self, series: &Item) -> Result<Vec<Season>> {
        if series.platform != Platform::Demo {
            return Err(Error::unsupported());
        }
        let seasons = (1..=2u32)
            .map(|number| {
                let episodes = (1..=5u32)
                    .map(|e| {
                        let mut ep =
                            make_item(Section::Series, (number as usize - 1) * 5 + e as usize);
                        ep.kind = Kind::Episode;
                        ep.season = Some(number);
                        ep.episode = Some(e);
                        ep.subtitle = format!("T{number} · E{e}");
                        ep.title = format!("{} · Episodio {e}", series.title);
                        ep
                    })
                    .collect();
                Season {
                    number,
                    name: format!("Temporada {number}"),
                    episodes,
                }
            })
            .collect();
        Ok(seasons)
    }

    fn details(&self, item: &Item) -> Result<Details> {
        if item.platform != Platform::Demo {
            return Err(Error::unsupported());
        }
        Ok(Details {
            item: item.clone(),
            description: "Elemento de prueba. Reproduce un stream HLS público de Apple, para probar el reproductor.".to_string(),
            sources: vec![Source {
                url: DEMO_STREAM.to_string(),
                label: "HLS adaptativo (prueba)".to_string(),
                height: Some(1080),
                live: false,
                audio_url: None,
                drop: Vec::new(),
                twitch_ads: false,
                available_at: None,
            }],
        })
    }
}

fn parse_cursor(cursor: Option<&str>) -> Result<usize> {
    match cursor {
        None => Ok(0),
        Some(c) => c
            .parse::<usize>()
            .map_err(|_| Error::parse("cursor inválido")),
    }
}

fn title(n: usize) -> String {
    format!("Video de prueba {:03}", n + 1)
}

fn make_item(section: Section, n: usize) -> Item {
    let kind = match section {
        Section::Live => Kind::Live,
        Section::Movies => Kind::Movie,
        Section::Series => Kind::Series,
        Section::Videos => Kind::Video,
        Section::Music => Kind::Track,
    };
    let minutes = 3 + (n * 7) % 50;
    let mut item = Item::new(Platform::Demo, kind, n.to_string(), title(n));
    item.subtitle = format!("{} · {} min", section.label(), minutes);
    item.live = section == Section::Live;
    item.duration_secs = Some((minutes * 60) as u64);
    item.thumb = Some(thumb_data_url(n));
    item
}

/// Un degradado con color propio para cada elemento, en PNG dentro de una `data:` URL.
fn thumb_data_url(n: usize) -> String {
    const W: u32 = 320;
    const H: u32 = 180;
    let hue = (n * 47 % 256) as u8;
    let img = image::RgbaImage::from_fn(W, H, |x, y| {
        let t = ((x + y) / 4) as u8;
        image::Rgba([
            hue.saturating_add(t),
            90u8.saturating_add(t / 2),
            255u8.saturating_sub(hue / 2),
            255,
        ])
    });
    let mut png = Vec::new();
    use image::ImageEncoder;
    let encoded = image::codecs::png::PngEncoder::new(&mut png).write_image(
        img.as_raw(),
        W,
        H,
        image::ExtendedColorType::Rgba8,
    );
    if encoded.is_err() {
        return String::new();
    }
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_pages_through_all_items() {
        let demo = Demo::new();
        let mut cursor: Option<String> = None;
        let mut total = 0;
        loop {
            let page = demo.list(Section::Movies, None, cursor.as_deref()).unwrap();
            total += page.items.len();
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(total, TOTAL);
    }

    #[test]
    fn search_filters_by_title() {
        let demo = Demo::new();
        let page = demo.search("prueba 042", None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].title, "Video de prueba 042");
        assert!(demo.search("   ", None).unwrap().items.is_empty());
    }

    #[test]
    fn thumbnails_are_decodable_png() {
        let url = thumb_data_url(3);
        let payload = url.split_once(',').unwrap().1;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (320, 180));
    }
}
