//! El reproductor de YouTube (`base.js`): su id, su URL, su `signatureTimestamp` y su preparación
//! para el solucionador. El id sale de `/iframe_api`; el `sts`, del propio `base.js`.

use std::path::Path;

use media_lite_core::http::{self, Agent};
use media_lite_core::{Error, ErrorKind, Result};
use media_lite_ytsolver::cache::Cache;
use media_lite_ytsolver::solve;

const IFRAME_API: &str = "https://www.youtube.com/iframe_api";
/// Un `base.js` pesa 2–3 MB.
const MAX_PLAYER: u64 = 12 * 1024 * 1024;

/// Id de 8 hex del reproductor en el texto de `/iframe_api` (`/s/player/4efb8b2c/…`, con las
/// barras escapadas en JS o no).
pub fn player_id_in(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(pos) = text[from..].find("player") {
        let mut i = from + pos + "player".len();
        from = i;
        if bytes.get(i) == Some(&b'\\') {
            i += 1;
        }
        if bytes.get(i) != Some(&b'/') {
            continue;
        }
        i += 1;
        let id = text.get(i..i + 8)?;
        if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        i += 8;
        if bytes.get(i) == Some(&b'\\') {
            i += 1;
        }
        if bytes.get(i) == Some(&b'/') {
            return Some(id.to_string());
        }
    }
    None
}

/// `signatureTimestamp` (5 dígitos) del `base.js`, como `(?:signatureTimestamp|sts)\s*:\s*([0-9]{5})`.
pub fn sts_in(text: &str) -> Option<String> {
    for key in ["signatureTimestamp", "sts"] {
        let mut from = 0;
        while let Some(pos) = text[from..].find(key) {
            let start = from + pos;
            from = start + key.len();
            // «sts» solo cuenta como palabra entera.
            if key == "sts" {
                let before = text[..start].chars().next_back();
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
            }
            let rest = text[from..].trim_start();
            let Some(rest) = rest.strip_prefix(':') else {
                continue;
            };
            let digits = rest.trim_start();
            if let Some(number) = digits.get(..5) {
                if number.bytes().all(|b| b.is_ascii_digit())
                    && !digits.as_bytes().get(5).is_some_and(u8::is_ascii_digit)
                {
                    return Some(number.to_string());
                }
            }
        }
    }
    None
}

pub fn player_url(id: &str) -> String {
    format!("https://www.youtube.com/s/player/{id}/player_ias.vflset/en_US/base.js")
}

/// Id del reproductor que YouTube usa ahora.
pub fn current_player_id(agent: &Agent) -> Result<String> {
    let response = http::get(agent, IFRAME_API, &[], 1024 * 1024)?;
    player_id_in(&response.text()).ok_or_else(|| {
        Error::new(
            ErrorKind::Parse,
            "No se encontró el reproductor de YouTube en /iframe_api",
        )
    })
}

/// Descarga el `base.js` de un id.
pub fn download(agent: &Agent, id: &str) -> Result<String> {
    let bytes = http::get_bytes(agent, &player_url(id), MAX_PLAYER)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Prepara un reproductor para el solucionador (una vez por id) y devuelve su `sts`.
pub fn prepare(exe: &Path, cache: &Cache, id: &str, js: &str) -> Result<Option<String>> {
    solve::preprocess(exe, cache, id, js)
        .map_err(|e| Error::new(ErrorKind::Other, format!("solucionador: {e}")))?;
    Ok(sts_in(js))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_player_id_in_plain_and_escaped_urls() {
        assert_eq!(
            player_id_in("var x=\"https://www.youtube.com/s/player/4efb8b2c/player_ias.vflset/\";")
                .as_deref(),
            Some("4efb8b2c")
        );
        assert_eq!(
            player_id_in(r#"src:"https:\/\/www.youtube.com\/s\/player\/11a45ffb\/player_ias.js""#)
                .as_deref(),
            Some("11a45ffb")
        );
        assert_eq!(player_id_in("player/xyz12345/ no es hex"), None);
    }

    #[test]
    fn reads_the_signature_timestamp_in_its_common_forms() {
        assert_eq!(
            sts_in("{signatureTimestamp:20123,foo:1}").as_deref(),
            Some("20123")
        );
        assert_eq!(sts_in("var a={sts : 20456}").as_deref(), Some("20456"));
        // Un número de otro largo no cuenta, ni una palabra que termina en «sts».
        assert_eq!(sts_in("requests:123456"), None);
        assert_eq!(sts_in("tests: 20123"), None);
    }

    #[test]
    fn url_uses_the_player_id() {
        assert_eq!(
            player_url("4efb8b2c"),
            "https://www.youtube.com/s/player/4efb8b2c/player_ias.vflset/en_US/base.js"
        );
    }
}
