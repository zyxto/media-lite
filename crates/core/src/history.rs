//! Historial y posiciones guardadas: sirve para retomar un video donde quedó.

use serde::{Deserialize, Serialize};

use crate::model::Platform;

/// Máximo de entradas. Lo más reciente va primero.
pub const LIMIT: usize = 500;
/// Debajo de estos segundos no vale la pena retomar.
const MIN_RESUME_SECS: f64 = 15.0;
/// Si quedan menos de estos segundos, se considera visto y empieza de cero.
const NEAR_END_SECS: f64 = 30.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub platform: Platform,
    pub id: String,
    pub title: String,
    /// Posición en segundos.
    pub position: f64,
    /// Duración en segundos; 0 si no se conoce (directos).
    #[serde(default)]
    pub duration: f64,
    /// Momento de la última reproducción, en segundos desde la época Unix.
    pub at: u64,
}

/// Guarda o actualiza la entrada de (plataforma, id) y la pone primera.
pub fn upsert(list: &mut Vec<Entry>, entry: Entry) {
    list.retain(|e| !(e.platform == entry.platform && e.id == entry.id));
    list.insert(0, entry);
    list.truncate(LIMIT);
}

/// Segundos desde donde retomar, si conviene.
pub fn resume_point(entry: &Entry) -> Option<f64> {
    if entry.position < MIN_RESUME_SECS {
        return None;
    }
    if entry.duration > 0.0 && entry.position > entry.duration - NEAR_END_SECS {
        return None;
    }
    Some(entry.position)
}

pub fn find<'a>(list: &'a [Entry], platform: Platform, id: &str) -> Option<&'a Entry> {
    list.iter().find(|e| e.platform == platform && e.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, position: f64, duration: f64) -> Entry {
        Entry {
            platform: Platform::Demo,
            id: id.into(),
            title: id.into(),
            position,
            duration,
            at: 0,
        }
    }

    #[test]
    fn upsert_moves_to_front_and_dedupes() {
        let mut list = vec![entry("a", 1.0, 100.0), entry("b", 2.0, 100.0)];
        upsert(&mut list, entry("a", 50.0, 100.0));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "a");
        assert_eq!(list[0].position, 50.0);
    }

    #[test]
    fn list_is_capped() {
        let mut list = Vec::new();
        for i in 0..(LIMIT + 10) {
            upsert(&mut list, entry(&i.to_string(), 0.0, 0.0));
        }
        assert_eq!(list.len(), LIMIT);
    }

    #[test]
    fn resume_rules() {
        assert_eq!(resume_point(&entry("x", 5.0, 600.0)), None);
        assert_eq!(resume_point(&entry("x", 300.0, 600.0)), Some(300.0));
        assert_eq!(resume_point(&entry("x", 590.0, 600.0)), None);
        // Directos (duración desconocida): siempre se retoma.
        assert_eq!(resume_point(&entry("x", 300.0, 0.0)), Some(300.0));
    }
}
