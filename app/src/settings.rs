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

/// Qué hace cada fila. La pantalla despacha por esto y no por la posición: reordenar las filas no
/// cambia lo que hace cada clic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Id {
    Quality,
    VideoOutput,
    Decoder,
    LowVideo,
    SlowProfile,
    Theme,
    Renderer,
    ThumbCache,
    Menu,
    YoutubeSupport,
    YoutubeCookies,
    PlexEnglish,
    Platform(usize),
}

/// Cómo se dibuja el valor de una fila.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    /// Un valor que cambia al siguiente con cada clic.
    Value,
    /// Un interruptor de sí / no.
    Switch(bool),
    /// Una acción: el valor es lo que hace el botón.
    Action,
}

/// Todas las filas, en el orden de la pantalla, con el título del grupo en la primera de cada uno.
fn layout(settings: &Settings) -> Vec<(Id, SettingRow)> {
    let mut out = Vec::new();
    let mut add = |id: Id, group: &str, label: &str, value: &str, note: &str, control: Control| {
        out.push((id, row(group, label, value, note, control)));
    };
    let on_off = |b: bool| if b { "Sí" } else { "No" };
    let slow = settings.quality == SLOW_QUALITY && settings.low_video;

    add(
        Id::Quality,
        "Reproducción",
        "Calidad preferida",
        &format!("{}p", settings.quality),
        "Se aplica al próximo video",
        Control::Value,
    );
    add(
        Id::VideoOutput,
        "",
        "Salida de video (mpv)",
        &settings.vo,
        "xv en equipos viejos; gpu si el driver anda bien",
        Control::Value,
    );
    add(
        Id::Decoder,
        "",
        "Decodificación por hardware",
        &settings.hwdec,
        "no por defecto; auto-safe si VA-API anda",
        Control::Value,
    );
    add(
        Id::LowVideo,
        "",
        "Video liviano",
        on_off(settings.low_video),
        "Salta cuadros para que el equipo responda",
        Control::Switch(settings.low_video),
    );
    add(
        Id::SlowProfile,
        "",
        "Perfil equipo lento",
        on_off(slow),
        "360p, video liviano y poca caché de miniaturas",
        Control::Switch(slow),
    );
    add(
        Id::Theme,
        "Interfaz",
        "Tema",
        if settings.theme_dark {
            "Oscuro"
        } else {
            "Claro"
        },
        "También se cambia desde la barra lateral",
        Control::Value,
    );
    add(
        Id::Renderer,
        "",
        "Renderizador de la interfaz",
        &settings.renderer,
        "Al reiniciar la app",
        Control::Value,
    );
    add(
        Id::ThumbCache,
        "",
        "Caché de miniaturas",
        &format!("{} MB", settings.thumb_mem_mb),
        "Al reiniciar la app",
        Control::Value,
    );
    let in_menu = crate::menu::installed();
    add(
        Id::Menu,
        "",
        "Entrada en el menú",
        on_off(in_menu),
        "Copia el acceso directo a ~/.local/share/applications",
        Control::Switch(in_menu),
    );
    add(
        Id::YoutubeSupport,
        "YouTube",
        "Soporte de YouTube",
        &youtube_support_version(),
        "Tocá para buscar la última versión del solucionador",
        Control::Action,
    );
    add(
        Id::YoutubeCookies,
        "",
        "Cookies de YouTube",
        if cookies::path(&paths::config_dir()).exists() {
            "Cargadas"
        } else {
            "No hay"
        },
        "Archivo cookies/youtube.txt en ~/.config/media-lite (permisos 0600)",
        Control::Action,
    );
    add(
        Id::PlexEnglish,
        "Plataformas",
        "Plex: incluir inglés",
        on_off(settings.plex_english),
        "Al reiniciar la app",
        Control::Switch(settings.plex_english),
    );
    for (i, p) in switchable().into_iter().enumerate() {
        let shown = !settings.hidden_platforms.iter().any(|k| k == p.key());
        add(
            Id::Platform(i),
            "",
            &format!("Mostrar {}", p.label()),
            on_off(shown),
            "Al reiniciar la app",
            Control::Switch(shown),
        );
    }
    out
}

/// Filas de la pantalla, según los ajustes actuales.
pub fn rows(settings: &Settings) -> Vec<SettingRow> {
    layout(settings).into_iter().map(|(_, row)| row).collect()
}

/// Versión del soporte de YouTube que se usa: la instalada, o la que trae la app.
fn youtube_support_version() -> String {
    match media_lite_ytsolver::update::load_installed(&paths::data_dir().join("ejs")) {
        Some((version, _)) if !version.is_empty() => version,
        _ => format!("{} (incluido)", media_lite_ytsolver::EJS_VERSION),
    }
}

fn row(group: &str, label: &str, value: &str, note: &str, control: Control) -> SettingRow {
    SettingRow {
        group: SharedString::from(group),
        label: SharedString::from(label),
        value: SharedString::from(value),
        note: SharedString::from(note),
        switch: matches!(control, Control::Switch(_)),
        on: control == Control::Switch(true),
        action: control == Control::Action,
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
    let id = {
        let settings = app.settings.borrow();
        layout(&settings).get(index).map(|(id, _)| *id)
    };
    let Some(id) = id else {
        return;
    };
    let restart = match id {
        Id::SlowProfile => return toggle_slow_profile(app),
        Id::Menu => return toggle_menu(app),
        Id::YoutubeSupport => return update_youtube(app),
        Id::Platform(i) => return toggle_platform(app, i),
        Id::YoutubeCookies => {
            set_status(
                app,
                "Pegá tus cookies de YouTube en ~/.config/media-lite/cookies/youtube.txt (formato Netscape o «Cookie:»).",
            );
            return;
        }
        _ => {
            let mut s = app.settings.borrow_mut();
            match id {
                Id::Quality => s.quality = next(&QUALITIES, s.quality),
                Id::VideoOutput => {
                    s.vo = next(&VIDEO_OUTPUTS, s.vo.as_str()).to_string();
                    stop_player(app);
                }
                Id::Decoder => {
                    s.hwdec = next(&DECODERS, s.hwdec.as_str()).to_string();
                    stop_player(app);
                }
                Id::LowVideo => s.low_video = !s.low_video,
                Id::Renderer => s.renderer = next(&RENDERERS, s.renderer.as_str()).to_string(),
                Id::PlexEnglish => s.plex_english = !s.plex_english,
                Id::ThumbCache => s.thumb_mem_mb = next(&CACHE_MB, s.thumb_mem_mb),
                Id::Theme => {
                    s.theme_dark = !s.theme_dark;
                    if let Some(w) = app.weak.upgrade() {
                        use slint::ComponentHandle;
                        w.global::<crate::Theme>().set_dark(s.theme_dark);
                    }
                }
                _ => return,
            }
            matches!(id, Id::Renderer | Id::PlexEnglish | Id::ThumbCache)
        }
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
        // Las filas fijas y una por plataforma (todas menos la de prueba).
        assert_eq!(r.len(), 12 + switchable().len());
        assert_eq!(r[0].value.as_str(), "480p");
        assert_eq!(r[1].value.as_str(), "xv");
        assert_eq!(r[3].value.as_str(), "No");
        assert!(r[3].switch && !r[3].on);
    }

    #[test]
    fn every_row_does_what_its_label_says() {
        let s = Settings::default();
        let l = layout(&s);
        let find = |label: &str| {
            l.iter()
                .position(|(_, r)| r.label.as_str() == label)
                .unwrap()
        };
        assert_eq!(l[find("Entrada en el menú")].0, Id::Menu);
        assert_eq!(l[find("Perfil equipo lento")].0, Id::SlowProfile);
        assert_eq!(l[find("Soporte de YouTube")].0, Id::YoutubeSupport);
        assert_eq!(l[find("Cookies de YouTube")].0, Id::YoutubeCookies);
        assert_eq!(l[find("Tema")].0, Id::Theme);
    }

    #[test]
    fn each_group_title_appears_once_and_first_row_has_one() {
        let l = layout(&Settings::default());
        assert!(!l[0].1.group.is_empty());
        let titles: Vec<&str> = l
            .iter()
            .map(|(_, r)| r.group.as_str())
            .filter(|g| !g.is_empty())
            .collect();
        let mut unique = titles.clone();
        unique.dedup();
        assert_eq!(titles, unique);
        assert_eq!(
            titles,
            ["Reproducción", "Interfaz", "YouTube", "Plataformas"]
        );
    }
}
