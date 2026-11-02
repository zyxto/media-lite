//! Letras sincronizadas (formato LRC) de lrclib.net: una línea con su tiempo. Es una API pública,
//! sin cuenta. Si la canción no está con su álbum, se busca por título y artista.

use media_lite_core::http::{self, Agent};
use media_lite_core::{Error, ErrorKind, Result};
use serde_json::Value;

use crate::util::pct_encode;

const API: &str = "https://lrclib.net/api";
const MAX_JSON: u64 = 1024 * 1024;

/// Una línea de la letra, con el segundo en que se canta.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub at: f64,
    pub text: String,
}

/// Lee un LRC: `[mm:ss.xx] texto`. Las etiquetas de metadatos (`[ar:…]`) se saltan, y una línea con
/// varios tiempos (`[00:10.00][00:50.00] coro`) da una línea por cada tiempo.
pub fn parse_lrc(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else {
                break;
            };
            match parse_time(&stripped[..end]) {
                Some(t) => times.push(t),
                None => {
                    times.clear();
                    break;
                }
            }
            rest = stripped[end + 1..].trim_start();
        }
        for at in times {
            out.push(Line {
                at,
                text: rest.trim().to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    out
}

/// `mm:ss.xx` o `mm:ss` a segundos.
fn parse_time(text: &str) -> Option<f64> {
    let (minutes, seconds) = text.split_once(':')?;
    let minutes: f64 = minutes.trim().parse().ok()?;
    let seconds: f64 = seconds.trim().parse().ok()?;
    Some(minutes * 60.0 + seconds)
}

/// Índice de la línea que suena en `pos` (la última que ya empezó). `None` antes de la primera.
pub fn line_at(lines: &[Line], pos: f64) -> Option<usize> {
    let after = lines.partition_point(|l| l.at <= pos);
    after.checked_sub(1)
}

/// Una letra con menos líneas que esto se considera un registro de prueba o incompleto.
const MIN_LINES: usize = 5;

/// La letra sincronizada de una canción: la coincidencia exacta (título, artista, álbum y duración)
/// y, si tiene pocas líneas, las de la búsqueda por título y artista. Gana la que tenga más líneas.
/// Sin letra útil, `Unavailable`.
pub fn fetch(
    agent: &Agent,
    title: &str,
    artist: &str,
    album: &str,
    duration: Option<u64>,
) -> Result<Vec<Line>> {
    let mut exact = format!(
        "{API}/get?track_name={}&artist_name={}",
        pct_encode(title),
        pct_encode(artist)
    );
    if !album.is_empty() {
        exact.push_str(&format!("&album_name={}", pct_encode(album)));
    }
    if let Some(secs) = duration {
        exact.push_str(&format!("&duration={secs}"));
    }
    let mut best = candidates(agent, &exact, false)?
        .into_iter()
        .next()
        .unwrap_or_default();
    if best.len() < MIN_LINES {
        let search = format!(
            "{API}/search?track_name={}&artist_name={}",
            pct_encode(title),
            pct_encode(artist)
        );
        for lines in candidates(agent, &search, true)? {
            if lines.len() > best.len() {
                best = lines;
            }
        }
    }
    if best.len() < MIN_LINES {
        return Err(Error::new(
            ErrorKind::Unavailable,
            "No hay letra sincronizada para esta canción.",
        ));
    }
    Ok(best)
}

/// Las letras sincronizadas de una respuesta de lrclib, una por resultado. `list` dice si la
/// respuesta es una lista (búsqueda) o un solo registro.
fn candidates(agent: &Agent, url: &str, list: bool) -> Result<Vec<Vec<Line>>> {
    let response = http::request(
        agent,
        "GET",
        url,
        &[("Accept", "application/json")],
        None,
        MAX_JSON,
    )?;
    if response.status == 404 {
        return Ok(Vec::new());
    }
    if !response.ok() {
        return Err(Error::network(format!(
            "lrclib respondió con error {}",
            response.status
        )));
    }
    let value: Value = serde_json::from_slice(&response.body)
        .map_err(|_| Error::parse("lrclib: la respuesta no se entiende"))?;
    let entries: Vec<&Value> = if list {
        value
            .as_array()
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    } else {
        vec![&value]
    };
    Ok(entries
        .iter()
        .filter_map(|e| e["syncedLyrics"].as_str())
        .map(parse_lrc)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LRC: &str = "[ar:Rick Astley]\n[00:18.50] We're no strangers to love\n[00:22.00][01:30.00] You know the rules\n\n[00:26.10] \n";

    #[test]
    fn lrc_lines_are_timed_and_sorted() {
        let lines = parse_lrc(LRC);
        // 18.5, 22 (la misma letra dos veces), 26.1 (vacía: una pausa) y 90.
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].text, "We're no strangers to love");
        assert!((lines[0].at - 18.5).abs() < 1e-9);
        assert_eq!(lines[1].text, "You know the rules");
        assert_eq!(lines[2].text, "");
        assert!((lines[3].at - 90.0).abs() < 1e-9);
    }

    #[test]
    fn current_line_is_the_last_one_started() {
        let lines = parse_lrc(LRC);
        assert_eq!(line_at(&lines, 10.0), None);
        assert_eq!(line_at(&lines, 18.5), Some(0));
        assert_eq!(line_at(&lines, 60.0), Some(2));
        assert_eq!(line_at(&lines, 95.0), Some(3));
    }
}
