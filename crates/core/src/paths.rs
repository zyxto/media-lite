//! Rutas XDG: configuración, caché y datos de media-lite.

use std::path::PathBuf;

fn base(var: &str, home_fallback: &str) -> PathBuf {
    let root = std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(home_fallback)
        });
    root.join("media-lite")
}

/// `$XDG_CONFIG_HOME/media-lite` (por defecto `~/.config/media-lite`).
pub fn config_dir() -> PathBuf {
    base("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_CACHE_HOME/media-lite` (por defecto `~/.cache/media-lite`).
pub fn cache_dir() -> PathBuf {
    base("XDG_CACHE_HOME", ".cache")
}

/// `$XDG_DATA_HOME/media-lite` (por defecto `~/.local/share/media-lite`).
pub fn data_dir() -> PathBuf {
    base("XDG_DATA_HOME", ".local/share")
}
