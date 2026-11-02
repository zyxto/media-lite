//! Pantalla de un título (película, episodio, video…) o de una serie, al estilo de las plataformas
//! de streaming: de fondo va la imagen del título, y encima la sinopsis y los botones. Una serie
//! suma sus temporadas y sus episodios.
//!
//! El fondo es más grande que una miniatura (ver `HERO_W`), así que no sale de la grilla: se pide
//! aparte, por el pool de red, y mientras llega se ve la miniatura de la tarjeta.

use std::rc::Rc;

use media_lite_core::thumbs::Thumb;
use media_lite_core::{Item, Kind, Platform};
use slint::{Image, SharedString};

use crate::state::{with_app, App};
use crate::AppWindow;

/// Tamaño máximo del fondo. Con 960×540 llena la ventana sin pesar: son unos 2 MB de píxeles,
/// y solo mientras la pantalla está abierta.
const HERO_W: u32 = 960;
const HERO_H: u32 = 540;

/// «1 h 42 min», «42 min» o «2 h».
pub fn format_duration(secs: u64) -> String {
    let minutes = secs.div_ceil(60).max(1);
    let (hours, minutes) = (minutes / 60, minutes % 60);
    match (hours, minutes) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// El tipo de elemento, si dice algo que el subtítulo no dice ya: un episodio lleva «T1 · E3»
/// y un canal «Canal 12», así que ahí no hace falta.
fn kind_label(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Movie => Some("Película"),
        Kind::Series => Some("Serie"),
        Kind::Video => Some("Video"),
        Kind::Short => Some("Corto"),
        Kind::Track => Some("Canción"),
        Kind::Album => Some("Álbum"),
        Kind::Artist => Some("Artista"),
        Kind::Episode | Kind::Channel | Kind::Live => None,
    }
}

/// La línea de datos bajo el título: «Película · Acción · TV-14 · 1 h 42 min».
/// `seasons` es la cantidad de temporadas de una serie (0 si no es una serie).
pub fn meta_line(item: &Item, seasons: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(label) = kind_label(item.kind) {
        parts.push(label.to_string());
    }
    if !item.subtitle.is_empty() {
        parts.push(item.subtitle.clone());
    }
    if seasons > 0 {
        parts.push(if seasons == 1 {
            "1 temporada".to_string()
        } else {
            format!("{seasons} temporadas")
        });
    }
    if let Some(secs) = item.duration_secs.filter(|_| !item.live) {
        parts.push(format_duration(secs));
    }
    parts.join(" · ")
}

/// Texto del botón principal de un título.
fn play_label(item: &Item) -> String {
    if item.live {
        "Ver en vivo".to_string()
    } else if let (Some(season), Some(episode)) = (item.season, item.episode) {
        format!("Reproducir T{season} · E{episode}")
    } else {
        "Reproducir".to_string()
    }
}

/// Muestra la pantalla del título `index` de la grilla.
pub fn show_item(app: &App, index: usize) {
    let (item, placeholder) = {
        let mut grid = app.grid.borrow_mut();
        let item = grid.items.get(index).cloned();
        grid.detail = item.as_ref().map(|_| index);
        (item, grid.images.get(index).cloned().flatten())
    };
    let Some(item) = item else {
        return;
    };
    // Las fuentes de YouTube se piden al abrir el detalle: «Ver» ya las tiene cuando se aprieta.
    if item.platform == Platform::YouTube
        && matches!(item.kind, Kind::Video | Kind::Short | Kind::Live)
    {
        if let Some(provider) = app
            .providers
            .iter()
            .find(|p| p.platform() == item.platform)
            .cloned()
        {
            let warm = item.clone();
            app.pool.run(move || {
                let _ = provider.details(&warm);
            });
        }
    }
    present(
        app,
        &item,
        meta_line(&item, 0),
        play_label(&item),
        false,
        placeholder,
    );
}

/// Muestra la pantalla de la serie abierta: su encabezado y, debajo, los episodios.
pub fn show_series(app: &App) {
    let (item, seasons, first, placeholder) = {
        let mut grid = app.grid.borrow_mut();
        let placeholder = grid.placeholder.take();
        let Some(view) = &grid.series else {
            return;
        };
        (
            view.item.clone(),
            view.seasons.len(),
            grid.items.first().cloned(),
            placeholder,
        )
    };
    let label = first
        .as_ref()
        .map_or_else(|| "Reproducir".into(), play_label);
    present(
        app,
        &item,
        meta_line(&item, seasons),
        label,
        true,
        placeholder,
    );
}

/// Cambió la temporada: el botón principal reproduce el primer episodio de la nueva.
pub fn sync_series_play_label(app: &App) {
    let first = app.grid.borrow().items.first().cloned();
    if let (Some(first), Some(w)) = (first, app.weak.upgrade()) {
        w.set_detail_play_label(SharedString::from(play_label(&first)));
    }
}

fn present(
    app: &App,
    item: &Item,
    meta: String,
    play_label: String,
    series: bool,
    placeholder: Option<Image>,
) {
    let Some(w) = app.weak.upgrade() else {
        return;
    };
    w.set_detail_title(SharedString::from(item.title.as_str()));
    w.set_detail_subtitle(SharedString::from(meta));
    w.set_detail_text(SharedString::from(item.description.trim()));
    w.set_detail_has_thumb(placeholder.is_some());
    w.set_detail_thumb(placeholder.unwrap_or_default());
    w.set_detail_image(Image::default());
    w.set_detail_image_ready(false);
    crate::library::sync_favorite_button(app, item);
    w.set_detail_series(series);
    w.set_detail_play_label(SharedString::from(play_label));
    w.set_detail_open(true);
    request_hero(app, item);
}

/// Cierra la pantalla y suelta las imágenes: son lo único pesado que tiene.
pub fn hide(w: &AppWindow) {
    w.set_detail_open(false);
    w.set_detail_series(false);
    w.set_detail_image(Image::default());
    w.set_detail_thumb(Image::default());
    w.set_detail_has_thumb(false);
    w.set_detail_image_ready(false);
}

/// Cierra el título abierto y vuelve a la grilla.
pub fn close(app: &App) {
    app.grid.borrow_mut().detail = None;
    if let Some(w) = app.weak.upgrade() {
        hide(&w);
    }
}

/// Pide el fondo de `item`. Va por el pool de red, no por el de imágenes: lo pidió el usuario, y
/// no tiene que esperar atrás de las miniaturas de la grilla.
fn request_hero(app: &App, item: &Item) {
    let Some(url) = item.backdrop.clone().or_else(|| item.thumb.clone()) else {
        return;
    };
    let key = (item.platform, item.id.clone());
    let thumbs = app.thumbs.clone();
    app.pool.run(move || {
        match thumbs.get_sized(&url, HERO_W, HERO_H) {
            Ok(hero) => {
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| deliver_hero(app, &key, hero));
                });
            }
            Err(e) => log::debug!("no se pudo traer el fondo de {url}: {e}"),
        };
    });
}

/// Hilo principal: pone el fondo, si la pantalla sigue mostrando ese título.
fn deliver_hero(app: &App, key: &(Platform, String), hero: Thumb) {
    let shown = app.grid.borrow().page_item();
    if !shown.is_some_and(|item| item.platform == key.0 && item.id == key.1) {
        return;
    }
    if let Some(w) = app.weak.upgrade() {
        w.set_detail_image(crate::listing::image_from_rgba(
            hero.width,
            hero.height,
            &hero.rgba,
        ));
        w.set_detail_image_ready(true);
    }
}

/// Ya se está preparando un video. El botón principal se deshabilita en ese caso, pero la tecla
/// Enter y las filas de episodios no pasan por el botón.
fn preparing(app: &App) -> bool {
    app.weak
        .upgrade()
        .is_some_and(|w| w.get_activity() == crate::Activity::Preparing)
}

/// El botón principal: reproduce el título abierto o, en una serie, el primer episodio de la
/// temporada que se ve. La cola es la lista de la grilla, para que Anterior y Siguiente anden.
pub fn play(app: &Rc<App>) {
    if preparing(app) {
        return;
    }
    let (item, queue) = {
        let grid = app.grid.borrow();
        let item = match grid.detail {
            Some(i) => grid.items.get(i).cloned(),
            None if grid.series.is_some() => grid.items.first().cloned(),
            None => None,
        };
        (item, grid.items.clone())
    };
    if let Some(item) = item {
        crate::playback::request(app, item, queue);
    }
}

/// Un clic en un episodio de la lista de la serie: se reproduce directo.
pub fn play_episode(app: &Rc<App>, index: usize) {
    if preparing(app) {
        return;
    }
    let (item, queue) = {
        let grid = app.grid.borrow();
        (grid.items.get(index).cloned(), grid.items.clone())
    };
    if let Some(item) = item {
        crate::playback::request(app, item, queue);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(format_duration(42 * 60), "42 min");
        assert_eq!(format_duration(3600), "1 h");
        assert_eq!(format_duration(102 * 60), "1 h 42 min");
        assert_eq!(format_duration(125 * 60 + 20), "2 h 6 min");
        // Menos de un minuto no puede decir «0 min».
        assert_eq!(format_duration(20), "1 min");
    }

    #[test]
    fn movie_meta_has_kind_subtitle_and_duration() {
        let mut item = Item::new(Platform::Pluto, Kind::Movie, "1", "Peli");
        item.subtitle = "Acción · TV-14".into();
        item.duration_secs = Some(102 * 60);
        assert_eq!(
            meta_line(&item, 0),
            "Película · Acción · TV-14 · 1 h 42 min"
        );
    }

    #[test]
    fn episode_and_channel_meta_do_not_repeat_what_the_subtitle_says() {
        let mut episode = Item::new(Platform::Pluto, Kind::Episode, "1", "Ep");
        episode.subtitle = "T1 · E3".into();
        episode.duration_secs = Some(42 * 60);
        assert_eq!(meta_line(&episode, 0), "T1 · E3 · 42 min");

        let mut channel = Item::new(Platform::Pluto, Kind::Channel, "2", "Canal");
        channel.subtitle = "Canal 12".into();
        channel.live = true;
        channel.duration_secs = Some(3600);
        assert_eq!(meta_line(&channel, 0), "Canal 12");
    }

    #[test]
    fn series_meta_counts_seasons() {
        let mut series = Item::new(Platform::Plex, Kind::Series, "1", "Serie");
        series.subtitle = "2020 · Drama".into();
        assert_eq!(meta_line(&series, 1), "Serie · 2020 · Drama · 1 temporada");
        assert_eq!(meta_line(&series, 3), "Serie · 2020 · Drama · 3 temporadas");
    }

    #[test]
    fn play_label_names_the_episode_or_the_live_channel() {
        let movie = Item::new(Platform::Demo, Kind::Movie, "1", "Peli");
        assert_eq!(play_label(&movie), "Reproducir");
        let mut episode = Item::new(Platform::Demo, Kind::Episode, "2", "Ep");
        episode.season = Some(2);
        episode.episode = Some(5);
        assert_eq!(play_label(&episode), "Reproducir T2 · E5");
        let mut live = Item::new(Platform::Demo, Kind::Live, "3", "Canal");
        live.live = true;
        assert_eq!(play_label(&live), "Ver en vivo");
    }
}
