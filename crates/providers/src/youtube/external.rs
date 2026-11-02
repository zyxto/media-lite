//! Último recurso: el yt-dlp que el usuario tenga instalado, pero solo si su versión (≥ 2025.11.12)
//! resuelve YouTube con un motor de JavaScript. Corre con un tiempo límite y con las cookies del
//! usuario si las hay. Nunca es el camino principal.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use media_lite_core::{Error, ErrorKind, Result, Source};
use serde_json::Value;

/// Primera versión de yt-dlp con soporte completo de YouTube (con un motor de JavaScript).
pub const MIN_VERSION: (u32, u32, u32) = (2025, 11, 12);
const TIMEOUT: Duration = Duration::from_secs(60);

/// `2026.10.21` → `(2026, 10, 21)`. `None` si no tiene esa forma.
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// ¿Sirve esta versión de yt-dlp?
pub fn version_ok(text: &str) -> bool {
    parse_version(text).is_some_and(|v| v >= MIN_VERSION)
}

/// Ejecuta `yt-dlp` con argumentos y un tiempo límite, y devuelve su salida.
fn run(args: &[String], limit: Duration) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    let args = args.to_vec();
    thread::Builder::new()
        .name("yt-dlp".into())
        .spawn(move || {
            let output = Command::new("yt-dlp")
                .args(&args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output();
            let _ = tx.send(
                output
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()),
            );
        })
        .ok()?;
    rx.recv_timeout(limit).ok().flatten()
}

/// Si hay un yt-dlp instalado y sirve.
pub fn available() -> bool {
    run(&["--version".to_string()], Duration::from_secs(10)).is_some_and(|v| version_ok(&v))
}

/// Las fuentes que elige yt-dlp para un video: un video H.264 con su audio AAC, o un MP4 con los dos.
pub fn sources(video_id: &str, cookies: Option<&Path>) -> Result<Vec<Source>> {
    let mut args = vec![
        "-J".to_string(),
        "--no-playlist".to_string(),
        "-f".to_string(),
        "bv[vcodec^=avc1]+ba[acodec^=mp4a]/18".to_string(),
    ];
    if let Some(path) = cookies.filter(|p| p.exists()) {
        args.push("--cookies".to_string());
        args.push(path.display().to_string());
    }
    args.push(format!("https://www.youtube.com/watch?v={video_id}"));
    let text = run(&args, TIMEOUT)
        .ok_or_else(|| Error::new(ErrorKind::Unavailable, "yt-dlp no pudo resolver el video"))?;
    parse_sources(&text)
}

/// Lee la salida de `yt-dlp -J`: `requested_formats` (video y audio aparte) o una URL única.
pub fn parse_sources(json: &str) -> Result<Vec<Source>> {
    let value: Value =
        serde_json::from_str(json).map_err(|_| Error::parse("yt-dlp: la salida no se entiende"))?;
    let formats = value["requested_formats"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let (video, audio) = match formats.as_slice() {
        [video, audio, ..] => (video, Some(audio)),
        [only] => (only, None),
        [] => (&value, None),
    };
    let url = video["url"]
        .as_str()
        .ok_or_else(|| Error::new(ErrorKind::Unavailable, "yt-dlp no entregó una URL"))?;
    let height = video["height"].as_u64().map(|h| h as u32);
    Ok(vec![Source {
        url: url.to_string(),
        label: height.map_or("yt-dlp".to_string(), |h| format!("{h}p")),
        height,
        live: value["is_live"].as_bool().unwrap_or(false),
        audio_url: audio.and_then(|a| a["url"].as_str()).map(str::to_string),
        drop: Vec::new(),
        twitch_ads: false,
        available_at: None,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_date() {
        assert_eq!(parse_version("2026.10.21\n"), Some((2026, 10, 21)));
        assert!(version_ok("2026.10.21"));
        assert!(version_ok("2025.11.12"));
        assert!(!version_ok("2025.11.11"));
        assert!(!version_ok("2024.04.09"));
        assert_eq!(parse_version("nope"), None);
    }

    #[test]
    fn requested_formats_give_video_and_audio() {
        let json = r#"{"requested_formats":[
            {"url":"https://v/1","height":720,"vcodec":"avc1.4d401f","acodec":"none"},
            {"url":"https://a/1","acodec":"mp4a.40.2","vcodec":"none"}]}"#;
        let sources = parse_sources(json).unwrap();
        assert_eq!(sources[0].url, "https://v/1");
        assert_eq!(sources[0].height, Some(720));
        assert_eq!(sources[0].audio_url.as_deref(), Some("https://a/1"));
    }

    #[test]
    fn a_single_url_is_used_as_it_is() {
        let json = r#"{"url":"https://m/18","height":360}"#;
        let sources = parse_sources(json).unwrap();
        assert_eq!(sources[0].url, "https://m/18");
        assert_eq!(sources[0].audio_url, None);
    }
}
