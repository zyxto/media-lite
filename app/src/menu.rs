//! Entrada en el menú de la distro: copia el acceso directo a `~/.local/share/applications`, con el
//! ejecutable correcto (el AppImage, si se corre desde uno) y un ícono que siempre existe.

use std::fs;
use std::path::PathBuf;

use media_lite_core::paths;

/// El acceso directo del empaquetado (su `Exec` se reemplaza al instalarlo).
const DESKTOP: &str = include_str!("../../packaging/media-lite.desktop");
/// El ícono del empaquetado, guardado junto a la app para que el menú siempre lo encuentre.
const ICON: &[u8] = include_bytes!("../../packaging/media-lite.png");

fn entry_path() -> PathBuf {
    paths::data_dir()
        .parent()
        .map(|share| share.join("applications"))
        .unwrap_or_else(|| PathBuf::from("applications"))
        .join("media-lite.desktop")
}

fn icon_path() -> PathBuf {
    paths::data_dir().join("media-lite.png")
}

/// ¿Está en el menú?
pub fn installed() -> bool {
    entry_path().exists()
}

/// Ejecutable que abre la app: el AppImage si se corre desde uno, o este binario.
fn executable() -> String {
    std::env::var("APPIMAGE")
        .ok()
        .filter(|p| !p.is_empty())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| "media-lite".to_string())
}

/// El texto del acceso directo con el ejecutable y el ícono de esta instalación.
pub fn desktop_text(exec: &str, icon: &str) -> String {
    DESKTOP
        .lines()
        .map(|line| {
            if line.starts_with("Exec=") {
                format!("Exec={} %U", exec.replace(' ', "\\ "))
            } else if line.starts_with("Icon=") {
                format!("Icon={icon}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Agrega la entrada al menú (o la quita, si ya estaba). Devuelve si ahora está.
pub fn toggle() -> std::io::Result<bool> {
    if installed() {
        fs::remove_file(entry_path())?;
        let _ = fs::remove_file(icon_path());
        return Ok(false);
    }
    fs::create_dir_all(paths::data_dir())?;
    fs::write(icon_path(), ICON)?;
    let text = desktop_text(&executable(), &icon_path().display().to_string());
    if let Some(parent) = entry_path().parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(entry_path(), text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_text_points_to_this_install() {
        let text = desktop_text(
            "/home/u/Apps/media-lite-x86_64.AppImage",
            "/home/u/.local/share/media-lite/media-lite.png",
        );
        assert!(text.contains("Exec=/home/u/Apps/media-lite-x86_64.AppImage %U"));
        assert!(text.contains("Icon=/home/u/.local/share/media-lite/media-lite.png"));
        assert!(text.contains("Name=media-lite"));
        assert!(!text.contains("Exec=media-lite %U"));
    }

    #[test]
    fn paths_with_spaces_are_escaped_in_exec() {
        let text = desktop_text("/opt/my apps/media-lite.AppImage", "/i.png");
        assert!(text.contains("Exec=/opt/my\\ apps/media-lite.AppImage %U"));
    }
}
