//! Biblioteca: Inicio (seguir viendo y favoritos), historial y favoritos. Todo sale del disco:
//! no hace falta red para verla.

use std::rc::Rc;

use media_lite_core::{history, Item, Kind};

use crate::listing;
use crate::state::{set_status, App};

/// Las pantallas de la biblioteca, en el orden de la barra lateral.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shelf {
    Home,
    History,
    Favorites,
    Following,
}

impl Shelf {
    pub fn from_index(i: usize) -> Option<Shelf> {
        match i {
            0 => Some(Shelf::Home),
            1 => Some(Shelf::History),
            2 => Some(Shelf::Favorites),
            3 => Some(Shelf::Following),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Shelf::Home => "Inicio",
            Shelf::History => "Historial",
            Shelf::Favorites => "Favoritos",
            Shelf::Following => "Siguiendo",
        }
    }
}

/// Elementos de una pantalla de la biblioteca.
pub fn items(app: &App, shelf: Shelf) -> Vec<Item> {
    match shelf {
        Shelf::Home => {
            let mut out = continue_watching(app);
            out.extend(app.favorites.borrow().iter().cloned());
            out
        }
        Shelf::History => app.history.borrow().iter().map(entry_item).collect(),
        Shelf::Favorites => app.favorites.borrow().clone(),
        Shelf::Following => crate::follow::shelf_items(app),
    }
}

/// Los videos empezados y no terminados, más recientes primero.
fn continue_watching(app: &App) -> Vec<Item> {
    app.history
        .borrow()
        .iter()
        .filter(|e| history::resume_point(e).is_some())
        .map(entry_item)
        .collect()
}

/// Un elemento del historial. Las entradas viejas, sin elemento guardado, usan lo que tienen.
fn entry_item(entry: &history::Entry) -> Item {
    if let Some(item) = &entry.item {
        return item.clone();
    }
    let mut item = Item::new(
        entry.platform,
        Kind::Video,
        entry.id.clone(),
        entry.title.clone(),
    );
    item.duration_secs = Some(entry.duration as u64).filter(|d| *d > 0);
    item
}

/// Abre una pantalla de la biblioteca en la grilla.
pub fn open(app: &Rc<App>, shelf: Shelf) {
    {
        let mut nav = app.nav.borrow_mut();
        nav.library = Some(shelf);
        nav.query = None;
        nav.category = None;
    }
    show(app, shelf);
    if shelf == Shelf::Following {
        crate::follow::refresh(app);
    }
}

/// Vuelve a dibujar la pantalla de la biblioteca (por ejemplo, después de cambiar un favorito).
pub fn show(app: &Rc<App>, shelf: Shelf) {
    let list = items(app, shelf);
    let count = list.len();
    let generation = app.generation.next();
    listing::show_listing(app, generation, list, Vec::new(), String::new());
    // Una pantalla de la biblioteca no espera nada de la red: no hay carga pendiente.
    crate::state::set_activity(app, crate::Activity::Idle);
    let status = match shelf {
        Shelf::Home => format!(
            "{} · {count} elementos (seguir viendo y favoritos)",
            shelf.label()
        ),
        _ => format!("{} · {count} elementos", shelf.label()),
    };
    set_status(app, status);
    if let Some(w) = app.weak.upgrade() {
        if count == 0 {
            w.set_empty_message(empty_text(shelf).into());
        }
        // Ninguna plataforma ni sección queda marcada mientras se ve la biblioteca.
        w.set_platform_index(-1);
        w.set_section_index(-1);
        w.set_library_index(match shelf {
            Shelf::Home => 0,
            Shelf::History => 1,
            Shelf::Favorites => 2,
            Shelf::Following => 3,
        });
    }
}

/// Mensaje para una pantalla vacía: dice qué hacer para que aparezca algo.
fn empty_text(shelf: Shelf) -> &'static str {
    match shelf {
        Shelf::Home => "Todavía no hay nada en «seguir viendo» ni en favoritos.",
        Shelf::History => "Todavía no viste nada. Lo que mires aparece acá.",
        Shelf::Favorites => "No tenés favoritos. Abrí un título y tocá «Agregar a favoritos».",
        Shelf::Following => {
            "No seguís canales. Abrí un canal de Twitch, Kick o YouTube y tocá «Seguir canal»."
        }
    }
}

/// Agrega o quita el elemento abierto de los favoritos.
pub fn toggle_favorite(app: &Rc<App>) {
    let item = app.grid.borrow().page_item();
    let Some(item) = item else {
        return;
    };
    if crate::follow::is_channel(&item) {
        let following = crate::follow::toggle(app, &item);
        if let Some(w) = app.weak.upgrade() {
            w.set_detail_favorite(following);
            w.set_favorite_label(channel_label(following).into());
        }
        set_status(
            app,
            if following {
                "Canal seguido: aparece en «Siguiendo»."
            } else {
                "Dejaste de seguir el canal."
            },
        );
        return;
    }
    let now_favorite = {
        let mut favorites = app.favorites.borrow_mut();
        match favorites
            .iter()
            .position(|f| f.id == item.id && f.platform == item.platform)
        {
            Some(i) => {
                favorites.remove(i);
                false
            }
            None => {
                favorites.insert(0, item.clone());
                true
            }
        }
    };
    if let Err(e) = app
        .store
        .save_list("favorites.json", &app.favorites.borrow()[..])
    {
        log::warn!("no se pudieron guardar los favoritos: {e}");
    }
    if let Some(w) = app.weak.upgrade() {
        w.set_detail_favorite(now_favorite);
    }
    set_status(
        app,
        if now_favorite {
            "Agregado a favoritos."
        } else {
            "Quitado de favoritos."
        },
    );
}

/// Texto del botón de favoritos para un canal (seguir) o para cualquier otro título.
fn channel_label(following: bool) -> &'static str {
    if following {
        "Siguiendo"
    } else {
        "Seguir canal"
    }
}

/// Pone el botón de favoritos (o de seguir, en un canal) de acuerdo con el elemento abierto.
pub fn sync_favorite_button(app: &App, item: &Item) {
    let Some(w) = app.weak.upgrade() else {
        return;
    };
    if crate::follow::is_channel(item) {
        let following = crate::follow::is_following(app, item);
        w.set_detail_favorite(following);
        w.set_favorite_label(channel_label(following).into());
    } else {
        let favorite = is_favorite(app, item);
        w.set_detail_favorite(favorite);
        w.set_favorite_label(
            if favorite {
                "En favoritos"
            } else {
                "Favoritos"
            }
            .into(),
        );
    }
}

/// Si el elemento abierto está en favoritos (para el texto del botón).
pub fn is_favorite(app: &App, item: &Item) -> bool {
    app.favorites
        .borrow()
        .iter()
        .any(|f| f.id == item.id && f.platform == item.platform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_lite_core::Platform;

    #[test]
    fn shelves_map_from_sidebar_indexes() {
        assert_eq!(Shelf::from_index(0), Some(Shelf::Home));
        assert_eq!(Shelf::from_index(2), Some(Shelf::Favorites));
        assert_eq!(Shelf::from_index(3), Some(Shelf::Following));
        assert_eq!(Shelf::from_index(4), None);
    }

    #[test]
    fn old_history_entries_still_produce_items() {
        let entry = history::Entry {
            platform: Platform::Demo,
            id: "7".into(),
            title: "Vieja".into(),
            position: 100.0,
            duration: 600.0,
            at: 0,
            item: None,
        };
        let item = entry_item(&entry);
        assert_eq!(item.id, "7");
        assert_eq!(item.duration_secs, Some(600));
    }
}
