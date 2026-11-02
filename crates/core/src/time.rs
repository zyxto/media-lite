//! Fechas: lectura de ISO-8601 (en UTC) a segundos desde la época.

/// Días desde 1970-01-01 para una fecha civil (algoritmo de Howard Hinnant).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Lee `AAAA-MM-DDTHH:MM:SS…` (en UTC) y devuelve segundos desde la época. 0 si no se entiende.
pub fn parse_iso(s: &str) -> u64 {
    let digits: Vec<i64> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .take(6)
        .filter_map(|p| p.parse().ok())
        .collect();
    if digits.len() < 6 {
        return 0;
    }
    let days = days_from_civil(digits[0], digits[1], digits[2]);
    let secs = days * 86400 + digits[3] * 3600 + digits[4] * 60 + digits[5];
    secs.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_iso_dates_in_utc() {
        assert_eq!(parse_iso("2026-11-01T11:30:00.000Z"), 1793532600);
        assert_eq!(parse_iso("2026-11-01T11:30:00.000000Z"), 1793532600);
        assert_eq!(parse_iso("nada"), 0);
    }
}
