//! Lectura de las respuestas de innertube: videos, directos, Shorts y canales, y el token de la
//! página siguiente. Portado de `Sanson-FLinux/plugins/youtube/script.js`.
//!
//! YouTube cambia la forma de sus respuestas seguido, así que cada renderer se busca en todo el
//! árbol en vez de seguir una ruta fija.

use std::collections::HashSet;

use media_lite_core::{Item, Kind, Platform};
use serde_json::Value;

/// Claves que no llevan contenido (menús, cabeceras, seguimiento). Se saltan al buscar.
const SKIP: [&str; 12] = [
    "frameworkUpdates",
    "topbar",
    "header",
    "responseContext",
    "menu",
    "sidebar",
    "trackingParams",
    "commandMetadata",
    "loggingDirectives",
    "accessibility",
    "playerOverlays",
    "engagementPanels",
];

const THUMB: &str = "https://i.ytimg.com/vi";

/// Quién publica un elemento, cuando la respuesta no lo dice (una pestaña de un canal).
#[derive(Clone, Debug, Default)]
pub struct Owner {
    pub name: String,
    pub channel_id: String,
}

/// Lo que se saca de una respuesta: los elementos y el token de la página siguiente.
#[derive(Debug, Default)]
pub struct Collected {
    pub items: Vec<Item>,
    pub token: Option<String>,
}

/// Todos los videos, directos, Shorts y canales de `root`, sin repetir ninguno.
pub fn collect(root: &Value, owner: Option<&Owner>) -> Collected {
    let mut out = Collected::default();
    let mut seen = HashSet::new();
    walk(root, 0, owner, &mut out, &mut seen);
    out
}

fn walk(
    value: &Value,
    depth: usize,
    owner: Option<&Owner>,
    out: &mut Collected,
    seen: &mut HashSet<String>,
) {
    if depth > 80 {
        return;
    }
    match value {
        Value::Array(list) => {
            for x in list {
                walk(x, depth + 1, owner, out, seen);
            }
        }
        Value::Object(map) => {
            for (key, v) in map {
                if SKIP.contains(&key.as_str()) {
                    continue;
                }
                let item = match key.as_str() {
                    "videoRenderer"
                    | "gridVideoRenderer"
                    | "compactVideoRenderer"
                    | "playlistVideoRenderer" => video_renderer(v, owner),
                    "lockupViewModel" => lockup(v, owner),
                    "reelItemRenderer" | "shortsLockupViewModel" => short(v, owner),
                    "channelRenderer" | "gridChannelRenderer" => channel_renderer(v),
                    "continuationItemRenderer" => {
                        if let Some(token) =
                            find_key(v, "continuationCommand", 0).and_then(|c| c["token"].as_str())
                        {
                            out.token = Some(token.to_string());
                        }
                        continue;
                    }
                    _ => {
                        walk(v, depth + 1, owner, out, seen);
                        continue;
                    }
                };
                if let Some(item) = item {
                    if seen.insert(format!("{:?}:{}", item.kind, item.id)) {
                        out.items.push(item);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Primer valor de `key` en `value`, buscando a cualquier profundidad (hasta 25 niveles).
pub fn find_key<'a>(value: &'a Value, key: &str, depth: usize) -> Option<&'a Value> {
    if depth > 25 {
        return None;
    }
    if let Some(v) = value.get(key) {
        return Some(v);
    }
    match value {
        Value::Object(map) => map.values().find_map(|v| find_key(v, key, depth + 1)),
        Value::Array(list) => list.iter().find_map(|v| find_key(v, key, depth + 1)),
        _ => None,
    }
}

/// Texto de un campo de YouTube: un texto plano, `simpleText`, la suma de `runs` o `content`.
pub fn text(value: &Value) -> String {
    if let Some(plain) = value.as_str() {
        return plain.to_string();
    }
    if let Some(simple) = value["simpleText"].as_str() {
        return simple.to_string();
    }
    value["runs"]
        .as_array()
        .map(|runs| {
            runs.iter()
                .filter_map(|r| r["text"].as_str())
                .collect::<String>()
        })
        .or_else(|| value["content"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Canal que publica un texto de byline: su nombre y su id (`UC…`).
fn owner_of(value: &Value) -> Owner {
    let run = value["runs"].as_array().and_then(|runs| {
        runs.iter()
            .find(|r| r["navigationEndpoint"]["browseEndpoint"].is_object())
    });
    Owner {
        name: text(value),
        channel_id: run
            .and_then(|r| r["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str())
            .unwrap_or("")
            .to_string(),
    }
}

/// Miniatura de un video: `mqdefault` (320×180), que basta para las tarjetas.
pub fn thumb(id: &str) -> String {
    format!("{THUMB}/{id}/mqdefault.jpg")
}

/// Segundos de una duración «1:02:03» o «4:05».
pub fn secs_of(text: &str) -> Option<u64> {
    let parts: Vec<u64> = text
        .trim()
        .split(':')
        .map(|p| p.trim().parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()?;
    match parts.as_slice() {
        [m, s] => Some(m * 60 + s),
        [h, m, s] => Some(h * 3600 + m * 60 + s),
        _ => None,
    }
}

fn video_item(id: &str, title: &str, owner: &Owner, live: bool, short: bool) -> Item {
    let kind = if live {
        Kind::Live
    } else if short {
        Kind::Short
    } else {
        Kind::Video
    };
    let mut item = Item::new(Platform::YouTube, kind, id, title);
    item.subtitle = owner.name.clone();
    item.thumb = Some(thumb(id));
    item.live = live;
    if !owner.channel_id.is_empty() {
        item.extra = Some(owner.channel_id.clone());
    }
    item
}

/// Un video de la forma clásica (`videoRenderer` y sus parientes).
fn video_renderer(v: &Value, fallback: Option<&Owner>) -> Option<Item> {
    let id = v["videoId"].as_str()?;
    if v.get("upcomingEventData").is_some() {
        return None;
    }
    let title = text(&v["title"]);
    let title = if title.is_empty() {
        text(&v["headline"])
    } else {
        title
    };
    if title.is_empty() {
        // Anuncios y tarjetas promocionadas no traen datos.
        return None;
    }
    let owner = owner_of_or(
        v,
        &["ownerText", "longBylineText", "shortBylineText"],
        fallback,
    );
    let live = badge_live(v["badges"].as_array()) || badge_live(v["thumbnailOverlays"].as_array());
    let mut item = video_item(id, &title, &owner, live, false);
    if !live {
        item.duration_secs = secs_of(&text(&v["lengthText"])).or_else(|| overlay_duration(v));
    }
    item.description = description(&text(&v["viewCountText"]), &text(&v["publishedTimeText"]));
    Some(item)
}

/// Lo que se ve al lado del título: vistas y antigüedad.
fn description(views: &str, when: &str) -> String {
    [views, when]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Duración que trae la miniatura (`thumbnailOverlayTimeStatusRenderer`), si `lengthText` no la da.
fn overlay_duration(v: &Value) -> Option<u64> {
    v["thumbnailOverlays"]
        .as_array()?
        .iter()
        .find_map(|x| secs_of(&text(&x["thumbnailOverlayTimeStatusRenderer"]["text"])))
}

fn owner_of_or(v: &Value, keys: &[&str], fallback: Option<&Owner>) -> Owner {
    for key in keys {
        if v.get(*key).is_some() {
            let owner = owner_of(&v[*key]);
            if !owner.name.is_empty() {
                return owner;
            }
        }
    }
    fallback.cloned().unwrap_or_default()
}

fn badge_live(list: Option<&Vec<Value>>) -> bool {
    list.is_some_and(|items| {
        items.iter().any(|b| {
            let style = b["metadataBadgeRenderer"]["style"].as_str().unwrap_or("");
            let overlay = b["thumbnailOverlayTimeStatusRenderer"]["style"]
                .as_str()
                .unwrap_or("");
            style.contains("LIVE") || overlay == "LIVE"
        })
    })
}

/// Todas las etiquetas (`thumbnailBadgeViewModel`) de una miniatura, estén en el overlay que estén.
fn thumbnail_badges(value: &Value, depth: usize, out: &mut Vec<Value>) {
    if depth > 25 {
        return;
    }
    match value {
        Value::Object(map) => {
            if let Some(badge) = map.get("thumbnailBadgeViewModel") {
                out.push(badge.clone());
            }
            for v in map.values() {
                thumbnail_badges(v, depth + 1, out);
            }
        }
        Value::Array(list) => {
            for v in list {
                thumbnail_badges(v, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// Un video en la forma nueva (`lockupViewModel`). Las listas de reproducción se saltan.
fn lockup(l: &Value, fallback: Option<&Owner>) -> Option<Item> {
    let id = l["contentId"].as_str()?;
    if l["contentType"]
        .as_str()
        .is_some_and(|t| t != "LOCKUP_CONTENT_TYPE_VIDEO")
    {
        return None;
    }
    let meta = &l["metadata"]["lockupMetadataViewModel"];
    let title = text(&meta["title"]);
    if title.is_empty() {
        return None;
    }
    let mut owner = fallback.cloned().unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    for row in meta["metadata"]["contentMetadataViewModel"]["metadataRows"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        for part in row["metadataParts"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let t = text(&part["text"]);
            if t.is_empty() {
                continue;
            }
            let label = part["accessibilityLabel"]
                .as_str()
                .unwrap_or(&t)
                .to_string();
            if label.contains("hace")
                || label.contains("ago")
                || label.contains("visualiz")
                || label.contains("views")
            {
                parts.push(t);
            } else if owner.name.is_empty() {
                owner.name = t;
            }
        }
    }
    // Duración y «EN DIRECTO» están en las etiquetas de la miniatura, en cualquier overlay.
    let mut badges = Vec::new();
    thumbnail_badges(&l["contentImage"], 0, &mut badges);
    let live = badges.iter().any(|b| {
        b["badgeStyle"].as_str().is_some_and(|s| s.contains("LIVE"))
            || text(&b["text"]).to_uppercase().contains("EN DIRECTO")
    });
    let duration = badges.iter().find_map(|b| secs_of(&text(&b["text"])));
    let mut item = video_item(id, &title, &owner, live, false);
    if !live {
        item.duration_secs = duration;
    }
    item.description = parts.join(" · ");
    Some(item)
}

/// Un Short: su id sale de `videoId` o de `reelWatchEndpoint`.
fn short(s: &Value, fallback: Option<&Owner>) -> Option<Item> {
    let id = s["videoId"].as_str().or_else(|| {
        find_key(&s["onTap"], "reelWatchEndpoint", 0).and_then(|e| e["videoId"].as_str())
    })?;
    let title = {
        let headline = text(&s["headline"]);
        if headline.is_empty() {
            text(&s["overlayMetadata"]["primaryText"])
        } else {
            headline
        }
    };
    let owner = fallback.cloned().unwrap_or_default();
    Some(video_item(id, &title, &owner, false, true))
}

/// Un canal. Se abre como serie: sus pestañas son sus temporadas.
fn channel_renderer(c: &Value) -> Option<Item> {
    let id = c["channelId"].as_str()?;
    let mut item = Item::new(Platform::YouTube, Kind::Series, id, text(&c["title"]));
    item.thumb = c["thumbnail"]["thumbnails"]
        .as_array()
        .and_then(|t| t.last())
        .and_then(|t| t["url"].as_str())
        .map(|u| {
            if let Some(rest) = u.strip_prefix("//") {
                format!("https://{rest}")
            } else {
                u.to_string()
            }
        });
    let subs = [&c["subscriberCountText"], &c["videoCountText"]]
        .iter()
        .map(|v| text(v))
        .find(|t| t.contains("suscript") || t.contains("subscri"))
        .unwrap_or_default();
    item.subtitle = subs;
    Some(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secs_of_reads_clock_durations() {
        assert_eq!(secs_of("4:05"), Some(245));
        assert_eq!(secs_of("1:02:03"), Some(3723));
        assert_eq!(secs_of("EN VIVO"), None);
    }

    #[test]
    fn video_renderer_becomes_a_video_with_owner_and_duration() {
        let root = json!({ "contents": [ { "videoRenderer": {
            "videoId": "abcdefghijk",
            "title": { "runs": [ { "text": "Un video" } ] },
            "ownerText": { "runs": [ { "text": "Canal", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCabc" } } } ] },
            "lengthText": { "simpleText": "4:05" },
            "viewCountText": { "simpleText": "1.234 visualizaciones" },
            "publishedTimeText": { "simpleText": "hace 3 días" }
        } } ] });
        let c = collect(&root, None);
        assert_eq!(c.items.len(), 1);
        let item = &c.items[0];
        assert_eq!(item.kind, Kind::Video);
        assert_eq!(item.id, "abcdefghijk");
        assert_eq!(item.subtitle, "Canal");
        assert_eq!(item.extra.as_deref(), Some("UCabc"));
        assert_eq!(item.duration_secs, Some(245));
        assert_eq!(item.description, "1.234 visualizaciones · hace 3 días");
    }

    #[test]
    fn live_badges_make_live_items_and_skip_duration() {
        let root = json!({ "videoRenderer": {
            "videoId": "live0000001", "title": { "simpleText": "En directo" },
            "badges": [ { "metadataBadgeRenderer": { "style": "BADGE_STYLE_TYPE_LIVE_NOW" } } ],
            "lengthText": { "simpleText": "9:99" }
        }});
        let c = collect(&root, None);
        assert_eq!(c.items[0].kind, Kind::Live);
        assert!(c.items[0].live);
        assert_eq!(c.items[0].duration_secs, None);
    }

    #[test]
    fn lockup_duration_is_read_from_any_thumbnail_badge() {
        // La forma nueva de la búsqueda: el «3:34» es texto plano, en otro overlay.
        let root = json!({ "lockupViewModel": {
            "contentId": "dQw4w9WgXcQ", "contentType": "LOCKUP_CONTENT_TYPE_VIDEO",
            "contentImage": { "thumbnailViewModel": { "overlays": [
                { "thumbnailOverlayBadgeViewModel": { "thumbnailBadges": [ { "thumbnailBadgeViewModel": { "text": "Mix" } } ] } },
                { "thumbnailBottomOverlayViewModel": { "badges": [ { "thumbnailBadgeViewModel": { "text": "3:34", "badgeStyle": "THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT" } } ] } }
            ] } },
            "metadata": { "lockupMetadataViewModel": { "title": { "content": "Un video" } } }
        }});
        let c = collect(&root, None);
        assert_eq!(c.items.len(), 1);
        assert_eq!(c.items[0].kind, Kind::Video);
        assert_eq!(c.items[0].duration_secs, Some(214));
    }

    #[test]
    fn continuation_token_and_duplicates() {
        let root = json!({ "contents": [
            { "videoRenderer": { "videoId": "dup00000001", "title": { "simpleText": "A" } } },
            { "videoRenderer": { "videoId": "dup00000001", "title": { "simpleText": "A" } } },
            { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "TOKEN123" } } } }
        ]});
        let c = collect(&root, None);
        assert_eq!(c.items.len(), 1);
        assert_eq!(c.token.as_deref(), Some("TOKEN123"));
    }

    #[test]
    fn channel_renderer_is_a_series_with_subscribers() {
        let root = json!({ "channelRenderer": {
            "channelId": "UCxyz",
            "title": { "simpleText": "Un canal" },
            "subscriberCountText": { "simpleText": "1,2 M de suscriptores" },
            "thumbnail": { "thumbnails": [ { "url": "//yt3.example/a.jpg" } ] }
        }});
        let c = collect(&root, None);
        assert_eq!(c.items[0].kind, Kind::Series);
        assert_eq!(c.items[0].id, "UCxyz");
        assert_eq!(c.items[0].subtitle, "1,2 M de suscriptores");
        assert_eq!(
            c.items[0].thumb.as_deref(),
            Some("https://yt3.example/a.jpg")
        );
    }

    #[test]
    fn promoted_cards_without_title_are_skipped() {
        let root = json!({ "videoRenderer": { "videoId": "ad000000001" } });
        assert!(collect(&root, None).items.is_empty());
    }
}
