//! Los formatos de una respuesta de `/player`: descifrado (`signatureCipher`), el desafío `n`, la
//! elección de formatos H.264 y la espera del anuncio previo. Portado de yt-dlp (`_video.py`,
//! `_get_available_at_timestamp`) y de `Sanson-FLinux/plugins/youtube/script.js`.

use std::collections::{BTreeSet, HashMap};

use media_lite_core::Source;
use media_lite_ytsolver::solve::Solutions;
use serde_json::Value;

use super::catalog::find_key;
use crate::util::pct_encode;

/// Un formato del `streamingData`, con lo necesario para elegirlo.
#[derive(Clone, Debug, PartialEq)]
pub struct Format {
    pub itag: u32,
    pub mime: String,
    pub codecs: String,
    pub url: Option<String>,
    pub cipher: Option<String>,
    pub height: Option<u32>,
    pub bitrate: u64,
    pub drm: bool,
}

impl Format {
    pub fn from_json(v: &Value) -> Option<Format> {
        let itag = v["itag"].as_u64()? as u32;
        let mime = v["mimeType"].as_str().unwrap_or("").to_string();
        let codecs = codecs_of(&mime);
        Some(Format {
            itag,
            mime,
            codecs,
            url: v["url"].as_str().map(str::to_string),
            cipher: v["signatureCipher"]
                .as_str()
                .or_else(|| v["cipher"].as_str())
                .map(str::to_string),
            height: v["height"].as_u64().map(|h| h as u32),
            bitrate: v["bitrate"].as_u64().unwrap_or(0),
            drm: v.get("drmFamilies").is_some(),
        })
    }

    /// Video H.264 en MP4 (avc1), el único que decodifica el procesador.
    pub fn is_h264_video(&self) -> bool {
        self.mime.starts_with("video/mp4") && self.codecs.contains("avc1")
    }

    /// Audio AAC en MP4 (mp4a).
    pub fn is_aac_audio(&self) -> bool {
        self.mime.starts_with("audio/mp4") && self.codecs.contains("mp4a")
    }

    /// Video y audio juntos (itag 18 y similares).
    pub fn is_muxed_h264(&self) -> bool {
        self.mime.starts_with("video/mp4")
            && self.codecs.contains("avc1")
            && self.codecs.contains("mp4a")
    }
}

/// Los `codecs="…"` de un tipo MIME (`video/mp4; codecs="avc1.4d401e"`).
fn codecs_of(mime: &str) -> String {
    mime.split_once("codecs=")
        .map(|(_, rest)| rest.trim_matches(|c| c == '"' || c == ' ').to_lowercase())
        .unwrap_or_default()
}

/// Todos los formatos de `streamingData` (`formats` y `adaptiveFormats`).
pub fn formats(streaming: &Value) -> Vec<Format> {
    ["formats", "adaptiveFormats"]
        .iter()
        .flat_map(|key| streaming[*key].as_array().cloned().unwrap_or_default())
        .filter_map(|v| Format::from_json(&v))
        .collect()
}

/// Partes de un `signatureCipher` (`s`, `sp` y `url`).
#[derive(Clone, Debug, PartialEq)]
pub struct Cipher {
    pub url: String,
    pub signature: String,
    pub sp: String,
}

pub fn parse_cipher(text: &str) -> Option<Cipher> {
    let mut url = None;
    let mut signature = None;
    let mut sp = "signature".to_string();
    for (key, value) in query_pairs(text) {
        match key.as_str() {
            "url" => url = Some(value),
            "s" => signature = Some(value),
            "sp" => sp = value,
            _ => {}
        }
    }
    Some(Cipher {
        url: url?,
        signature: signature?,
        sp,
    })
}

/// Los desafíos que hacen falta para resolver estos formatos: `sig` (las firmas cifradas) y `n`.
pub fn challenges(formats: &[&Format]) -> (Vec<String>, Vec<String>) {
    let mut sig = BTreeSet::new();
    let mut n = BTreeSet::new();
    for f in formats {
        if let Some(cipher) = f.cipher.as_deref().and_then(parse_cipher) {
            sig.insert(cipher.signature.clone());
            if let Some(v) = query_param(&cipher.url, "n") {
                n.insert(v);
            }
        } else if let Some(url) = f.url.as_deref() {
            if let Some(v) = query_param(url, "n") {
                n.insert(v);
            }
        }
    }
    (sig.into_iter().collect(), n.into_iter().collect())
}

/// La URL final de un formato, con la firma descifrada y el `n` resuelto. Una firma cifrada siempre
/// necesita su solución (`None` si falta). Un `n` sin solución: con `strict` el formato no sirve; sin
/// `strict` (clientes que no usan JavaScript) la URL se usa como viene.
pub fn resolve_url(format: &Format, solved: &Solutions, strict: bool) -> Option<String> {
    let mut url = match format.cipher.as_deref().and_then(parse_cipher) {
        Some(cipher) => {
            let sig = solved.sig.get(&cipher.signature)?;
            set_param(&cipher.url, &cipher.sp, sig)
        }
        None => format.url.clone()?,
    };
    if let Some(n) = query_param(&url, "n") {
        match solved.n.get(&n) {
            Some(resolved) => url = set_param(&url, "n", resolved),
            None if strict => return None,
            None => {}
        }
    }
    Some(url)
}

/// Los formatos utilizables con sus URLs ya resueltas: sin DRM y sin SABR (sin URL ni cifrado).
pub fn playable<'a>(
    formats: &'a [Format],
    solved: &Solutions,
    strict: bool,
) -> Vec<(&'a Format, String)> {
    formats
        .iter()
        .filter(|f| !f.drm)
        .filter_map(|f| resolve_url(f, solved, strict).map(|url| (f, url)))
        .collect()
}

/// Elige las fuentes que se muestran: un DASH H.264 por altura (con el audio AAC aparte), o el
/// HLS si no hay DASH, o el itag 18 para 360p si es lo único. Los directos son siempre HLS.
pub fn choose_sources(formats: &[(&Format, String)], hls: Option<&str>, live: bool) -> Vec<Source> {
    if live {
        return hls
            .map(|url| vec![hls_source(url, true)])
            .unwrap_or_default();
    }
    let audio = audio_url(formats);
    // Mejor video por altura: el de mayor bitrate.
    let mut best: HashMap<u32, (u64, String)> = HashMap::new();
    for (f, url) in formats {
        if f.is_h264_video() && !f.is_muxed_h264() {
            if let Some(h) = f.height.filter(|h| *h > 0) {
                let entry = best.entry(h).or_insert((0, String::new()));
                if f.bitrate >= entry.0 {
                    *entry = (f.bitrate, url.clone());
                }
            }
        }
    }
    let mut sources: Vec<Source> = best
        .into_iter()
        .map(|(h, (_, url))| Source {
            url,
            label: format!("{h}p"),
            height: Some(h),
            live: false,
            audio_url: audio.clone(),
            drop: Vec::new(),
            twitch_ads: false,
            available_at: None,
        })
        .collect();
    // Itag 18 (video y audio juntos, 360p) solo si no hay un DASH de 360p.
    if let Some((_, url)) = formats.iter().find(|(f, _)| f.itag == 18) {
        if !sources.iter().any(|s| s.height == Some(360)) {
            sources.push(Source {
                url: url.clone(),
                label: "360p".into(),
                height: Some(360),
                live: false,
                audio_url: None,
                drop: Vec::new(),
                twitch_ads: false,
                available_at: None,
            });
        }
    }
    if sources.is_empty() {
        if let Some(url) = hls {
            return vec![hls_source(url, false)];
        }
        return Vec::new();
    }
    sources.sort_by_key(|s| std::cmp::Reverse(s.height));
    sources
}

fn hls_source(url: &str, live: bool) -> Source {
    Source {
        url: url.to_string(),
        label: if live { "Directo".into() } else { "HLS".into() },
        height: None,
        live,
        audio_url: None,
        drop: Vec::new(),
        twitch_ads: false,
        available_at: None,
    }
}

/// El audio AAC de 128 kb/s (itag 140), o el de mayor bitrate si ese falta.
fn audio_url(formats: &[(&Format, String)]) -> Option<String> {
    formats
        .iter()
        .find(|(f, _)| f.itag == 140)
        .or_else(|| {
            formats
                .iter()
                .filter(|(f, _)| f.is_aac_audio())
                .max_by_key(|(f, _)| f.bitrate)
        })
        .map(|(_, url)| url.clone())
}

/// Segundos de espera antes de que YouTube sirva el video (anuncios previos), como yt-dlp: la
/// duración de cada anuncio de inicio (o lo que tarda en poderse saltar).
pub fn available_at(player: &Value, now: u64) -> u64 {
    let mut wait = 0.0_f64;
    for renderer in pre_roll_ads(player) {
        let mut duration = query_param(
            renderer["playerVars"].as_str().unwrap_or(""),
            "length_seconds",
        )
        .and_then(|s| s.parse::<f64>().ok());
        if let Some(skip_ms) = renderer["skipOffsetMilliseconds"].as_f64() {
            duration = Some(skip_ms / 1000.0);
        }
        if let Some(d) = duration {
            wait += d;
        }
    }
    if wait > 0.0 {
        now + wait.ceil() as u64
    } else {
        now
    }
}

/// Los `instreamVideoAdRenderer` que van antes del video.
fn pre_roll_ads(player: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    for placement in player["adPlacements"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let kind = placement["adPlacementRenderer"]["config"]["adPlacementConfig"]["kind"].as_str();
        if kind == Some("AD_PLACEMENT_KIND_START") {
            if let Some(ad) = find_key(
                &placement["adPlacementRenderer"]["renderer"],
                "instreamVideoAdRenderer",
                0,
            ) {
                out.push(ad);
            }
        }
    }
    for slot in player["adSlots"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let trigger = slot["adSlotRenderer"]["adSlotMetadata"]["triggerEvent"].as_str();
        if trigger == Some("SLOT_TRIGGER_EVENT_BEFORE_CONTENT") {
            let layout = &slot["adSlotRenderer"]["fulfillmentContent"]["fulfilledLayout"];
            if let Some(ad) = find_key(layout, "instreamVideoAdRenderer", 0) {
                out.push(ad);
            }
        }
    }
    out
}

/// Pares `clave=valor` de una consulta (o de un `signatureCipher`), con los valores decodificados.
pub fn query_pairs(text: &str) -> Vec<(String, String)> {
    let query = text.split_once('?').map_or(text, |(_, q)| q);
    query
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((percent_decode(k), percent_decode(v)))
        })
        .collect()
}

/// Valor de un parámetro de la consulta de una URL, decodificado.
pub fn query_param(url: &str, key: &str) -> Option<String> {
    query_pairs(url)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

/// Cambia (o agrega) un parámetro de la consulta de una URL. El resto de la URL no se toca.
pub fn set_param(url: &str, key: &str, value: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, q),
        None => (url, ""),
    };
    let mut parts: Vec<String> = query
        .split('&')
        .filter(|p| {
            !p.is_empty() && p.split('=').next().map(percent_decode).as_deref() != Some(key)
        })
        .map(str::to_string)
        .collect();
    parts.push(format!("{key}={}", pct_encode(value)));
    format!("{base}?{}", parts.join("&"))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CIPHER: &str = "s=ABCDEF&sp=sig&url=https%3A%2F%2Fr1.googlevideo.com%2Fvideoplayback%3Fexpire%3D1%26n%3DOLDN";

    fn fmt(v: Value) -> Format {
        Format::from_json(&v).unwrap()
    }

    #[test]
    fn cipher_is_parsed_and_decoded() {
        let c = parse_cipher(CIPHER).unwrap();
        assert_eq!(c.signature, "ABCDEF");
        assert_eq!(c.sp, "sig");
        assert_eq!(
            c.url,
            "https://r1.googlevideo.com/videoplayback?expire=1&n=OLDN"
        );
        assert_eq!(
            parse_cipher("s=X&url=https%3A%2F%2Fa").unwrap().sp,
            "signature"
        );
    }

    #[test]
    fn resolve_applies_signature_and_n() {
        let f = fmt(
            json!({ "itag": 135, "mimeType": "video/mp4; codecs=\"avc1.4d401f\"", "signatureCipher": CIPHER, "height": 480 }),
        );
        let mut solved = Solutions::default();
        solved.sig.insert("ABCDEF".into(), "SIGNED".into());
        solved.n.insert("OLDN".into(), "NEWN".into());
        let url = resolve_url(&f, &solved, true).unwrap();
        assert!(url.contains("sig=SIGNED"), "{url}");
        assert!(url.contains("n=NEWN"), "{url}");
        assert!(!url.contains("OLDN"), "{url}");
        // Sin el desafío resuelto, el formato no se puede usar.
        assert_eq!(resolve_url(&f, &Solutions::default(), true), None);
        // Sin JavaScript (visionos) el `n` no hace falta: la URL se usa como viene.
        let plain = fmt(
            json!({ "itag": 135, "mimeType": "video/mp4; codecs=\"avc1\"", "url": "https://v/x?n=ZZ", "height": 480 }),
        );
        assert_eq!(
            resolve_url(&plain, &Solutions::default(), false).as_deref(),
            Some("https://v/x?n=ZZ")
        );
    }

    #[test]
    fn challenges_are_collected_once() {
        let a = fmt(
            json!({ "itag": 135, "mimeType": "video/mp4; codecs=\"avc1\"", "signatureCipher": CIPHER }),
        );
        let b = fmt(
            json!({ "itag": 136, "mimeType": "video/mp4; codecs=\"avc1\"", "signatureCipher": CIPHER }),
        );
        let (sig, n) = challenges(&[&a, &b]);
        assert_eq!(sig, vec!["ABCDEF".to_string()]);
        assert_eq!(n, vec!["OLDN".to_string()]);
    }

    #[test]
    fn dash_h264_per_height_with_audio_140_and_no_vp9() {
        let formats = [
            fmt(
                json!({ "itag": 135, "mimeType": "video/mp4; codecs=\"avc1.4d401f\"", "url": "https://v/135", "height": 480, "bitrate": 500000 }),
            ),
            fmt(
                json!({ "itag": 134, "mimeType": "video/mp4; codecs=\"avc1.4d401e\"", "url": "https://v/134", "height": 360, "bitrate": 300000 }),
            ),
            fmt(
                json!({ "itag": 136, "mimeType": "video/mp4; codecs=\"avc1.4d401f\"", "url": "https://v/136", "height": 720, "bitrate": 900000 }),
            ),
            fmt(
                json!({ "itag": 247, "mimeType": "video/webm; codecs=\"vp9\"", "url": "https://v/247", "height": 720, "bitrate": 999999 }),
            ),
            fmt(
                json!({ "itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "url": "https://a/140", "bitrate": 128000 }),
            ),
            fmt(
                json!({ "itag": 251, "mimeType": "audio/webm; codecs=\"opus\"", "url": "https://a/251", "bitrate": 160000 }),
            ),
        ];
        let playable: Vec<(&Format, String)> = formats
            .iter()
            .map(|f| (f, f.url.clone().unwrap()))
            .collect();
        let sources = choose_sources(&playable, Some("https://hls"), false);
        let heights: Vec<u32> = sources.iter().filter_map(|s| s.height).collect();
        assert_eq!(heights, vec![720, 480, 360]);
        assert!(sources
            .iter()
            .all(|s| s.audio_url.as_deref() == Some("https://a/140")));
        assert_eq!(sources[0].url, "https://v/136");
    }

    #[test]
    fn hls_is_the_fallback_and_live_is_always_hls() {
        let none: Vec<(&Format, String)> = Vec::new();
        let s = choose_sources(&none, Some("https://hls/master.m3u8"), false);
        assert_eq!(s.len(), 1);
        assert!(!s[0].live);
        let live = choose_sources(&none, Some("https://hls/live.m3u8"), true);
        assert!(live[0].live);
        assert_eq!(choose_sources(&none, None, false), Vec::new());
    }

    #[test]
    fn itag_18_is_used_only_when_there_is_no_360p_dash() {
        let f18 = fmt(
            json!({ "itag": 18, "mimeType": "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"", "url": "https://v/18", "height": 360 }),
        );
        let list = vec![(&f18, "https://v/18".to_string())];
        let sources = choose_sources(&list, None, false);
        assert_eq!(sources[0].url, "https://v/18");
        assert_eq!(sources[0].audio_url, None);
    }

    #[test]
    fn pre_roll_ad_is_waited_for() {
        let player = json!({ "adPlacements": [ { "adPlacementRenderer": {
            "config": { "adPlacementConfig": { "kind": "AD_PLACEMENT_KIND_START" } },
            "renderer": { "instreamVideoAdRenderer": { "playerVars": "length_seconds=6&foo=1" } }
        }}]});
        assert_eq!(available_at(&player, 1000), 1006);
        assert_eq!(available_at(&json!({}), 1000), 1000);
    }

    #[test]
    fn skippable_ad_waits_for_the_skip_time() {
        let player = json!({ "adSlots": [ { "adSlotRenderer": {
            "adSlotMetadata": { "triggerEvent": "SLOT_TRIGGER_EVENT_BEFORE_CONTENT" },
            "fulfillmentContent": { "fulfilledLayout": { "playerBytesAdLayoutRenderer": { "renderingContent": {
                "instreamVideoAdRenderer": { "playerVars": "length_seconds=30", "skipOffsetMilliseconds": 5000.0 }
            }}}}
        }}]});
        assert_eq!(available_at(&player, 10), 15);
    }

    #[test]
    fn query_helpers_round_trip() {
        let url = set_param("https://v/x?a=1&n=old", "n", "a b");
        assert_eq!(query_param(&url, "n").as_deref(), Some("a b"));
        assert_eq!(query_param(&url, "a").as_deref(), Some("1"));
    }
}
