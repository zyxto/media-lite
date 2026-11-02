//! Datos en disco: ajustes (TOML) y listas (JSON). Las escrituras son atómicas.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::paths;

/// Ajustes del usuario. Lo que falta en el archivo toma el valor por defecto.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme_dark: bool,
    /// Renderizador de Slint: `winit-software` (por defecto, el más seguro) o `winit-femtovg`.
    pub renderer: String,
    pub window_width: u32,
    pub window_height: u32,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    /// Calidad preferida en líneas (360, 480, 720…).
    pub quality: u32,
    /// Video liviano: menos cuadros por segundo para equipos lentos.
    pub low_video: bool,
    /// Salida de video de mpv: `xv` (liviana en GMA 4500) o `gpu`.
    pub vo: String,
    /// Decodificación por hardware de mpv: `no` (por defecto) o `auto-safe`.
    pub hwdec: String,
    /// Plex: mostrar también lo que está en inglés.
    pub plex_english: bool,
    /// Tope de la caché de miniaturas en memoria, en MB.
    pub thumb_mem_mb: u32,
    /// Tope de la caché de miniaturas en disco, en MB.
    pub thumb_disk_mb: u32,
    /// Plataformas que no se muestran (su clave: `pluto`, `twitch`…). Se aplica al reiniciar la app.
    #[serde(default)]
    pub hidden_platforms: Vec<String>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            theme_dark: true,
            renderer: "winit-software".to_string(),
            // Entra en una pantalla de 768 px de alto (notebooks), con la barra de título.
            window_width: 1024,
            window_height: 640,
            window_x: None,
            window_y: None,
            quality: 480,
            low_video: false,
            vo: "xv".to_string(),
            hwdec: "no".to_string(),
            plex_english: false,
            thumb_mem_mb: 24,
            thumb_disk_mb: 64,
            hidden_platforms: Vec::new(),
        }
    }
}

/// Carpeta de datos de media-lite.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    /// Store en `~/.config/media-lite` (o `$XDG_CONFIG_HOME`).
    pub fn default_dir() -> Store {
        Store::new(paths::config_dir())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn load_settings(&self) -> Settings {
        let path = self.dir.join("settings.toml");
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("settings.toml no se pudo leer ({e}); se usan los valores por defecto");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        let text = toml::to_string_pretty(settings).map_err(|e| io::Error::other(e.to_string()))?;
        write_atomic(&self.dir.join("settings.toml"), text.as_bytes(), 0o644)
    }

    /// Lee una lista JSON (`favorites.json`, `history.json`…). Si no existe o está rota, devuelve vacía.
    pub fn load_list<T: DeserializeOwned>(&self, name: &str) -> Vec<T> {
        let path = self.dir.join(name);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log::warn!("{name} no se pudo leer ({e}); se empieza vacío");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        }
    }

    pub fn save_list<T: Serialize>(&self, name: &str, items: &[T]) -> io::Result<()> {
        let bytes =
            serde_json::to_vec_pretty(items).map_err(|e| io::Error::other(e.to_string()))?;
        write_atomic(&self.dir.join(name), &bytes, 0o644)
    }
}

/// Inserta al principio de la lista (lo más nuevo primero) y recorta a `cap` elementos.
pub fn push_capped<T>(list: &mut Vec<T>, item: T, cap: usize) {
    list.insert(0, item);
    list.truncate(cap);
}

/// Escribe en un temporal y lo renombra: un corte de luz no deja el archivo a medias.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    set_mode(&tmp, mode)?;
    fs::rename(&tmp, path)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("media-lite-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn settings_roundtrip_and_defaults() {
        let dir = temp_dir("settings");
        let store = Store::new(&dir);
        assert_eq!(store.load_settings(), Settings::default());
        let s = Settings {
            quality: 360,
            window_x: Some(40),
            ..Settings::default()
        };
        store.save_settings(&s).unwrap();
        assert_eq!(store.load_settings(), s);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_settings_fall_back_to_defaults() {
        let dir = temp_dir("broken");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.toml"), "esto no es toml = [").unwrap();
        assert_eq!(Store::new(&dir).load_settings(), Settings::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn push_capped_keeps_newest_first() {
        let mut v = vec![1, 2, 3];
        push_capped(&mut v, 0, 3);
        assert_eq!(v, vec![0, 1, 2]);
    }
}
