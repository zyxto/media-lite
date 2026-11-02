//! Los últimos videos de un canal de YouTube, por su feed RSS (Atom). No pide cuenta ni usa
//! innertube: es el feed público que YouTube publica para cada canal.

use media_lite_core::{Item, Kind, Platform};

use super::catalog::thumb;

/// URL del feed de un canal (`UC…`).
pub fn feed_url(channel_id: &str) -> String {
    format!("https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}")
}

/// Los videos de un feed, del más nuevo al más viejo (como lo publica YouTube).
pub fn parse(xml: &str) -> Vec<Item> {
    xml.split("<entry>").skip(1).filter_map(entry).collect()
}

fn entry(text: &str) -> Option<Item> {
    let id = inner(text, "yt:videoId")?;
    let title = unescape(&inner(text, "title").unwrap_or_default());
    let mut item = Item::new(Platform::YouTube, Kind::Video, id.clone(), title);
    item.subtitle = inner(text, "name")
        .map(|n| unescape(&n))
        .unwrap_or_default();
    item.description = inner(text, "published").unwrap_or_default();
    item.thumb = Some(thumb(&id));
    item.extra = inner(text, "yt:channelId");
    Some(item)
}

/// El texto entre `<tag>` y `</tag>`, sin atributos.
fn inner(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim().to_string())
}

/// Las entidades que trae un feed (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#39;`).
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0"?>
<feed xmlns:yt="http://www.youtube.com/xml/schemas/2015">
  <title>Canal</title>
  <entry>
    <id>yt:video:AAAAAAAAAAA</id>
    <yt:videoId>AAAAAAAAAAA</yt:videoId>
    <yt:channelId>UCxyz</yt:channelId>
    <title>Tom &amp; Jerry &lt;3</title>
    <author><name>Canal</name></author>
    <published>2026-11-01T10:00:00+00:00</published>
  </entry>
  <entry>
    <yt:videoId>BBBBBBBBBBB</yt:videoId>
    <title>Segundo</title>
    <author><name>Canal</name></author>
    <published>2026-10-30T10:00:00+00:00</published>
  </entry>
</feed>"#;

    #[test]
    fn entries_become_videos_newest_first() {
        let items = parse(FEED);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "AAAAAAAAAAA");
        assert_eq!(items[0].title, "Tom & Jerry <3");
        assert_eq!(items[0].subtitle, "Canal");
        assert_eq!(items[0].extra.as_deref(), Some("UCxyz"));
        assert_eq!(items[0].description, "2026-11-01T10:00:00+00:00");
        assert_eq!(items[1].id, "BBBBBBBBBBB");
    }

    #[test]
    fn feed_url_uses_the_channel_id() {
        assert_eq!(
            feed_url("UCxyz"),
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCxyz"
        );
    }
}
