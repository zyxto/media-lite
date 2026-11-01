//! Caché por versión del reproductor (`player_id`): el `base.js` ya preprocesado y los desafíos
//! ya resueltos. YouTube cambia `base.js` cada algunos días; mientras tanto, cada reproducción
//! es rápida. Las versiones de más de 14 días se borran.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Lo que se borra después de este tiempo sin usarse.
pub const MAX_AGE: Duration = Duration::from_secs(14 * 24 * 3600);

/// Los `n` y `sig` resueltos de una versión. Claves: `n|<desafío>` y `sig|<desafío>`.
pub type Solved = BTreeMap<String, String>;

pub struct Cache {
    dir: PathBuf,
}

/// Un `player_id` de YouTube son 8 caracteres hexadecimales. Cualquier otra cosa se rechaza, así
/// que nunca se escribe fuera de la carpeta de caché.
pub fn valid_id(id: &str) -> bool {
    id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Cache {
    pub fn new(dir: impl Into<PathBuf>) -> Cache {
        Cache { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn preprocessed(&self, id: &str) -> Option<String> {
        if !valid_id(id) {
            return None;
        }
        fs::read_to_string(self.dir.join(format!("{id}.js"))).ok()
    }

    pub fn store_preprocessed(&self, id: &str, text: &str) -> io::Result<()> {
        if !valid_id(id) {
            return Err(io::Error::other("id de reproductor inválido"));
        }
        fs::create_dir_all(&self.dir)?;
        media_lite_core::store::write_atomic(
            &self.dir.join(format!("{id}.js")),
            text.as_bytes(),
            0o644,
        )
    }

    pub fn solved(&self, id: &str) -> Solved {
        if !valid_id(id) {
            return Solved::new();
        }
        fs::read(self.dir.join(format!("{id}.solved.json")))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn store_solved(&self, id: &str, solved: &Solved) -> io::Result<()> {
        if !valid_id(id) {
            return Err(io::Error::other("id de reproductor inválido"));
        }
        fs::create_dir_all(&self.dir)?;
        let body = serde_json::to_vec(solved).map_err(|e| io::Error::other(e.to_string()))?;
        media_lite_core::store::write_atomic(
            &self.dir.join(format!("{id}.solved.json")),
            &body,
            0o644,
        )
    }

    /// Borra los archivos de versiones que no se usan hace más de `max_age`. Devuelve cuántos.
    pub fn prune(&self, max_age: Duration) -> io::Result<usize> {
        let mut removed = 0;
        let now = SystemTime::now();
        if !self.dir.exists() {
            return Ok(0);
        }
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let modified = entry.metadata()?.modified()?;
            if now.duration_since(modified).unwrap_or_default() > max_age
                && fs::remove_file(entry.path()).is_ok()
            {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_hex_ids_are_accepted() {
        assert!(valid_id("11a45ffb"));
        assert!(!valid_id("../etc/passwd"));
        assert!(!valid_id("11a45ffbx"));
        assert!(!valid_id(""));
    }

    #[test]
    fn stores_and_reads_back_per_player() {
        let dir = std::env::temp_dir().join(format!("media-lite-ytcache-{}", std::process::id()));
        let cache = Cache::new(&dir);
        cache.store_preprocessed("11a45ffb", "pre").unwrap();
        assert_eq!(cache.preprocessed("11a45ffb").as_deref(), Some("pre"));
        assert!(cache.preprocessed("zzzzzzzz").is_none());
        let mut solved = Solved::new();
        solved.insert("n|abc".into(), "xyz".into());
        cache.store_solved("11a45ffb", &solved).unwrap();
        assert_eq!(cache.solved("11a45ffb"), solved);
        let _ = fs::remove_dir_all(&dir);
    }
}
