//! Listados, búsqueda y miniaturas. Lo pesado va en el pool; la grilla se actualiza en el hilo principal.

use std::rc::Rc;

use media_lite_core::thumbs::Thumb;
use media_lite_core::{Item, Section};
use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, SharedString};

use crate::state::{set_status, with_app, App};

/// Pide el listado, o la búsqueda, de la navegación actual.
pub fn load(app: &Rc<App>) {
    let (provider_index, section, query) = {
        let nav = app.nav.borrow();
        (nav.platform, Section::ALL[nav.section], nav.query.clone())
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
                let check = shared_gen.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| {
                        if check.current() == generation {
                            show_listing(app, generation, items, status);
                        }
                    });
                });
                for (index, url) in urls {
                    let thumbs = thumbs.clone();
                    let check = shared_gen.clone();
                    pool.run(move || {
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
                            show_listing(app, generation, Vec::new(), message);
                        }
                    });
                });
            }
        }
    });
}

/// Hilo principal: muestra el listado nuevo y descarta las miniaturas que quedaron de antes.
pub fn show_listing(app: &Rc<App>, generation: u64, items: Vec<Item>, status: String) {
    let first = items.first().cloned();
    {
        let mut grid = app.grid.borrow_mut();
        grid.listing = generation;
        grid.images = vec![None; items.len()];
        grid.items = items;
        grid.detail = None;
        grid.rebuild();
    }
    if let Some(w) = app.weak.upgrade() {
        w.set_busy(false);
        w.set_detail_open(false);
    }
    set_status(app, status);
    // Gancho de desarrollo (prueba automática): abre el primer elemento del primer listado.
    if generation == 1 && std::env::var_os("MEDIA_LITE_AUTOPLAY").is_some() {
        if let Some(item) = first {
            crate::playback::request(app, item);
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
    w.set_detail_text(SharedString::from(
        "Elemento de prueba. Reproduce un stream HLS público de Apple, para probar el reproductor.",
    ));
    w.set_detail_open(true);
}

/// Cierra el detalle y vuelve a la grilla.
pub fn close_detail(app: &App) {
    app.grid.borrow_mut().detail = None;
    if let Some(w) = app.weak.upgrade() {
        w.set_detail_open(false);
    }
}

/// Columnas que caben para un ancho de ventana dado (en píxeles lógicos).
pub fn columns_for(width_px: f32) -> usize {
    let available = width_px - crate::state::SIDEBAR - crate::state::GUTTER;
    ((available + crate::state::GAP) / (crate::state::CARD_MIN_WIDTH + crate::state::GAP))
        .floor()
        .clamp(2.0, 8.0) as usize
}
