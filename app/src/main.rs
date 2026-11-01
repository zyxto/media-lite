//! media-lite: la interfaz de escritorio.
//!
//! Regla de hilos: los proveedores y las miniaturas se piden en el pool de hilos; los resultados
//! vuelven al hilo de la interfaz con `invoke_from_event_loop`. Todo el estado de la interfaz
//! vive en un `thread_local` que solo toca el hilo principal.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use media_lite_core::pool::{Generation, Pool};
use media_lite_core::store::{Settings, Store};
use media_lite_core::thumbs::{self, Thumb, ThumbCache};
use media_lite_core::{http, paths, Item, Section};
use media_lite_providers::{registry, Provider};
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
    Weak,
};

slint::include_modules!();

/// Tamaño de las miniaturas: lo que se ve en una tarjeta. Así no se guardan imágenes de 1080p.
const THUMB_W: u32 = 256;
const THUMB_H: u32 = 144;
/// Hilos de trabajo: dos de red y uno de imágenes, pensado para dos núcleos.
const THREADS: usize = 3;

/// Ancho mínimo de una tarjeta; define cuántas columnas caben (la misma cuenta que en Slint).
const CARD_MIN_WIDTH: f32 = 240.0;
const GAP: f32 = 16.0;
const SIDEBAR: f32 = 200.0;
const GUTTER: f32 = 40.0;

/// Estado de la interfaz, solo en el hilo principal.
struct Ui {
    items: Vec<Item>,
    images: Vec<Option<Image>>,
    rows: Rc<VecModel<RowData>>,
    columns: usize,
    /// Generación del listado que se muestra. Resultados de otras generaciones se descartan.
    listing: u64,
}

impl Ui {
    fn rebuild(&mut self) {
        let count = self.items.len().div_ceil(self.columns);
        let rows: Vec<RowData> = (0..count).map(|r| self.row(r)).collect();
        self.rows.set_vec(rows);
    }

    fn row(&self, r: usize) -> RowData {
        let start = r * self.columns;
        let end = (start + self.columns).min(self.items.len());
        let cards: Vec<CardData> = (start..end).map(|i| self.card(i)).collect();
        RowData {
            cards: ModelRc::new(VecModel::from(cards)),
        }
    }

    fn card(&self, i: usize) -> CardData {
        let item = &self.items[i];
        let image = self.images[i].clone();
        CardData {
            title: SharedString::from(item.title.as_str()),
            subtitle: SharedString::from(item.subtitle.as_str()),
            thumb: image.clone().unwrap_or_default(),
            has_thumb: image.is_some(),
            index: i as i32,
        }
    }
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

/// Lo que el usuario eligió: plataforma, sección y búsqueda, si hay una.
#[derive(Clone)]
struct Nav {
    platform: usize,
    section: usize,
    query: Option<String>,
}

/// Estado compartido de la aplicación. Vive en el hilo principal.
struct App {
    providers: Vec<Arc<dyn Provider>>,
    pool: Pool,
    thumbs: Arc<ThumbCache>,
    generation: Arc<Generation>,
    store: Store,
    settings: RefCell<Settings>,
    nav: RefCell<Nav>,
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().filter_or("MEDIA_LITE_LOG", "warn"))
        .init();

    let store = Store::default_dir();
    let settings = store.load_settings();
    if std::env::var_os("SLINT_BACKEND").is_none() {
        std::env::set_var("SLINT_BACKEND", &settings.renderer);
    }

    let providers = registry();
    let thumb_dir = paths::cache_dir().join("thumbs");
    let pool = Pool::new(THREADS, "media-lite");
    let thumbs = Arc::new(ThumbCache::new(
        http::agent(),
        Some(thumb_dir.clone()),
        (settings.thumb_mem_mb as usize) << 20,
        THUMB_W,
        THUMB_H,
    ));
    let disk_budget = (settings.thumb_disk_mb as u64) << 20;
    pool.run(move || {
        let _ = thumbs::prune_disk(&thumb_dir, disk_budget);
    });

    let window = AppWindow::new()?;
    let columns = columns_for(settings.window_width as f32);
    UI.with(|cell| {
        *cell.borrow_mut() = Some(Ui {
            items: Vec::new(),
            images: Vec::new(),
            rows: Rc::new(VecModel::default()),
            columns,
            listing: 0,
        });
    });
    let rows_model = UI.with(|cell| cell.borrow().as_ref().map(|ui| ui.rows.clone()));
    if let Some(rows) = rows_model {
        window.set_rows(ModelRc::from(rows));
    }

    let app = Rc::new(App {
        providers,
        pool,
        thumbs,
        generation: Arc::new(Generation::default()),
        store,
        settings: RefCell::new(settings.clone()),
        nav: RefCell::new(Nav {
            platform: 0,
            section: 1,
            query: None,
        }),
    });

    window.global::<Theme>().set_dark(settings.theme_dark);
    let platform_names: Vec<SharedString> = app
        .providers
        .iter()
        .map(|p| SharedString::from(p.platform().label()))
        .collect();
    window.set_platforms(ModelRc::new(VecModel::from(platform_names)));
    let section_names: Vec<SharedString> = Section::ALL
        .iter()
        .map(|s| SharedString::from(s.label()))
        .collect();
    window.set_sections(ModelRc::new(VecModel::from(section_names)));
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
    load(&app, &window.as_weak());

    if let Some(secs) = std::env::var("MEDIA_LITE_EXIT_AFTER")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        // Para pruebas automáticas: cierra solo después de N segundos.
        slint::Timer::single_shot(Duration::from_secs(secs), || {
            let _ = slint::quit_event_loop();
        });
    }

    window.run()?;
    save_window_state(&app, &window);
    Ok(())
}

fn wire_callbacks(app: &Rc<App>, window: &AppWindow) {
    let weak = window.as_weak();

    {
        let app = app.clone();
        let weak = weak.clone();
        window.on_platform_clicked(move |i| {
            app.nav.borrow_mut().platform = i as usize;
            app.nav.borrow_mut().query = None;
            reset_detail(&weak);
            load(&app, &weak);
        });
    }
    {
        let app = app.clone();
        let weak = weak.clone();
        window.on_section_clicked(move |i| {
            {
                let mut nav = app.nav.borrow_mut();
                nav.section = i as usize;
                nav.query = None;
            }
            reset_detail(&weak);
            load(&app, &weak);
        });
    }
    {
        let app = app.clone();
        let weak = weak.clone();
        window.on_search_submitted(move |text| {
            let query = text.trim().to_string();
            app.nav.borrow_mut().query = (!query.is_empty()).then_some(query);
            reset_detail(&weak);
            load(&app, &weak);
        });
    }
    {
        let app = app.clone();
        let weak = weak.clone();
        window.on_refresh_requested(move || load(&app, &weak));
    }
    {
        let weak = weak.clone();
        window.on_back_requested(move || {
            if let Some(w) = weak.upgrade() {
                w.set_detail_open(false);
            }
        });
    }
    {
        let weak = weak.clone();
        window.on_card_clicked(move |i| show_detail(&weak, i as usize));
    }
    {
        let app = app.clone();
        let weak = weak.clone();
        window.on_theme_toggled(move || {
            if let Some(w) = weak.upgrade() {
                let dark = !w.global::<Theme>().get_dark();
                w.global::<Theme>().set_dark(dark);
                app.settings.borrow_mut().theme_dark = dark;
                save_settings(&app);
            }
        });
    }
    {
        let weak = weak.clone();
        window.on_width_changed(move |px| {
            let columns = columns_for(px);
            let changed = UI.with(|cell| {
                let mut slot = cell.borrow_mut();
                match slot.as_mut() {
                    Some(ui) if ui.columns != columns => {
                        ui.columns = columns;
                        ui.rebuild();
                        true
                    }
                    _ => false,
                }
            });
            if changed {
                if let Some(w) = weak.upgrade() {
                    w.set_columns(columns as i32);
                }
            }
        });
    }
    {
        let app = app.clone();
        let weak = weak.clone();
        window.window().on_close_requested(move || {
            if let Some(w) = weak.upgrade() {
                save_window_state(&app, &w);
            }
            slint::CloseRequestResponse::HideWindow
        });
    }
}

/// Pide el listado (o la búsqueda) de la navegación actual, en un hilo de trabajo.
fn load(app: &Rc<App>, weak: &Weak<AppWindow>) {
    let (provider_index, section, query) = {
        let nav = app.nav.borrow();
        (nav.platform, Section::ALL[nav.section], nav.query.clone())
    };
    let Some(provider) = app.providers.get(provider_index).cloned() else {
        return;
    };
    let generation = app.generation.next();
    if let Some(w) = weak.upgrade() {
        w.set_busy(true);
        w.set_status(SharedString::from("Cargando…"));
    }

    let weak = weak.clone();
    let thumbs = app.thumbs.clone();
    let pool = app.pool.clone();
    let shared_gen = app.generation.clone();
    app.pool.run(move || {
        let result = match &query {
            Some(q) => provider.search(q, None),
            None => provider.list(section, None, None),
        };
        match result {
            Ok(page) => {
                let urls: Vec<(usize, String)> = page
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, item)| item.thumb.clone().map(|url| (i, url)))
                    .collect();
                let status = match &query {
                    Some(q) => format!(
                        "{} · búsqueda «{}» · {} resultados",
                        provider.platform().label(),
                        q,
                        page.items.len()
                    ),
                    None => format!(
                        "{} · {} · {} elementos",
                        provider.platform().label(),
                        section.label(),
                        page.items.len()
                    ),
                };
                let items = page.items;
                let (w, g, gen_check) = (weak.clone(), generation, shared_gen.clone());
                let _ = slint::invoke_from_event_loop(move || {
                    if gen_check.current() == g {
                        show_listing(&w, g, items, status);
                    }
                });
                for (index, url) in urls {
                    let thumbs = thumbs.clone();
                    let gen_check = shared_gen.clone();
                    pool.run(move || {
                        if gen_check.current() != generation {
                            return;
                        }
                        match thumbs.get(&url) {
                            Ok(thumb) => {
                                let _ = slint::invoke_from_event_loop(move || {
                                    deliver_thumb(generation, index, thumb)
                                });
                            }
                            Err(e) => log::debug!("miniatura descartada: {e}"),
                        }
                    });
                }
            }
            Err(e) => {
                let message = format!("No se pudo cargar: {e}");
                let (w, g, gen_check) = (weak.clone(), generation, shared_gen.clone());
                let _ = slint::invoke_from_event_loop(move || {
                    if gen_check.current() == g {
                        show_listing(&w, g, Vec::new(), message);
                    }
                });
            }
        }
    });
}

/// Hilo principal: muestra el listado nuevo y descarta las miniaturas que quedaron de antes.
fn show_listing(weak: &Weak<AppWindow>, generation: u64, items: Vec<Item>, status: String) {
    UI.with(|cell| {
        if let Some(ui) = cell.borrow_mut().as_mut() {
            ui.listing = generation;
            ui.images = vec![None; items.len()];
            ui.items = items;
            ui.rebuild();
        }
    });
    if let Some(w) = weak.upgrade() {
        w.set_busy(false);
        w.set_status(SharedString::from(status));
    }
}

/// Hilo principal: pone una miniatura en su tarjeta, si el listado sigue siendo el mismo.
fn deliver_thumb(generation: u64, index: usize, thumb: Thumb) {
    let image = image_from_rgba(thumb.width, thumb.height, &thumb.rgba);
    UI.with(|cell| {
        if let Some(ui) = cell.borrow_mut().as_mut() {
            if ui.listing != generation || index >= ui.images.len() {
                return;
            }
            ui.images[index] = Some(image);
            let row = index / ui.columns;
            ui.rows.set_row_data(row, ui.row(row));
        }
    });
}

fn image_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(rgba);
    Image::from_rgba8(buffer)
}

fn show_detail(weak: &Weak<AppWindow>, index: usize) {
    let item = UI.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|ui| ui.items.get(index).cloned())
    });
    let (Some(item), Some(w)) = (item, weak.upgrade()) else {
        return;
    };
    w.set_detail_title(SharedString::from(item.title.as_str()));
    w.set_detail_subtitle(SharedString::from(item.subtitle.as_str()));
    w.set_detail_text(SharedString::from(
        "La reproducción llega en la Fase 2 del roadmap. Por ahora esta pantalla es el esqueleto del detalle.",
    ));
    w.set_detail_open(true);
}

fn reset_detail(weak: &Weak<AppWindow>) {
    if let Some(w) = weak.upgrade() {
        w.set_detail_open(false);
    }
}

fn columns_for(width_px: f32) -> usize {
    let available = width_px - SIDEBAR - GUTTER;
    ((available + GAP) / (CARD_MIN_WIDTH + GAP))
        .floor()
        .clamp(2.0, 8.0) as usize
}

fn save_settings(app: &App) {
    if let Err(e) = app.store.save_settings(&app.settings.borrow()) {
        log::warn!("no se pudieron guardar los ajustes: {e}");
    }
}

/// Guarda tamaño y posición de la ventana, para que el próximo inicio la ponga igual.
fn save_window_state(app: &App, window: &AppWindow) {
    let winit = window.window();
    let scale = winit.scale_factor();
    let size = winit.size().to_logical(scale);
    {
        let mut s = app.settings.borrow_mut();
        s.window_width = size.width.max(1.0) as u32;
        s.window_height = size.height.max(1.0) as u32;
        let position = winit.position().to_logical(scale);
        s.window_x = Some(position.x as i32);
        s.window_y = Some(position.y as i32);
    }
    save_settings(app);
}
