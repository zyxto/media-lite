//! Listados, búsqueda, categorías, series y miniaturas. Lo pesado va en el pool; la grilla se
//! actualiza en el hilo principal.

use std::rc::Rc;
use std::sync::Arc;

use media_lite_core::thumbs::Thumb;
use media_lite_core::{Category, Item, Kind, Page, Season, Section};
use media_lite_providers::Provider;
use slint::{ComponentHandle, Image, Model, Rgba8Pixel, SharedPixelBuffer, SharedString};

use crate::detail;
use crate::state::{self, set_status, sync_chips, with_app, App, Grid, SeriesView};

/// Pide el listado (o la búsqueda) de la navegación actual, y sus categorías.
pub fn load(app: &Rc<App>) {
    // Cargar un listado (plataforma, sección, búsqueda, categoría, F5) saca de Ajustes.
    crate::settings::close(app);
    let shelf = app.nav.borrow().library;
    if let Some(shelf) = shelf {
        crate::library::show(app, shelf);
        return;
    }
    state::sync_nav(app);
    let (provider_index, section, category, query) = {
        let nav = app.nav.borrow();
        (
            nav.platform,
            Section::ALL[nav.section],
            nav.category.clone(),
            nav.query.clone(),
        )
    };
    if app.nav.borrow().global {
        match query {
            Some(q) => global_search(app, q),
            None => global_section(app, section),
        }
        return;
    }
    let Some(provider) = app.providers.get(provider_index).cloned() else {
        return;
    };
    let generation = app.generation.next();
    // Mientras carga, la grilla queda vacía: se ve el esqueleto y no el listado anterior.
    clear_grid(app);
    state::set_activity(app, crate::Activity::Catalog);
    set_status(app, "Cargando…");

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
                            state::set_activity(app, crate::Activity::Idle);
                            show_listing(app, generation, items, categories, status);
                        }
                    });
                });
            }
            Err(e) => {
                let message = format!("No se pudo cargar: {e}");
                let check = shared_gen.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| {
                        if check.current() == generation {
                            state::set_activity(app, crate::Activity::Idle);
                            show_listing(app, generation, Vec::new(), Vec::new(), message.clone());
                            state::set_error(app, message);
                        }
                    });
                });
            }
        }
    });
}

/// Busca en todas las plataformas a la vez. Cada una responde por su lado y sus resultados se
/// agregan a la grilla al llegar, con el nombre de la plataforma en el subtítulo.
pub fn global_search(app: &Rc<App>, query: String) {
    let generation = app.generation.next();
    show_listing(
        app,
        generation,
        Vec::new(),
        Vec::new(),
        format!("Buscando «{query}» en todas las plataformas…"),
    );
    app.grid.borrow_mut().pending = app.providers.len();
    state::set_activity(app, crate::Activity::Catalog);
    let providers = app.providers.clone();
    for provider in providers {
        let query = query.clone();
        app.pool.run(move || {
            let name = provider.platform().label();
            let result = provider.search(&query, None);
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| {
                    part_arrived(app, generation, name, result, move |count, done| {
                        if done {
                            format!(
                                "Búsqueda «{query}» en todas las plataformas · {count} resultados"
                            )
                        } else {
                            format!("Buscando «{query}»… {count} resultados hasta ahora")
                        }
                    })
                });
            });
        });
    }
}

/// «Todas las plataformas» sin búsqueda: la sección (en vivo, películas, series…) de cada
/// plataforma que la tiene, en una sola grilla.
pub fn global_section(app: &Rc<App>, section: Section) {
    let label = section.label();
    let providers: Vec<Arc<dyn Provider>> = app
        .providers
        .iter()
        .filter(|p| p.caps().offers(section))
        .cloned()
        .collect();
    let generation = app.generation.next();
    if providers.is_empty() {
        show_listing(
            app,
            generation,
            Vec::new(),
            Vec::new(),
            format!("Ninguna plataforma tiene «{label}»."),
        );
        return;
    }
    show_listing(
        app,
        generation,
        Vec::new(),
        Vec::new(),
        format!("Cargando {label} de todas las plataformas…"),
    );
    app.grid.borrow_mut().pending = providers.len();
    state::set_activity(app, crate::Activity::Catalog);
    for provider in providers {
        app.pool.run(move || {
            let name = provider.platform().label();
            let result = provider.list(section, None, None);
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| {
                    part_arrived(app, generation, name, result, move |count, done| {
                        if done {
                            format!("{label} de todas las plataformas · {count} elementos")
                        } else {
                            format!("Cargando {label}… {count} elementos hasta ahora")
                        }
                    })
                });
            });
        });
    }
}

/// Llegó la parte de una plataforma a un listado de todas: se agrega a la grilla (con el nombre de la
/// plataforma en el subtítulo) y se actualiza el estado. `status(cantidad, terminó)` dice qué mostrar.
fn part_arrived(
    app: &Rc<App>,
    generation: u64,
    name: &str,
    result: media_lite_core::Result<Page>,
    status: impl Fn(usize, bool) -> String,
) {
    if app.generation.current() != generation {
        return;
    }
    let found: Vec<Item> = match result {
        Ok(page) => page
            .items
            .into_iter()
            .map(|mut item| {
                item.subtitle = if item.subtitle.is_empty() {
                    name.to_string()
                } else {
                    format!("{name} · {}", item.subtitle)
                };
                item
            })
            .collect(),
        Err(e) => {
            log::info!("{name}: {e}");
            Vec::new()
        }
    };
    let pending = {
        let mut grid = app.grid.borrow_mut();
        grid.pending = grid.pending.saturating_sub(1);
        grid.pending
    };
    append_items(app, generation, found);
    if pending == 0 {
        state::set_activity(app, crate::Activity::Idle);
    }
    let count = app.grid.borrow().items.len();
    set_status(app, status(count, pending == 0));
}

/// Agrega resultados al final de la grilla, si siguen siendo del mismo listado.
fn append_items(app: &Rc<App>, generation: u64, items: Vec<Item>) {
    if items.is_empty() {
        return;
    }
    {
        let mut grid = app.grid.borrow_mut();
        if grid.listing != generation {
            return;
        }
        let added = items.len();
        grid.images.extend(std::iter::repeat_n(None, added));
        grid.requested.extend(std::iter::repeat_n(false, added));
        grid.items.extend(items);
        grid.rebuild();
    }
    visible_now(app);
}

/// Vacía la grilla sin mostrar mensajes: mientras carga, lo que se ve es el esqueleto.
fn clear_grid(app: &App) {
    {
        let mut grid = app.grid.borrow_mut();
        grid.items.clear();
        grid.images.clear();
        grid.requested.clear();
        grid.categories.clear();
        grid.series = None;
        grid.detail = None;
        grid.rebuild();
    }
    sync_chips(app);
    state::sync_skeleton(app);
    if let Some(w) = app.weak.upgrade() {
        w.set_empty_message(SharedString::new());
        // Mientras carga se ve el esqueleto, no el título que había abierto.
        detail::hide(&w);
    }
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
    // Si no hay elementos, el mensaje de la barra también se muestra en el centro de la grilla.
    let empty_message = if items.is_empty() {
        status.clone()
    } else {
        String::new()
    };
    {
        let mut grid = app.grid.borrow_mut();
        grid.listing = generation;
        grid.images = vec![None; items.len()];
        grid.requested = vec![false; items.len()];
        grid.items = items;
        grid.categories = categories;
        grid.series = None;
        grid.detail = None;
        grid.rebuild();
    }
    sync_chips(app);
    if let Some(w) = app.weak.upgrade() {
        w.set_empty_message(empty_message.into());
        detail::hide(&w);
    }
    set_status(app, status);
    visible_now(app);
    // Gancho de desarrollo (prueba automática): MEDIA_LITE_OPEN=settings abre Ajustes.
    if generation == 1 {
        if let Ok(query) = std::env::var("MEDIA_LITE_SEARCH") {
            app.nav.borrow_mut().global = true;
            global_search(app, query);
        }
        match std::env::var("MEDIA_LITE_OPEN").as_deref() {
            Ok("settings") => crate::settings::open(app),
            Ok("home") => crate::library::open(app, crate::library::Shelf::Home),
            Ok("history") => crate::library::open(app, crate::library::Shelf::History),
            Ok("favorites") => crate::library::open(app, crate::library::Shelf::Favorites),
            // Abre la pantalla del primer título (o de la primera serie) del listado.
            Ok("detail") => open_first(app, false),
            Ok("series") => open_first(app, true),
            _ => {}
        }
    }
    // Gancho de desarrollo (prueba automática): abre el primer elemento del primer listado.
    if generation == 1 && std::env::var_os("MEDIA_LITE_AUTOPLAY").is_some() {
        if let Some(item) = first {
            crate::playback::request(app, item, Vec::new());
        }
    }
}

/// Gancho de desarrollo: abre el primer elemento que es una serie (o que no lo es).
/// MEDIA_LITE_PICK=n salta los primeros n.
fn open_first(app: &Rc<App>, series: bool) {
    let skip = std::env::var("MEDIA_LITE_PICK")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let index = app
        .grid
        .borrow()
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| (item.kind == Kind::Series) == series)
        .nth(skip)
        .map(|(i, _)| i);
    if let Some(index) = index {
        card_clicked(app, index);
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
    if grid.series.is_some() {
        let episode = grid.episode(index);
        grid.episodes.set_row_data(index, episode);
    }
}

pub fn image_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(rgba);
    Image::from_rgba8(buffer)
}

/// Un clic en una tarjeta: una serie abre sus temporadas; el resto, su pantalla.
pub fn card_clicked(app: &Rc<App>, index: usize) {
    let item = app.grid.borrow().items.get(index).cloned();
    let Some(item) = item else {
        return;
    };
    if item.kind == Kind::Series {
        let placeholder = app.grid.borrow().images.get(index).cloned().flatten();
        open_series(app, item, placeholder);
    } else {
        detail::show_item(app, index);
    }
}

/// Trae las temporadas de una serie y muestra su pantalla, con los episodios de la primera
/// temporada. `placeholder` es la miniatura de su tarjeta: se ve hasta que llega la imagen grande.
pub fn open_series(app: &Rc<App>, series: Item, placeholder: Option<Image>) {
    let Some(provider) = app
        .providers
        .iter()
        .find(|p| p.platform() == series.platform)
        .cloned()
    else {
        return;
    };
    let check = app.generation.clone();
    let generation = app.generation.next();
    clear_grid(app);
    app.grid.borrow_mut().placeholder = placeholder;
    state::set_activity(app, crate::Activity::Catalog);
    set_status(app, format!("Cargando «{}»…", series.title));
    app.pool.run(move || {
        let result = provider.seasons(&series);
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|app| match result {
                Ok(seasons) if check.current() == generation => {
                    state::set_activity(app, crate::Activity::Idle);
                    show_series(app, series, seasons);
                }
                Err(e) if check.current() == generation => {
                    state::set_activity(app, crate::Activity::Idle);
                    let message = format!("No se pudo abrir la serie: {e}");
                    show_listing(app, generation, Vec::new(), Vec::new(), message.clone());
                    state::set_error(app, message);
                }
                _ => {}
            });
        });
    });
}

fn show_series(app: &Rc<App>, series: Item, seasons: Vec<Season>) {
    let title = series.title.clone();
    {
        let mut grid = app.grid.borrow_mut();
        grid.series = Some(SeriesView {
            item: series,
            seasons,
            season: 0,
        });
        grid.category = 0;
        grid.detail = None;
    }
    show_season(app, 0);
    detail::show_series(app);
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
    let generation = {
        let mut grid = app.grid.borrow_mut();
        let generation = app.generation.next();
        grid.listing = generation;
        grid.images = vec![None; episodes.len()];
        grid.requested = vec![false; episodes.len()];
        grid.items = episodes;
        grid.rebuild();
        generation
    };
    sync_chips(app);
    let _ = generation;
    visible_now(app);
    detail::sync_series_play_label(app);
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

/// Filas de margen a cada lado de lo visible: se piden un poco antes de que aparezcan.
const ROW_MARGIN: usize = 2;
/// Altura de una fila de la grilla, en píxeles lógicos, para una tarjeta de `card_w` de ancho.
fn row_height(card_w: f32) -> f32 {
    card_w * 9.0 / 16.0 + 64.0
}

/// Miniaturas que se piden de una temporada. La lista de episodios no avisa qué parte se ve,
/// así que se piden todas hasta este tope (cada una, ya decodificada, ocupa unos 150 KB).
const EPISODE_THUMBS: usize = 48;

/// Calcula las filas visibles según el tamaño de la ventana, para el listado recién mostrado.
fn visible_now(app: &Rc<App>) {
    let Some(window) = app.weak.upgrade() else {
        return;
    };
    if app.grid.borrow().series.is_some() {
        let columns = app.grid.borrow().columns.max(1);
        visible_rows(app, 0, EPISODE_THUMBS.div_ceil(columns));
        return;
    }
    let scale = window.window().scale_factor();
    let size = window.window().size().to_logical(scale);
    let columns = app.grid.borrow().columns.max(1) as f32;
    let card_w =
        (size.width - state::SIDEBAR - state::GUTTER - (columns - 1.0) * state::GAP) / columns;
    // Encabezado, chips y barra del reproductor: unos 260 px no son de la grilla.
    let grid_h = (size.height - 260.0).max(0.0);
    let last = (grid_h / row_height(card_w)).ceil() as usize + 1;
    visible_rows(app, 0, last);
}

/// Pide las miniaturas de las filas `first..=last` y libera las que quedaron lejos.
pub fn visible_rows(app: &Rc<App>, first: usize, last: usize) {
    let (generation, to_load) = {
        let mut grid = app.grid.borrow_mut();
        let cols = grid.columns.max(1);
        let total_rows = grid.items.len().div_ceil(cols);
        let keep_from = first.saturating_sub(ROW_MARGIN);
        let keep_to = (last + ROW_MARGIN).min(total_rows.saturating_sub(1));
        let mut to_load = Vec::new();
        let mut touched = std::collections::BTreeSet::new();
        for i in 0..grid.items.len() {
            let row = i / cols;
            if row >= keep_from && row <= keep_to {
                if !grid.requested.get(i).copied().unwrap_or(true) {
                    if let Some(url) = grid.items[i].thumb.clone() {
                        grid.requested[i] = true;
                        to_load.push((i, url));
                    }
                }
            } else if grid.requested.get(i).copied().unwrap_or(false) {
                grid.requested[i] = false;
                if grid.images[i].take().is_some() {
                    touched.insert(row);
                }
            }
        }
        for row in touched {
            let data = grid.row(row);
            grid.rows.set_row_data(row, data);
        }
        (grid.listing, to_load)
    };
    if to_load.is_empty() {
        return;
    }
    let thumbs = app.thumbs.clone();
    let check = app.generation.clone();
    for (index, url) in to_load {
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
