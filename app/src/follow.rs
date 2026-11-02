//! Canales seguidos: Twitch, Kick y YouTube. Un canal se sigue desde su pantalla («Seguir canal»),
//! y «Siguiendo» muestra lo que hay en vivo y los videos nuevos, que se piden en segundo plano.

use std::rc::Rc;
use std::sync::Arc;

use media_lite_core::{Item, Kind, Platform};

use crate::library::{self, Shelf};
use crate::state::{with_app, App};

/// Cuántos canales se guardan.
const MAX_FOLLOWING: usize = 200;

/// Un canal de una plataforma que tiene canales (Twitch, Kick y YouTube), como serie.
pub fn is_channel(item: &Item) -> bool {
    item.kind == Kind::Series
        && matches!(
            item.platform,
            Platform::Twitch | Platform::Kick | Platform::YouTube
        )
}

pub fn is_following(app: &App, item: &Item) -> bool {
    app.following
        .borrow()
        .iter()
        .any(|f| f.id == item.id && f.platform == item.platform)
}

/// Sigue o deja de seguir un canal. Devuelve si ahora se sigue.
pub fn toggle(app: &Rc<App>, item: &Item) -> bool {
    let now_following = {
        let mut list = app.following.borrow_mut();
        match list
            .iter()
            .position(|f| f.id == item.id && f.platform == item.platform)
        {
            Some(i) => {
                list.remove(i);
                false
            }
            None => {
                list.insert(0, item.clone());
                list.truncate(MAX_FOLLOWING);
                true
            }
        }
    };
    if let Err(e) = app
        .store
        .save_list("following.json", &app.following.borrow()[..])
    {
        log::warn!("no se pudieron guardar los canales seguidos: {e}");
    }
    now_following
}

/// Pide lo nuevo de los canales seguidos, plataforma por plataforma, en segundo plano.
pub fn refresh(app: &Rc<App>) {
    let channels = app.following.borrow().clone();
    if channels.is_empty() {
        return;
    }
    for provider in app.providers.iter() {
        let mine: Vec<Item> = channels
            .iter()
            .filter(|c| c.platform == provider.platform())
            .cloned()
            .collect();
        if mine.is_empty() {
            continue;
        }
        let platform = provider.platform();
        let provider = Arc::clone(provider);
        app.pool.run(move || {
            let items = match provider.feed(&mine) {
                Ok(items) => items,
                Err(e) => {
                    log::info!("siguiendo en {}: {e}", platform.label());
                    Vec::new()
                }
            };
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| deliver(app, platform, items));
            });
        });
    }
}

/// Llegó lo nuevo de una plataforma: reemplaza lo que había de esa plataforma y, si la biblioteca
/// está en «Siguiendo», la vuelve a dibujar.
fn deliver(app: &Rc<App>, platform: Platform, items: Vec<Item>) {
    {
        let mut feed = app.feed.borrow_mut();
        feed.retain(|i| i.platform != platform);
        feed.extend(items);
    }
    let on_following = app.nav.borrow().library == Some(Shelf::Following);
    if on_following {
        library::show(app, Shelf::Following);
    }
}

/// Lo que se muestra en «Siguiendo»: primero lo que hay en vivo o es nuevo, después los canales.
pub fn shelf_items(app: &App) -> Vec<Item> {
    let mut out = app.feed.borrow().clone();
    out.extend(app.following.borrow().iter().cloned());
    out
}
