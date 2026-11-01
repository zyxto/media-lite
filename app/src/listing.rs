//! Listados, búsqueda, categorías, series y miniaturas. Lo pesado va en el pool; la grilla se
//! actualiza en el hilo principal.

use std::rc::Rc;

use media_lite_core::thumbs::Thumb;
use media_lite_core::{Category, Item, Kind, Season, Section};
use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, SharedString};

use crate::state::{self, set_status, sync_chips, with_app, App, Grid, SeriesView};

/// Pide el listado (o la búsqueda) de la navegación actual, y sus categorías.
pub fn load(app: &Rc<App>) {
    let (provider_index, section, category, query) = {
        let nav = app.nav.borrow();
        (
            nav.platform,
            Section::ALL[nav.section],
            nav.category.clone(),
            nav.query.clone(),
        )
    };
    let Some(provider) = app.providers.get(provider_index).cloned() else {
        return;
    };
    let generation = app.generation.next();
    if let Some(w) = app.weak.upgrade() {
        w.set_busy(true);
    }
    set_status(app, "Cargando…");

    let thumbs = app.thumbs.clone();
    let images = app.images.clone();
    let shared_gen = app.generation.clone();
    app.pool.run(move || {
        let categories = match &query {
            Some(_) => Vec::new(),
            None => provider.categories(section).unwrap_or_default(),
        };
        let result = match &query {
            Some(q) => provider.search(q, None),
            None => provider.list(section, category.as_deref(), None),
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
                let check = shared_gen.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| {
                        if check.current() == generation {
                            show_listing(app, generation, items, categories, status);
                        }
                    });
                });
                for (index, url) in urls {
                    let thumbs = thumbs.clone();
                    let check = shared_gen.clone();
                    images.run(move || {
                        if check.current() != generation {
                            return;
                        }
                        match thumbs.get(&url) {
                            Ok(thumb) => {
                                let _ = slint::invoke_from_event_loop(move || {
                                    with_app(|app| deliver_thumb(app, generation, index, thumb));
                                });
                            }
                            Err(e) => log::debug!("miniatura descartada: {e}"),
                        }
                    });
                }
            }
            Err(e) => {
                let message = format!("No se pudo cargar: {e}");
                let check = shared_gen.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| {
                        if check.current() == generation {
                            show_listing(app, generation, Vec::new(), Vec::new(), message);
                        }
                    });
                });
            }
        }
    });
}

/// Hilo principal: muestra el listado nuevo y descarta las miniaturas que quedaron de antes.
pub fn show_listing(
    app: &Rc<App>,
    generation: u64,
    items: Vec<Item>,
    categories: Vec<Category>,
    status: String,
) {
    let first = items.first().cloned();
    {
        let mut grid = app.grid.borrow_mut();
        grid.listing = generation;
        grid.images = vec![None; items.len()];
        grid.items = items;
        grid.categories = categories;
        grid.series = None;
        grid.detail = None;
        grid.rebuild();
    }
    sync_chips(app);
    if let Some(w) = app.weak.upgrade() {
        w.set_busy(false);
        w.set_detail_open(false);
    }
    set_status(app, status);
    // Gancho de desarrollo (prueba automática): abre el primer elemento del primer listado.
    if generation == 1 && std::env::var_os("MEDIA_LITE_AUTOPLAY").is_some() {
        if let Some(item) = first {
            crate::playback::request(app, item, Vec::new());
        }
    }
}

/// Hilo principal: pone una miniatura en su tarjeta, si el listado sigue siendo el mismo.
pub fn deliver_thumb(app: &App, generation: u64, index: usize, thumb: Thumb) {
    let image = image_from_rgba(thumb.width, thumb.height, &thumb.rgba);
    let mut grid = app.grid.borrow_mut();
    if grid.listing != generation || index >= grid.images.len() {
        return;
    }
    grid.images[index] = Some(image);
    let row = index / grid.columns;
    let data = grid.row(row);
    grid.rows.set_row_data(row, data);
}

fn image_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(rgba);
    Image::from_rgba8(buffer)
}

/// Un clic en una tarjeta: una serie abre sus temporadas; el resto, el detalle.
pub fn card_clicked(app: &Rc<App>, index: usize) {
    let item = app.grid.borrow().items.get(index).cloned();
    let Some(item) = item else {
        return;
    };
    if item.kind == Kind::Series {
        open_series(app, item);
    } else {
        show_detail(app, index);
    }
}

/// Abre la pantalla de detalle de la tarjeta `index`.
pub fn show_detail(app: &App, index: usize) {
    let item = {
        let mut grid = app.grid.borrow_mut();
        let item = grid.items.get(index).cloned();
        grid.detail = item.as_ref().map(|_| index);
        item
    };
    let (Some(item), Some(w)) = (item, app.weak.upgrade()) else {
        return;
    };
    w.set_detail_title(SharedString::from(item.title.as_str()));
    w.set_detail_subtitle(SharedString::from(item.subtitle.as_str()));
    let text = if item.description.is_empty() {
        "Sin descripción.".to_string()
    } else {
        item.description.clone()
    };
    w.set_detail_text(SharedString::from(text));
    w.set_detail_open(true);
}

/// Cierra el detalle y vuelve a la grilla.
pub fn close_detail(app: &App) {
    app.grid.borrow_mut().detail = None;
    if let Some(w) = app.weak.upgrade() {
        w.set_detail_open(false);
    }
}

/// Trae las temporadas de una serie y muestra sus episodios de la primera temporada.
pub fn open_series(app: &Rc<App>, series: Item) {
    let Some(provider) = app
        .providers
        .iter()
        .find(|p| p.platform() == series.platform)
        .cloned()
    else {
        return;
    };
    set_status(app, format!("Cargando «{}»…", series.title));
    let check = app.generation.clone();
    let generation = app.generation.next();
    app.pool.run(move || {
        let result = provider.seasons(&series);
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|app| match result {
                Ok(seasons) if check.current() == generation => show_series(app, series, seasons),
                Ok(_) => {}
                Err(e) => set_status(app, format!("No se pudo abrir la serie: {e}")),
            });
        });
    });
}

fn show_series(app: &Rc<App>, series: Item, seasons: Vec<Season>) {
    let title = series.title.clone();
    {
        let mut grid = app.grid.borrow_mut();
        grid.series = Some(SeriesView { seasons, season: 0 });
        grid.category = 0;
        grid.detail = None;
    }
    show_season(app, 0);
    if let Some(w) = app.weak.upgrade() {
        w.set_detail_open(false);
    }
    set_status(app, format!("Serie: {title}"));
}

/// Muestra los episodios de la temporada `index` de la serie abierta.
pub fn show_season(app: &Rc<App>, index: usize) {
    let episodes: Option<Vec<Item>> = {
        let mut grid = app.grid.borrow_mut();
        let Some(view) = grid.series.as_mut() else {
            return;
        };
        if index >= view.seasons.len() {
            return;
        }
        view.season = index;
        let episodes = view.seasons[index].episodes.clone();
        grid.category = index;
        Some(episodes)
    };
    let Some(episodes) = episodes else {
        return;
    };
    let (generation, urls) = {
        let mut grid = app.grid.borrow_mut();
        let generation = app.generation.next();
        grid.listing = generation;
        grid.images = vec![None; episodes.len()];
        let urls: Vec<(usize, String)> = episodes
            .iter()
            .enumerate()
            .filter_map(|(i, it)| it.thumb.clone().map(|u| (i, u)))
            .collect();
        grid.items = episodes;
        grid.rebuild();
        (generation, urls)
    };
    sync_chips(app);
    schedule_thumbs(app, generation, urls);
}

/// Cierra la serie y vuelve a la lista de la sección.
pub fn close_series(app: &Rc<App>) {
    app.grid.borrow_mut().series = None;
    load(app);
}

/// Un chip: cambia de categoría, o de temporada si hay una serie abierta.
pub fn chip_clicked(app: &Rc<App>, index: usize) {
    let in_series = app.grid.borrow().series.is_some();
    if in_series {
        show_season(app, index);
        return;
    }
    {
        let grid = app.grid.borrow();
        if index > grid.categories.len() {
            return;
        }
    }
    let category = if index == 0 {
        None
    } else {
        app.grid
            .borrow()
            .categories
            .get(index - 1)
            .map(|c| c.id.clone())
    };
    app.nav.borrow_mut().category = category;
    app.grid.borrow_mut().category = index;
    load(app);
}

/// Pide las miniaturas de una lista, con la generación para descartarlas si cambia la vista.
fn schedule_thumbs(app: &Rc<App>, generation: u64, urls: Vec<(usize, String)>) {
    let thumbs = app.thumbs.clone();
    let check = app.generation.clone();
    for (index, url) in urls {
        let thumbs = thumbs.clone();
        let check = check.clone();
        app.images.run(move || {
            if check.current() != generation {
                return;
            }
            if let Ok(thumb) = thumbs.get(&url) {
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| deliver_thumb(app, generation, index, thumb));
                });
            }
        });
    }
}

/// Columnas que caben para un ancho de ventana dado (en píxeles lógicos).
pub fn columns_for(width_px: f32) -> usize {
    let available = width_px - state::SIDEBAR - state::GUTTER;
    ((available + state::GAP) / (state::CARD_MIN_WIDTH + state::GAP))
        .floor()
        .clamp(2.0, 8.0) as usize
}

/// Grilla vacía para `Grid::new` (lo usa `main`).
pub fn empty_grid(columns: usize) -> Grid {
    Grid::new(columns)
}
