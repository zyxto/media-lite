//! Reescritura de playlists HLS (`.m3u8`). Solo cambia el texto de las listas: los segmentos de
//! video los baja mpv directo del CDN, así que el proxy nunca toca video.

use url::Url;

/// Reescribe una playlist.
///
/// - Las URLs relativas se hacen absolutas.
/// - Las listas hijas (variantes, pistas de audio y subtítulos) se mandan de nuevo por el proxy,
///   con `proxy` como generador de URL. Así también se filtran.
/// - Los segmentos cuya URL contiene alguno de `drop` (anuncios, por ejemplo) se quitan junto con su
///   `#EXTINF`, y se deja un `#EXT-X-DISCONTINUITY` en su lugar.
pub fn rewrite(text: &str, base: &Url, drop: &[String], proxy: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    // Líneas `#EXTINF` y demás etiquetas que preceden a un segmento; se guardan hasta ver la URI.
    let mut pending: Vec<&str> = Vec::new();
    let mut dropping = false;

    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            if rest.starts_with("EXTINF") || is_segment_tag(rest) {
                pending.push(trimmed);
                continue;
            }
            // Atributos con URI: se resuelven o se mandan por el proxy.
            if rest.starts_with("EXT-X-MEDIA") || rest.starts_with("EXT-X-I-FRAME-STREAM-INF") {
                flush(&mut out, &mut pending, &mut dropping);
                out.push_str(&rewrite_uri_attribute(trimmed, base, proxy));
                out.push('\n');
                continue;
            }
            if rest.starts_with("EXT-X-KEY")
                || rest.starts_with("EXT-X-MAP")
                || rest.starts_with("EXT-X-SESSION")
            {
                flush(&mut out, &mut pending, &mut dropping);
                out.push_str(&rewrite_uri_attribute(trimmed, base, &|absolute: &str| {
                    absolute.to_string()
                }));
                out.push('\n');
                continue;
            }
            flush(&mut out, &mut pending, &mut dropping);
            out.push_str(trimmed);
            out.push('\n');
            continue;
        }

        // Una URI: un segmento o una lista hija.
        let absolute = base
            .join(trimmed)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| trimmed.to_string());
        let is_child_playlist = absolute.split('?').next().unwrap_or("").ends_with(".m3u8");
        let is_dropped = drop
            .iter()
            .any(|pattern| absolute.contains(pattern.as_str()));
        if is_dropped && !is_child_playlist {
            pending.clear();
            if !dropping {
                out.push_str("#EXT-X-DISCONTINUITY\n");
                dropping = true;
            }
            continue;
        }
        dropping = false;
        flush(&mut out, &mut pending, &mut dropping);
        if is_child_playlist {
            out.push_str(&proxy(&absolute));
        } else {
            out.push_str(&absolute);
        }
        out.push('\n');
    }
    flush(&mut out, &mut pending, &mut dropping);
    out
}

/// Deja solo las variantes de una lista maestra que sirven para este equipo: altura hasta
/// `max_height` y códecs que el procesador decodifica (H.264; AV1, VP9 y HEVC no).
///
/// Si ninguna variante cumple, se queda la de menor altura: siempre hay algo que reproducir.
pub fn limit_variants(text: &str, max_height: Option<u32>) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut variants: Vec<(usize, u32, bool)> = Vec::new(); // (línea del #EXT, altura, pesada)
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("#EXT-X-STREAM-INF")
            && lines.get(i + 1).is_some_and(|l| !l.starts_with('#'))
        {
            let height = variant_height(lines[i]);
            let heavy = attr(lines[i], "CODECS").is_some_and(|c| {
                ["av01", "vp09", "vp9", "hvc1", "hev1"]
                    .iter()
                    .any(|bad| c.contains(bad))
            });
            variants.push((i, height, heavy));
            i += 2;
            continue;
        }
        i += 1;
    }
    if variants.is_empty() {
        return text.to_string();
    }

    let fits =
        |height: u32, heavy: bool| !heavy && max_height.is_none_or(|m| height == 0 || height <= m);
    let mut keep: Vec<bool> = variants
        .iter()
        .map(|(_, h, heavy)| fits(*h, *heavy))
        .collect();
    if !keep.iter().any(|k| *k) {
        if let Some(index) = variants
            .iter()
            .enumerate()
            .min_by_key(|(_, v)| v.1)
            .map(|(i, _)| i)
        {
            keep[index] = true;
        }
    }

    let mut dropped = std::collections::HashSet::new();
    for ((line, _, _), kept) in variants.iter().zip(&keep) {
        if !kept {
            dropped.insert(*line);
            dropped.insert(*line + 1);
        }
    }
    let mut out = String::with_capacity(text.len());
    for (index, line) in lines.iter().enumerate() {
        if !dropped.contains(&index) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Altura de una variante: la de `RESOLUTION`, o una estimación por `BANDWIDTH` si no la trae
/// (Pluto TV no la trae). Las estimaciones son las de lumiere (`guess_height` en stream.c).
fn variant_height(line: &str) -> u32 {
    if let Some(h) = attr(line, "RESOLUTION")
        .and_then(|r| r.split('x').nth(1))
        .and_then(|h| h.parse::<u32>().ok())
    {
        return h;
    }
    match attr(line, "BANDWIDTH").and_then(|b| b.parse::<u64>().ok()) {
        Some(bw) if bw < 800_000 => 240,
        Some(bw) if bw < 1_300_000 => 360,
        Some(bw) if bw < 2_000_000 => 480,
        Some(bw) if bw < 2_800_000 => 576,
        Some(_) => 720,
        None => 0,
    }
}

/// Valor de un atributo `CLAVE=valor` o `CLAVE="valor"` de una etiqueta HLS.
fn attr<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    let start = line
        .match_indices(&needle)
        .find(|(i, _)| *i == 0 || matches!(line.as_bytes()[*i - 1], b':' | b','))?
        .0
        + needle.len();
    let rest = &line[start..];
    if let Some(quoted) = rest.strip_prefix('"') {
        quoted.split('"').next()
    } else {
        rest.split(',').next()
    }
}

/// Etiquetas que van justo antes de un segmento y viajan con él.
fn is_segment_tag(rest: &str) -> bool {
    rest.starts_with("EXT-X-PROGRAM-DATE-TIME")
        || rest.starts_with("EXT-X-DATERANGE")
        || rest.starts_with("EXT-X-CUE")
        || rest.starts_with("EXT-OATCLS")
        || rest.starts_with("EXT-X-DISCONTINUITY")
        || rest.starts_with("EXT-X-BYTERANGE")
        || rest.starts_with("EXT-X-GAP")
}

/// Escribe las etiquetas pendientes, salvo que el segmento esté descartado.
fn flush(out: &mut String, pending: &mut Vec<&str>, dropping: &mut bool) {
    if !pending.is_empty() {
        if !*dropping {
            for tag in pending.iter() {
                out.push_str(tag);
                out.push('\n');
            }
        }
        pending.clear();
    }
}

/// Reemplaza `URI="..."` por la URL absoluta, o por el proxy si es una lista hija.
fn rewrite_uri_attribute(line: &str, base: &Url, proxy: &dyn Fn(&str) -> String) -> String {
    let Some(start) = line.find("URI=\"") else {
        return line.to_string();
    };
    let value_start = start + 5;
    let Some(len) = line[value_start..].find('"') else {
        return line.to_string();
    };
    let value = &line[value_start..value_start + len];
    let absolute = base
        .join(value)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| value.to_string());
    let replacement = if absolute.split('?').next().unwrap_or("").ends_with(".m3u8") {
        proxy(&absolute)
    } else {
        absolute
    };
    format!(
        "{}{}{}",
        &line[..value_start],
        replacement,
        &line[value_start + len..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://cdn.example.com/live/master.m3u8").unwrap()
    }

    fn fake_proxy(url: &str) -> String {
        format!("PROXY({url})")
    }

    #[test]
    fn limit_variants_keeps_only_what_fits_and_h264() {
        let master = "#EXTM3U\n\
#EXT-X-STREAM-INF:BANDWIDTH=8000000,RESOLUTION=1920x1080,CODECS=\"avc1.640028,mp4a.40.2\"\nv9/index.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=640x360,CODECS=\"avc1.42c01e,mp4a.40.2\"\nv3/index.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=1500000,RESOLUTION=640x360,CODECS=\"hvc1.1.6.L93\"\nhevc/index.m3u8\n";
        let out = limit_variants(master, Some(480));
        assert!(out.contains("v3/index.m3u8"));
        assert!(!out.contains("v9/index.m3u8"), "{out}");
        assert!(!out.contains("hevc/index.m3u8"), "{out}");
    }

    #[test]
    fn variants_without_resolution_use_bandwidth_estimate() {
        let master = "#EXT-X-STREAM-INF:BANDWIDTH=1042180\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2294080\nmid.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=4500000\nhigh.m3u8\n";
        let out = limit_variants(master, Some(576));
        assert!(
            out.contains("low.m3u8") && out.contains("mid.m3u8"),
            "{out}"
        );
        assert!(!out.contains("high.m3u8"), "{out}");
    }

    #[test]
    fn limit_variants_keeps_smallest_when_nothing_fits() {
        let master = "#EXT-X-STREAM-INF:BANDWIDTH=8000000,RESOLUTION=1920x1080\nbig.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=4000000,RESOLUTION=1280x720\nmid.m3u8\n";
        let out = limit_variants(master, Some(240));
        assert!(
            out.contains("mid.m3u8") && !out.contains("big.m3u8"),
            "{out}"
        );
    }

    #[test]
    fn media_playlists_are_left_alone() {
        let media = "#EXTM3U\n#EXTINF:4,\nseg.ts\n";
        assert_eq!(limit_variants(media, Some(360)), media);
    }

    #[test]
    fn relative_segments_become_absolute() {
        let text = "#EXTM3U\n#EXTINF:10.0,\nseg1.ts\n#EXTINF:10.0,\nseg2.ts\n#EXT-X-ENDLIST\n";
        let out = rewrite(text, &base(), &[], &fake_proxy);
        assert!(out.contains("https://cdn.example.com/live/seg1.ts"));
        assert!(out.contains("#EXTINF:10.0,\nhttps://cdn.example.com/live/seg2.ts"));
        assert!(out.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn variants_go_back_through_the_proxy() {
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nv5/index.m3u8\n";
        let out = rewrite(text, &base(), &[], &fake_proxy);
        assert!(
            out.contains("PROXY(https://cdn.example.com/live/v5/index.m3u8)"),
            "{out}"
        );
    }

    #[test]
    fn media_uri_attributes_are_rewritten() {
        let text = "#EXT-X-MEDIA:TYPE=AUDIO,URI=\"aud/index.m3u8\",GROUP-ID=\"a\"\n";
        let out = rewrite(text, &base(), &[], &fake_proxy);
        assert!(
            out.contains("URI=\"PROXY(https://cdn.example.com/live/aud/index.m3u8)\""),
            "{out}"
        );
    }

    #[test]
    fn keys_stay_direct() {
        let text = "#EXT-X-KEY:METHOD=AES-128,URI=\"keys/k.bin\"\n#EXTINF:4,\nseg.ts\n";
        let out = rewrite(text, &base(), &[], &fake_proxy);
        assert!(
            out.contains("URI=\"https://cdn.example.com/live/keys/k.bin\""),
            "{out}"
        );
    }

    #[test]
    fn ad_segments_are_dropped_with_their_extinf() {
        let text =
            "#EXTM3U\n#EXTINF:5.0,\nshow1.ts\n#EXTINF:3.0,\n/ads/spot.ts\n#EXTINF:5.0,\nshow2.ts\n";
        let drop = vec!["/ads/".to_string()];
        let out = rewrite(text, &base(), &drop, &fake_proxy);
        assert!(!out.contains("spot.ts"), "{out}");
        assert!(
            out.contains("show1.ts") && out.contains("show2.ts"),
            "{out}"
        );
        assert_eq!(out.matches("#EXT-X-DISCONTINUITY").count(), 1, "{out}");
        assert!(!out.contains("#EXTINF:3.0"), "{out}");
    }

    #[test]
    fn consecutive_ads_leave_one_discontinuity() {
        let text = "#EXTINF:3,\n/ads/a.ts\n#EXTINF:3,\n/ads/b.ts\n#EXTINF:5,\nshow.ts\n";
        let drop = vec!["/ads/".to_string()];
        let out = rewrite(text, &base(), &drop, &fake_proxy);
        assert_eq!(out.matches("#EXT-X-DISCONTINUITY").count(), 1, "{out}");
        assert!(out.contains("show.ts"));
    }
}
