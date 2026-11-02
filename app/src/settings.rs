//! Pantalla de Ajustes: cada fila muestra un valor y un clic lo cambia al siguiente. Lo que
//! necesita reiniciar la app lo dice en la fila.

use std::rc::Rc;

use media_lite_core::store::Settings;
use slint::{ModelRc, SharedString, VecModel};

use crate::state::{set_status, App};
use crate::SettingRow;

/// Calidades que se pueden elegir (en líneas).
const QUALITIES: [u32; 4] = [360, 480, 720, 1080];
const VIDEO_OUTPUTS: [&str; 2] = ["xv", "gpu"];
const DECODERS: [&str; 2] = ["no", "auto-safe"];
const RENDERERS: [&str; 2] = ["winit-software", "winit-femtovg"];
const CACHE_MB: [u32; 3] = [12, 24, 48];

/// Filas de la pantalla, según los ajustes actuales.
pub fn rows(settings: &Settings) -> Vec<SettingRow> {
    let on_off = |b: bool| if b { "Sí" } else { "No" }.to_string();
    let renderer_note = "Al reiniciar la app";
    vec![
        row(
            "Calidad preferida",
            &format!("{}p", settings.quality),
            "Se aplica al próximo video",
        ),
        row(
            "Salida de video (mpv)",
            &settings.vo,
            "xv en equipos viejos; gpu si el driver anda bien",
        ),
        row(
            "Decodificación por hardware",
            &settings.hwdec,
            "no por defecto; auto-safe si VA-API anda",
        ),
        row(
            "Video liviano",
            &on_off(settings.low_video),
            "Salta cuadros para que el equipo responda",
        ),
        row(
            "Renderizador de la interfaz",
            &settings.renderer,
            renderer_note,
        ),
        row(
            "Plex: incluir inglés",
            &on_off(settings.plex_english),
            "Al reiniciar la app",
        ),
        row(
            "Caché de miniaturas",
            &format!("{} MB", settings.thumb_mem_mb),
            "Al reiniciar la app",
        ),
        row(
            "Tema",
            if settings.theme_dark {
                "Oscuro"
            } else {
                "Claro"
            },
            "También se cambia desde la barra lateral",
        ),
    ]
}

fn row(label: &str, value: &str, note: &str) -> SettingRow {
    SettingRow {
        label: SharedString::from(label),
        value: SharedString::from(value),
        note: SharedString::from(note),
    }
}

/// Abre la pantalla de Ajustes.
pub fn open(app: &Rc<App>) {
    let rows = rows(&app.settings.borrow());
    if let Some(w) = app.weak.upgrade() {
        w.set_settings_rows(ModelRc::new(VecModel::from(rows)));
        // El detalle queda abierto debajo: al cerrar Ajustes se vuelve a él.
        w.set_settings_open(true);
    }
}

/// Cierra la pantalla de Ajustes.
pub fn close(app: &Rc<App>) {
    if let Some(w) = app.weak.upgrade() {
        w.set_settings_open(false);
    }
}

/// Un clic en la fila `index`: pasa al siguiente valor y lo guarda.
pub fn click(app: &Rc<App>, index: usize) {
    let restart = {
        let mut s = app.settings.borrow_mut();
        match index {
            0 => s.quality = next(&QUALITIES, s.quality),
            1 => {
                s.vo = next(&VIDEO_OUTPUTS, s.vo.as_str()).to_string();
                stop_player(app);
            }
            2 => {
                s.hwdec = next(&DECODERS, s.hwdec.as_str()).to_string();
                stop_player(app);
            }
            3 => s.low_video = !s.low_video,
            4 => s.renderer = next(&RENDERERS, s.renderer.as_str()).to_string(),
            5 => s.plex_english = !s.plex_english,
            6 => s.thumb_mem_mb = next(&CACHE_MB, s.thumb_mem_mb),
            7 => {
                s.theme_dark = !s.theme_dark;
                if let Some(w) = app.weak.upgrade() {
                    use slint::ComponentHandle;
                    w.global::<crate::Theme>().set_dark(s.theme_dark);
                }
            }
            _ => return,
        }
        index == 4 || index == 5 || index == 6
    };
    if let Err(e) = app.store.save_settings(&app.settings.borrow()) {
        log::warn!("no se pudieron guardar los ajustes: {e}");
    }
    if let Some(w) = app.weak.upgrade() {
        w.set_settings_rows(ModelRc::new(VecModel::from(rows(&app.settings.borrow()))));
    }
    let message = if restart {
        "Guardado. Este cambio se aplica al reiniciar la app."
    } else {
        "Guardado."
    };
    set_status(app, message);
}

/// Cierra mpv para que el próximo video use las opciones nuevas. No corta la lista de la app.
fn stop_player(app: &Rc<App>) {
    if app.playback.borrow().is_none() {
        *app.player.borrow_mut() = None;
    }
}

/// El valor que sigue a `current` en `options`, dando la vuelta al final. Si no está, el primero.
fn next<T: PartialEq + Copy>(options: &[T], current: T) -> T {
    let i = options
        .iter()
        .position(|o| *o == current)
        .map(|i| (i + 1) % options.len())
        .unwrap_or(0);
    options[i]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_cycle_and_wrap() {
        assert_eq!(next(&QUALITIES, 480), 720);
        assert_eq!(next(&QUALITIES, 1080), 360);
        assert_eq!(next(&VIDEO_OUTPUTS, "xv"), "gpu");
        assert_eq!(next(&VIDEO_OUTPUTS, "desconocido"), "xv");
    }

    #[test]
    fn rows_show_current_values() {
        let s = Settings::default();
        let r = rows(&s);
        assert_eq!(r.len(), 8);
        assert_eq!(r[0].value.as_str(), "480p");
        assert_eq!(r[1].value.as_str(), "xv");
        assert_eq!(r[3].value.as_str(), "No");
    }
}
