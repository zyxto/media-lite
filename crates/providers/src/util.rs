//! Utilidades comunes de los proveedores: fechas, codificación de URL, JSON y búsqueda sin acentos.

use std::fs::File;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Segundos desde la época Unix.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Codifica un valor para un query string (RFC 3986: solo deja sin codificar `A-Za-z0-9-._~`).
pub fn pct_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// UUID v4 sacado de `/dev/urandom`.
pub fn uuid4() -> String {
    let mut r = [0u8; 16];
    if File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut r))
        .is_err()
    {
        // Sin /dev/urandom (no debería pasar en Linux): se mezcla la hora para no repetir siempre.
        let t = now().to_le_bytes();
        for (i, b) in r.iter_mut().enumerate() {
            *b = t[i % 8] ^ (i as u8).wrapping_mul(37);
        }
    }
    r[6] = (r[6] & 0x0f) | 0x40;
    r[8] = (r[8] & 0x3f) | 0x80;
    let h: Vec<String> = r.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        h[0..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..16].concat()
    )
}

pub use media_lite_core::time::parse_iso;

/// Hora en UTC, redondeada hacia abajo, con el formato que espera la guía de Pluto TV.
pub fn iso_hour(epoch: u64) -> String {
    let e = epoch - epoch % 3600;
    let days = (e / 86400) as i64;
    let secs = e % 86400;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:00:00.000Z", secs / 3600)
}

/// Fecha civil a partir de días desde 1970-01-01 (algoritmo de Howard Hinnant).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Texto de un campo JSON, o vacío.
pub fn jstr(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Número de un campo JSON, o 0.
pub fn jnum(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// Booleano de un campo JSON, o falso.
pub fn jbool(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Arreglo de un campo JSON, o vacío.
pub fn jarr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Búsqueda sin acentos ni mayúsculas. Solo cubre las letras del español.
pub fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'Á' => 'a',
            'é' | 'è' | 'ë' | 'ê' | 'É' => 'e',
            'í' | 'ì' | 'ï' | 'î' | 'Í' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'Ó' => 'o',
            'ú' | 'ù' | 'ü' | 'û' | 'Ú' | 'Ü' => 'u',
            'ñ' | 'Ñ' => 'n',
            'ç' => 'c',
            other => other.to_lowercase().next().unwrap_or(other),
        })
        .collect()
}

/// Verdadero si cada palabra de `query` aparece en `title` (sin importar acentos ni mayúsculas).
pub fn matches(title: &str, query: &str) -> bool {
    let t = fold(title);
    let q = fold(query);
    let words: Vec<&str> = q.split_whitespace().collect();
    !words.is_empty() && words.iter().all(|w| t.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_lite_core::time::days_from_civil;

    #[test]
    fn iso_round_trip() {
        assert_eq!(parse_iso("2026-11-01T11:30:00.000Z"), 1793532600);
        assert_eq!(iso_hour(1793532600), "2026-11-01T11:00:00.000Z");
        assert_eq!(parse_iso("nada"), 0);
    }

    #[test]
    fn civil_days_are_inverse() {
        let (y, m, d) = civil_from_days(days_from_civil(2024, 2, 29));
        assert_eq!((y, m, d), (2024, 2, 29));
    }

    #[test]
    fn encodes_query_values() {
        assert_eq!(pct_encode("a b/c"), "a%20b%2Fc");
        assert_eq!(pct_encode("ñ"), "%C3%B1");
    }

    #[test]
    fn uuid_has_v4_shape() {
        let id = uuid4();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
    }

    #[test]
    fn search_ignores_accents_and_case() {
        assert!(matches("Acción en El Oeste", "accion oeste"));
        assert!(!matches("Comedia", "drama"));
        assert!(!matches("Algo", "   "));
    }
}
