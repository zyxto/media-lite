//! Pantalla de Ajustes: cada fila muestra un valor y un clic lo cambia al siguiente. Lo que
//! necesita reiniciar la app lo dice en la fila.

use std::rc::Rc;

use media_lite_core::store::Settings;
use media_lite_core::{paths, Platform};
use media_lite_providers::youtube::cookies;
use slint::{ModelRc, SharedString, VecModel};

use crate::state::{set_status, App};
use crate::SettingRow;

/// Calidades que se pueden elegir (en líneas).
const QUALITIES: [u32; 4] = [360, 480, 720, 1080];
const VIDEO_OUTPUTS: [&str; 2] = ["xv", "gpu"];
const DECODERS: [&str; 2] = ["no", "auto-safe"];
const RENDERERS: [&str; 2] = ["winit-software", "winit-femtovg"];
const CACHE_MB: [u32; 3] = [12, 24, 48];

/// Fila del perfil de equipo lento (un clic baja calidad, video y caché de miniaturas).
const ROW_SLOW: usize = 8;
/// Fila del soporte de YouTube (busca una versión nueva del solucionador).
const ROW_YOUTUBE: usize = 9;
/// Fila de las cookies de YouTube (solo informa dónde se ponen).
const ROW_COOKIES: usize = 10;
/// Primera fila de plataformas: una por cada una que se muestra, en el orden de `Platform::ALL`.
const ROW_PLATFORMS: usize = 11;

/// Fila de la entrada en el menú (va después de las de plataformas).
fn row_menu() -> usize {
    ROW_PLATFORMS + switchable().len()
}

/// Valores del perfil de equipo lento: 360p, video liviano y caché de miniaturas chica.
const SLOW_QUALITY: u32 = 360;
const SLOW_THUMB_MB: u32 = 12;
const NORMAL_QUALITY: u32 = 480;
const NORMAL_THUMB_MB: u32 = 24;

/// Las plataformas que tienen su fila (todas menos la de prueba).
fn switchable() -> Vec<Platform> {
    Platform::ALL
        .into_iter()
        .filter(|p| *p != Platform::Demo)
        .collect()
}

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
        row(
            "Entrada en el menú",
            if crate::menu::installed() {
                "Sí"
            } else {
                "No"
            },
            "Copia el acceso directo a ~/.local/share/applications",
        ),
        row(
            "Perfil equipo lento",
            &on_off(settings.quality == SLOW_QUALITY && settings.low_video),
            "360p, video liviano y poca caché de miniaturas",
        ),
        row(
            "Soporte de YouTube",
            &youtube_support_version(),
            "Toca para buscar la última versión del solucionador",
        ),
        row(
            "Cookies de YouTube",
            if cookies::path(&paths::config_dir()).exists() {
                "Cargadas"
            } else {
                "No hay"
            },
            "Archivo cookies/youtube.txt en ~/.config/media-lite (permisos 0600)",
        ),
    ]
    .into_iter()
    .chain(switchable().into_iter().map(|p| {
        let hidden = settings.hidden_platforms.iter().any(|k| k == p.key());
        row(
            &format!("Mostrar {}", p.label()),
            &on_off(!hidden),
            "Al reiniciar la app",
        )
    }))
    .collect()
}

/// Versión del soporte de YouTube que se usa: la instalada, o la que trae la app.
fn youtube_support_version() -> String {
    match media_lite_ytsolver::update::load_installed(&paths::data_dir().join("ejs")) {
        Some((version, _)) if !version.is_empty() => version,
        _ => format!("{} (incluido)", media_lite_ytsolver::EJS_VERSION),
    }
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
    if index == ROW_SLOW {
        return toggle_slow_profile(app);
    }
    if index == row_menu() {
        return toggle_menu(app);
    }
    if index == ROW_YOUTUBE {
        return update_youtube(app);
    }
    if index == ROW_COOKIES {
        set_status(
            app,
            "Pegá tus cookies de YouTube en ~/.config/media-lite/cookies/youtube.txt (formato Netscape o «Cookie:»).",
        );
        return;
    }
    if index >= ROW_PLATFORMS {
        return toggle_platform(app, index - ROW_PLATFORMS);
    }
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

/// Busca la última versión del soporte de YouTube (el solucionador) y la instala. Corre en el pool:
/// la app sigue usable. El cambio vale para las próximas resoluciones.
fn update_youtube(app: &Rc<App>) {
    set_status(app, "Buscando la última versión del soporte de YouTube…");
    let dir = paths::data_dir().join("ejs");
    app.pool.run(move || {
        let agent = media_lite_core::http::agent();
        let result = media_lite_ytsolver::update::fetch_latest(&agent).and_then(|update| {
            let version = update.version.clone();
            media_lite_ytsolver::update::install(&dir, &update)
                .map(|_| version)
                .map_err(|e| media_lite_core::Error::network(e.to_string()))
        });
        let _ = slint::invoke_from_event_loop(move || {
            crate::state::with_app(|app| {
                let message = match result {
                    Ok(version) => format!("Soporte de YouTube actualizado a {version}."),
                    Err(e) => format!("No se pudo actualizar el soporte de YouTube: {e}"),
                };
                set_status(app, message);
                if let Some(w) = app.weak.upgrade() {
                    w.set_settings_rows(ModelRc::new(VecModel::from(rows(&app.settings.borrow()))));
                }
            });
        });
    });
}

/// Muestra u oculta una plataforma (se aplica al reiniciar la app).
fn toggle_platform(app: &Rc<App>, i: usize) {
    let Some(platform) = switchable().get(i).copied() else {
        return;
    };
    {
        let mut s = app.settings.borrow_mut();
        let key = platform.key().to_string();
        match s.hidden_platforms.iter().position(|k| *k == key) {
            Some(pos) => {
                s.hidden_platforms.remove(pos);
            }
            None => s.hidden_platforms.push(key),
        }
    }
    if let Err(e) = app.store.save_settings(&app.settings.borrow()) {
        log::warn!("no se pudieron guardar los ajustes: {e}");
    }
    if let Some(w) = app.weak.upgrade() {
        w.set_settings_rows(ModelRc::new(VecModel::from(rows(&app.settings.borrow()))));
    }
    set_status(
        app,
        format!("{} cambia al reiniciar la app.", platform.label()),
    );
}

/// Prende o apaga el perfil de equipo lento. Se aplica al próximo video (la calidad) y a la próxima
/// vez que se abra la app (la caché de miniaturas).
fn toggle_slow_profile(app: &Rc<App>) {
    {
        let mut s = app.settings.borrow_mut();
        let slow = !(s.quality == SLOW_QUALITY && s.low_video);
        if slow {
            s.quality = SLOW_QUALITY;
            s.low_video = true;
            s.thumb_mem_mb = SLOW_THUMB_MB;
        } else {
            s.quality = NORMAL_QUALITY;
            s.low_video = false;
            s.thumb_mem_mb = NORMAL_THUMB_MB;
        }
    }
    if let Err(e) = app.store.save_settings(&app.settings.borrow()) {
        log::warn!("no se pudieron guardar los ajustes: {e}");
    }
    if let Some(w) = app.weak.upgrade() {
        w.set_settings_rows(ModelRc::new(VecModel::from(rows(&app.settings.borrow()))));
    }
    set_status(app, "Perfil de equipo lento: se aplica al próximo video (la caché de miniaturas, al reiniciar).");
}

/// Agrega o quita la entrada en el menú de la distro.
fn toggle_menu(app: &Rc<App>) {
    let message = match crate::menu::toggle() {
        Ok(true) => "Agregado al menú de aplicaciones.",
        Ok(false) => "Quitado del menú de aplicaciones.",
        Err(e) => {
            log::warn!("no se pudo cambiar el menú: {e}");
            "No se pudo cambiar el menú de aplicaciones."
        }
    };
    if let Some(w) = app.weak.upgrade() {
        w.set_settings_rows(ModelRc::new(VecModel::from(rows(&app.settings.borrow()))));
    }
    set_status(app, message);
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
        // Las filas fijas, luego las de plataformas (todas menos la de prueba) y la del menú.
        assert_eq!(r.len(), row_menu() + 1);
        assert_eq!(r[0].value.as_str(), "480p");
        assert_eq!(r[1].value.as_str(), "xv");
        assert_eq!(r[3].value.as_str(), "No");
    }
}
