//! Cookies de la cuenta de YouTube, opcionales. Se guardan en `cookies/youtube.txt` (permisos
//! 0600, fuera del registro): formato Netscape o una línea `Cookie:`. Con ellas, `/player` lleva la
//! cabecera `Cookie` y `Authorization: SAPISIDHASH`, que YouTube acepta como sesión.

use std::fs;
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

/// Dónde está el archivo de cookies dentro de la configuración.
pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join("cookies").join("youtube.txt")
}

/// Una sesión de YouTube lista para los pedidos.
#[derive(Clone, Debug, PartialEq)]
pub struct Auth {
    /// Valor de la cabecera `Cookie`.
    pub cookie_header: String,
    /// Valor de `SAPISID` (o `__Secure-3PAPISID`), para firmar.
    pub sapisid: String,
}

impl Auth {
    /// Lee el archivo de cookies. Sin SAPISID no hay sesión, y devuelve `None`.
    pub fn load(config_dir: &Path) -> Option<Auth> {
        let text = fs::read_to_string(path(config_dir)).ok()?;
        parse(&text)
    }
}

/// Lee cookies en formato Netscape o como encabezado `Cookie:`. Solo se usan las de YouTube y Google.
pub fn parse(text: &str) -> Option<Auth> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(header) = line.strip_prefix("Cookie:") {
            for part in header.split(';') {
                if let Some((name, value)) = part.trim().split_once('=') {
                    pairs.push((name.trim().to_string(), value.trim().to_string()));
                }
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        // Netscape: dominio, bandera, ruta, seguro, expiración, nombre, valor (separados por tabulación).
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() == 7
            && (fields[0].contains("youtube.com") || fields[0].contains("google.com"))
        {
            pairs.push((fields[5].to_string(), fields[6].to_string()));
        }
    }
    let sapisid = pairs
        .iter()
        .find(|(n, _)| n == "SAPISID")
        .or_else(|| pairs.iter().find(|(n, _)| n == "__Secure-3PAPISID"))?
        .1
        .clone();
    let cookie_header = pairs
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    Some(Auth {
        cookie_header,
        sapisid,
    })
}

/// `SAPISIDHASH <tiempo>_<sha1(tiempo + " " + SAPISID + " " + origen)>`.
pub fn sapisid_hash(sapisid: &str, origin: &str, now: u64) -> String {
    let digest = Sha1::digest(format!("{now} {sapisid} {origin}").as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("SAPISIDHASH {now}_{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_matches_the_documented_formula() {
        // Vector calculado con hashlib (python): sha1("1700000000 ABC123 https://www.youtube.com").
        assert_eq!(
            sapisid_hash("ABC123", "https://www.youtube.com", 1700000000),
            "SAPISIDHASH 1700000000_127b58fad191974bd15010621820454a8b3e6ebd"
        );
    }

    #[test]
    fn reads_netscape_and_header_cookies_and_keeps_only_google_domains() {
        let netscape = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tABC123\n.example.com\tTRUE\t/\tFALSE\t0\tOTHER\tx\n";
        let auth = parse(netscape).unwrap();
        assert_eq!(auth.sapisid, "ABC123");
        assert_eq!(auth.cookie_header, "SAPISID=ABC123");
        let header = parse("Cookie: SAPISID=XYZ; LOGIN_INFO=abc").unwrap();
        assert_eq!(header.sapisid, "XYZ");
        assert_eq!(header.cookie_header, "SAPISID=XYZ; LOGIN_INFO=abc");
        assert_eq!(parse("Cookie: LOGIN_INFO=abc"), None);
    }
}
