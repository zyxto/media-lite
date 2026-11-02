//! Proveedores: cada plataforma implementa `Provider`, y la interfaz solo ve este trait.
//!
//! Los proveedores son bloqueantes y se llaman desde hilos de trabajo, nunca desde la interfaz.

pub mod demo;
pub mod kick;
pub mod lyrics;
pub mod plex;
pub mod pluto;
pub mod runtime;
pub mod text;
pub mod twitch;
pub mod util;
pub mod youtube;
pub mod ytmusic;

use std::path::Path;
use std::sync::Arc;

use media_lite_core::{
    Caps, Category, Details, Error, Item, Page, Platform, Result, Season, Section,
};

pub trait Provider: Send + Sync {
    fn platform(&self) -> Platform;

    fn caps(&self) -> Caps;

    /// Subcategorías de una sección (Acción, Comedia, Canales de noticias…). Vacío si no hay.
    fn categories(&self, _section: Section) -> Result<Vec<Category>> {
        Ok(Vec::new())
    }

    /// Una página de una sección. `category` es el id de la subcategoría, o `None` para todas.
    /// `cursor` viene de `Page::next` de la página anterior.
    fn list(&self, section: Section, category: Option<&str>, cursor: Option<&str>) -> Result<Page>;

    fn search(&self, _query: &str, _cursor: Option<&str>) -> Result<Page> {
        Err(Error::unsupported())
    }

    /// Detalle y fuentes reproducibles de un elemento.
    fn details(&self, _item: &Item) -> Result<Details> {
        Err(Error::unsupported())
    }

    /// Temporadas y episodios de una serie.
    fn seasons(&self, _series: &Item) -> Result<Vec<Season>> {
        Err(Error::unsupported())
    }

    /// Lo nuevo de estos canales seguidos: los que están en vivo y, si la plataforma lo ofrece, los
    /// últimos videos. Se llama en segundo plano desde «Siguiendo».
    fn feed(&self, _channels: &[Item]) -> Result<Vec<Item>> {
        Err(Error::unsupported())
    }

    /// Se llama en segundo plano al arrancar la app, para dejar listo lo lento (por ejemplo, el
    /// reproductor de YouTube). No devuelve nada: los errores quedan en el registro.
    fn warm(&self) {}

    /// Olvida lo guardado de un elemento (una URL que venció). La próxima vez se vuelve a pedir.
    fn invalidate(&self, _item: &Item) {}

    /// La radio de una canción: canciones parecidas para seguir sonando al terminar la cola.
    fn radio(&self, _item: &Item) -> Result<Vec<Item>> {
        Err(Error::unsupported())
    }
}

/// Proveedores activos, en el orden de la barra lateral.
///
/// `config_dir` es donde se guardan los identificadores de sesión (permisos 0600 cuando hay token).
/// `plex_english` incluye en Plex lo que está en inglés.
pub fn registry(config_dir: &Path, plex_english: bool) -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(pluto::Pluto::new(config_dir)),
        Arc::new(runtime::Runtime::new()),
        Arc::new(plex::Plex::new(config_dir, plex_english)),
        Arc::new(twitch::Twitch::new()),
        Arc::new(youtube::YouTube::new(config_dir)),
        Arc::new(ytmusic::YouTubeMusic::new(config_dir)),
        Arc::new(kick::Kick::new()),
        Arc::new(demo::Demo::new()),
    ]
}

/// Busca un proveedor por plataforma.
pub fn find(providers: &[Arc<dyn Provider>], platform: Platform) -> Option<Arc<dyn Provider>> {
    providers.iter().find(|p| p.platform() == platform).cloned()
}
