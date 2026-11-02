//! media-lite: la interfaz de escritorio.
//!
//! Regla de hilos: los proveedores, las miniaturas y los detalles se piden en el pool de hilos;
//! los resultados vuelven al hilo principal con `invoke_from_event_loop`. Todo el estado de la
//! interfaz vive en el hilo principal (ver `state`).

mod chat;
mod detail;
mod follow;
mod guide;
mod library;
mod listing;
mod menu;
mod music;
mod playback;
mod settings;
mod state;

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use media_lite_core::history;
use media_lite_core::pool::{Generation, Pool};
use media_lite_core::store::Store;
use media_lite_core::thumbs::{self, ThumbCache};
use media_lite_core::{http, paths, Item, Section};
use media_lite_player::Proxy;
use media_lite_providers::registry;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use state::{App, Nav};

slint::include_modules!();

/// Tamaño de las miniaturas: lo que se ve en una tarjeta. Así no se guardan imágenes de 1080p.
const THUMB_W: u32 = 256;
const THUMB_H: u32 = 144;
/// Hilos de trabajo: dos de red y uno de imágenes, pensado para dos núcleos.
const NET_THREADS: usize = 2;
const IMAGE_THREADS: usize = 1;

/// Índice elegido por una variable de entorno de desarrollo; `default` si falta o no entra en `len`.
fn dev_index(var: &str, len: usize, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n < len)
        .unwrap_or(default)
}

fn main() -> Result<(), Box<dyn Error>> {
    // Proceso aparte del solucionador de YouTube: lee JSON por stdin y responde por stdout.
    if std::env::args().nth(1).as_deref() == Some("--yt-solver") {
        std::process::exit(media_lite_ytsolver::child_main());
    }
    env_logger::Builder::from_env(env_logger::Env::default().filter_or("MEDIA_LITE_LOG", "warn"))
        .init();

    let store = Store::default_dir();
    let settings = store.load_settings();
    if std::env::var_os("SLINT_BACKEND").is_none() {
        std::env::set_var("SLINT_BACKEND", &settings.renderer);
    }

    // Las plataformas ocultas en Ajustes no se cargan (si todas estuvieran ocultas, se muestran).
    let providers: Vec<_> = {
        let all = registry(store.dir(), settings.plex_english);
        let visible: Vec<_> = all
            .iter()
            .filter(|p| {
                !settings
                    .hidden_platforms
                    .iter()
                    .any(|k| k == p.platform().key())
            })
            .cloned()
            .collect();
        if visible.is_empty() {
            all
        } else {
            visible
        }
    };
    let following: Vec<Item> = store.load_list("following.json");
    let providers_len = providers.len();
    let thumb_dir = paths::cache_dir().join("thumbs");
    let pool = Pool::new(NET_THREADS, "media-lite-net");
    let images = Pool::new(IMAGE_THREADS, "media-lite-img");
    let thumbs = Arc::new(ThumbCache::new(
        http::agent(),
        Some(thumb_dir.clone()),
        (settings.thumb_mem_mb as usize) << 20,
        THUMB_W,
        THUMB_H,
    ));
    let disk_budget = (settings.thumb_disk_mb as u64) << 20;
    images.run(move || {
        let _ = thumbs::prune_disk(&thumb_dir, disk_budget);
    });

    let proxy = match Proxy::start(http::agent()) {
        Ok(proxy) => Some(Arc::new(proxy)),
        Err(e) => {
            log::warn!("sin proxy de playlists: {e}");
            None
        }
    };
    let history = store.load_list::<history::Entry>("history.json");
    let favorites = store.load_list::<media_lite_core::Item>("favorites.json");

    let window = AppWindow::new()?;
    let columns = listing::columns_for(settings.window_width as f32);
    let grid = listing::empty_grid(columns);
    window.set_rows(ModelRc::from(grid.rows.clone()));
    window.set_episodes(ModelRc::from(grid.episodes.clone()));

    let app = Rc::new(App {
        weak: window.as_weak(),
        providers,
        pool,
        images,
        thumbs,
        generation: Arc::new(Generation::default()),
        store,
        settings: RefCell::new(settings.clone()),
        nav: RefCell::new(Nav {
            library: None,
            global: false,
            // Ganchos de desarrollo: MEDIA_LITE_PLATFORM / MEDIA_LITE_SECTION eligen el arranque.
            platform: dev_index("MEDIA_LITE_PLATFORM", providers_len, 0),
            section: dev_index("MEDIA_LITE_SECTION", Section::ALL.len(), 1),
            category: None,
            query: None,
        }),
        proxy,
        player: RefCell::new(None),
        playback: RefCell::new(None),
        history: RefCell::new(history),
        favorites: RefCell::new(favorites),
        grid: RefCell::new(grid),
        chat: RefCell::new(chat::Chat::default()),
        following: RefCell::new(following),
        feed: RefCell::new(Vec::new()),
    });
    // Lo lento de cada plataforma (el reproductor de YouTube, por ejemplo) se prepara en segundo
    // plano, sin bloquear la ventana.
    for provider in &app.providers {
        let provider = Arc::clone(provider);
        app.pool.run(move || provider.warm());
    }
    state::install(app.clone());

    window.global::<Theme>().set_dark(settings.theme_dark);
    let mut platform_names: Vec<SharedString> = app
        .providers
        .iter()
        .map(|p| SharedString::from(p.platform().label()))
        .collect();
    // La última entrada busca en todas las plataformas a la vez.
    platform_names.push(SharedString::from("Todas las plataformas"));
    window.set_platforms(ModelRc::new(VecModel::from(platform_names)));
    window.set_columns(columns as i32);
    window.window().set_size(slint::LogicalSize::new(
        settings.window_width as f32,
        settings.window_height as f32,
    ));
    if let (Some(x), Some(y)) = (settings.window_x, settings.window_y) {
        window
            .window()
            .set_position(slint::LogicalPosition::new(x as f32, y as f32));
    }

    wire_callbacks(&app, &window);
    listing::load(&app);

    // Lee los eventos de mpv y refresca la barra del reproductor, cuatro veces por segundo.
    let player_timer = slint::Timer::default();
    player_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        || {
            state::with_app(playback::pump);
        },
    );

    if let Some(secs) = std::env::var("MEDIA_LITE_EXIT_AFTER")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        // Para pruebas automáticas: cierra solo después de N segundos.
        slint::Timer::single_shot(Duration::from_secs(secs), || {
            let _ = slint::quit_event_loop();
        });
    }

    // Gancho de desarrollo: MEDIA_LITE_SNAPSHOT=ruta.ppm guarda lo que dibuja la ventana a los
    // MEDIA_LITE_SNAPSHOT_AFTER segundos (8 si falta). Sale del propio renderizador: sirve para
    // revisar la interfaz sin sacar capturas del escritorio.
    if let Ok(path) = std::env::var("MEDIA_LITE_SNAPSHOT") {
        let secs = std::env::var("MEDIA_LITE_SNAPSHOT_AFTER")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(8);
        let weak = window.as_weak();
        slint::Timer::single_shot(Duration::from_secs(secs), move || {
            if let Some(w) = weak.upgrade() {
                match save_snapshot(&w, &path) {
                    Ok(()) => log::info!("captura guardada en {path}"),
                    Err(e) => log::warn!("no se pudo guardar la captura: {e}"),
                }
            }
        });
    }

    window.run()?;
    // Cerrar mpv explícitamente: el estado de hilo local no se destruye al salir del proceso.
    state::with_app(playback::stop);
    save_window_state(&app);
    Ok(())
}

/// Guarda lo que dibuja la ventana como imagen PPM (sin dependencias; se convierte con cualquier
/// herramienta). Solo para el gancho de desarrollo `MEDIA_LITE_SNAPSHOT`.
fn save_snapshot(window: &AppWindow, path: &str) -> Result<(), Box<dyn Error>> {
    use std::io::Write;
    let shot = window.window().take_snapshot()?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(out, "P6\n{} {}\n255\n", shot.width(), shot.height())?;
    for pixel in shot.as_slice() {
        out.write_all(&[pixel.r, pixel.g, pixel.b])?;
    }
    out.flush()?;
    Ok(())
}

fn wire_callbacks(app: &Rc<App>, window: &AppWindow) {
    {
        let app = app.clone();
        window.on_platform_clicked(move |i| {
            {
                let mut nav = app.nav.borrow_mut();
                nav.global = i as usize >= app.providers.len();
                nav.platform = (i as usize).min(app.providers.len().saturating_sub(1));
                nav.query = None;
                nav.library = None;
            }
            if let Some(w) = app.weak.upgrade() {
                w.set_platform_index(i);
            }
            detail::close(&app);
            listing::load(&app);
        });
    }
    {
        let app = app.clone();
        window.on_section_clicked(move |i| {
            {
                let mut nav = app.nav.borrow_mut();
                nav.section = i as usize;
                nav.query = None;
                nav.library = None;
            }
            detail::close(&app);
            listing::load(&app);
        });
    }
    {
        let app = app.clone();
        window.on_search_submitted(move |text| {
            let query = text.trim().to_string();
            app.nav.borrow_mut().query = (!query.is_empty()).then_some(query);
            app.nav.borrow_mut().library = None;
            detail::close(&app);
            listing::load(&app);
        });
    }
    {
        let app = app.clone();
        window.on_refresh_requested(move || listing::load(&app));
    }
    {
        let app = app.clone();
        window.on_back_requested(move || {
            let settings_open = app.weak.upgrade().is_some_and(|w| w.get_settings_open());
            let detail_open = app.grid.borrow().detail.is_some();
            let in_series = app.grid.borrow().series.is_some();
            if settings_open {
                settings::close(&app);
            } else if detail_open {
                detail::close(&app);
            } else if in_series {
                listing::close_series(&app);
            }
        });
    }
    {
        let app = app.clone();
        window.on_card_clicked(move |i| listing::card_clicked(&app, i as usize));
    }
    {
        let app = app.clone();
        window.on_play_requested(move || detail::play(&app));
    }
    {
        let app = app.clone();
        window.on_episode_clicked(move |i| detail::play_episode(&app, i.max(0) as usize));
    }
    {
        let app = app.clone();
        window.on_toggle_pause_requested(move || playback::toggle_pause(&app));
    }
    {
        let app = app.clone();
        window.on_seek_requested(move |seconds: f32| playback::seek_relative(&app, seconds as f64));
    }
    {
        let app = app.clone();
        window.on_seek_to_requested(move |ratio: f32| playback::seek_ratio(&app, ratio as f64));
    }
    {
        let app = app.clone();
        window.on_stop_requested(move || playback::stop(&app));
    }
    {
        let app = app.clone();
        window.on_prev_requested(move || playback::step(&app, -1));
    }
    {
        let app = app.clone();
        window.on_next_requested(move || playback::step(&app, 1));
    }
    {
        let app = app.clone();
        window.on_chip_clicked(move |i| listing::chip_clicked(&app, i as usize));
    }
    {
        let app = app.clone();
        window.on_fullscreen_requested(move || playback::toggle_fullscreen(&app));
    }
    {
        let app = app.clone();
        window.on_audio_cycle_requested(move || playback::cycle_audio(&app));
    }
    {
        // Aleatorio y repetir: los modos viven en la cola de música y se aplican en el próximo cambio.
        let app = app.clone();
        window.on_volume_requested(move |delta| playback::volume_step(&app, delta as f64));
    }
    {
        let app = app.clone();
        window.on_quality_requested(move || playback::cycle_quality(&app));
    }
    {
        let app = app.clone();
        window.on_shuffle_requested(move || {
            crate::music::toggle_shuffle();
            playback::sync_ui(&app);
            let on = crate::music::modes().shuffle;
            state::set_status(
                &app,
                if on {
                    "Aleatorio: sí"
                } else {
                    "Aleatorio: no"
                },
            );
        });
    }
    {
        let app = app.clone();
        window.on_repeat_requested(move || {
            crate::music::cycle_repeat();
            playback::sync_ui(&app);
            state::set_status(&app, crate::music::modes().repeat.label());
        });
    }
    {
        let app = app.clone();
        window.on_subtitle_cycle_requested(move || playback::cycle_subtitles(&app));
    }
    {
        let app = app.clone();
        window.on_settings_requested(move || {
            let open = app.weak.upgrade().is_some_and(|w| w.get_settings_open());
            if open {
                settings::close(&app);
            } else {
                settings::open(&app);
            }
        });
    }
    {
        let app = app.clone();
        window.on_library_clicked(move |i| {
            if let Some(shelf) = library::Shelf::from_index(i as usize) {
                settings::close(&app);
                library::open(&app, shelf);
            }
        });
    }
    {
        let app = app.clone();
        window.on_favorite_requested(move || library::toggle_favorite(&app));
    }
    {
        let app = app.clone();
        window.on_visible_rows_changed(move |first, last| {
            listing::visible_rows(&app, first.max(0) as usize, last.max(0) as usize)
        });
    }
    {
        let app = app.clone();
        window.on_setting_clicked(move |i| settings::click(&app, i as usize));
    }
    {
        let app = app.clone();
        window.on_theme_toggled(move || {
            if let Some(w) = app.weak.upgrade() {
                let dark = !w.global::<Theme>().get_dark();
                w.global::<Theme>().set_dark(dark);
                app.settings.borrow_mut().theme_dark = dark;
                save_settings(&app);
            }
        });
    }
    {
        let app = app.clone();
        window.on_width_changed(move |px| {
            let columns = listing::columns_for(px);
            let changed = {
                let mut grid = app.grid.borrow_mut();
                if grid.columns != columns {
                    grid.columns = columns;
                    grid.rebuild();
                    true
                } else {
                    false
                }
            };
            if changed {
                if let Some(w) = app.weak.upgrade() {
                    w.set_columns(columns as i32);
                }
                state::sync_skeleton(&app);
            }
        });
    }
    {
        let app = app.clone();
        window.window().on_close_requested(move || {
            playback::stop(&app);
            save_window_state(&app);
            slint::CloseRequestResponse::HideWindow
        });
    }
}

fn save_settings(app: &App) {
    if let Err(e) = app.store.save_settings(&app.settings.borrow()) {
        log::warn!("no se pudieron guardar los ajustes: {e}");
    }
}

/// Guarda tamaño y posición de la ventana, para que el próximo inicio la ponga igual.
fn save_window_state(app: &App) {
    let Some(window) = app.weak.upgrade() else {
        return;
    };
    let winit = window.window();
    let scale = winit.scale_factor();
    let size = winit.size().to_logical(scale);
    let position = winit.position().to_logical(scale);
    {
        let mut s = app.settings.borrow_mut();
        s.window_width = size.width.max(1.0) as u32;
        s.window_height = size.height.max(1.0) as u32;
        s.window_x = Some(position.x as i32);
        s.window_y = Some(position.y as i32);
    }
    save_settings(app);
}
