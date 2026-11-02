//! Estado de la interfaz. Todo vive en el hilo principal: los hilos de trabajo nunca lo tocan,
//! solo mandan resultados con `invoke_from_event_loop`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use media_lite_core::history;
use media_lite_core::pool::{Generation, Pool};
use media_lite_core::store::{Settings, Store};
use media_lite_core::thumbs::ThumbCache;
use media_lite_core::{Category, Item, Season, Section};
use media_lite_player::{Player, Proxy};
use media_lite_providers::Provider;
use slint::{Image, ModelRc, SharedString, VecModel, Weak};

use crate::playback::Playback;
use crate::{Activity, AppWindow, CardData, EpisodeData, RowData};

/// Ancho mínimo de una tarjeta; define cuántas columnas caben (la misma cuenta que en Slint).
pub const CARD_MIN_WIDTH: f32 = 240.0;
pub const GAP: f32 = 16.0;
pub const SIDEBAR: f32 = 200.0;
pub const GUTTER: f32 = 40.0;

/// Vista de una serie: sus temporadas, y la que se muestra en la lista de episodios.
pub struct SeriesView {
    /// La serie misma: es lo que muestra el encabezado de su pantalla.
    pub item: Item,
    pub seasons: Vec<Season>,
    pub season: usize,
}

/// Lo que se muestra en la grilla: las tarjetas, sus miniaturas y los chips de categorías.
pub struct Grid {
    pub items: Vec<Item>,
    pub images: Vec<Option<Image>>,
    pub rows: Rc<VecModel<RowData>>,
    pub columns: usize,
    /// Generación del listado que se muestra. Resultados de otras generaciones se descartan.
    pub listing: u64,
    /// Tarjeta abierta en el detalle, si hay una.
    pub detail: Option<usize>,
    /// Categorías del listado actual. El chip 0 es «Todas» (o la temporada, en una serie).
    pub categories: Vec<Category>,
    pub category: usize,
    /// Serie abierta, si la grilla muestra episodios.
    pub series: Option<SeriesView>,
    /// Los episodios de la temporada elegida, tal como los dibuja la pantalla de la serie.
    pub episodes: Rc<VecModel<EpisodeData>>,
    /// Miniatura de la serie que se está abriendo: se ve mientras llega su imagen grande.
    pub placeholder: Option<Image>,
    /// Miniaturas ya pedidas, para no volver a pedirlas al desplazarse.
    pub requested: Vec<bool>,
    /// Búsquedas de plataformas que todavía no respondieron (búsqueda global).
    pub pending: usize,
}

impl Grid {
    pub fn new(columns: usize) -> Grid {
        Grid {
            items: Vec::new(),
            images: Vec::new(),
            rows: Rc::new(VecModel::default()),
            columns,
            listing: 0,
            detail: None,
            categories: Vec::new(),
            category: 0,
            series: None,
            episodes: Rc::new(VecModel::default()),
            placeholder: None,
            requested: Vec::new(),
            pending: 0,
        }
    }

    pub fn rebuild(&mut self) {
        let count = self.items.len().div_ceil(self.columns);
        let rows: Vec<RowData> = (0..count).map(|r| self.row(r)).collect();
        self.rows.set_vec(rows);
        let episodes: Vec<EpisodeData> = match self.series {
            Some(_) => (0..self.items.len()).map(|i| self.episode(i)).collect(),
            None => Vec::new(),
        };
        self.episodes.set_vec(episodes);
    }

    /// El elemento que muestra la pantalla de detalle: el título abierto o, en una serie, la serie.
    pub fn page_item(&self) -> Option<Item> {
        match self.detail {
            Some(i) => self.items.get(i).cloned(),
            None => self.series.as_ref().map(|view| view.item.clone()),
        }
    }

    /// El episodio `i` de la lista de la serie.
    pub fn episode(&self, i: usize) -> EpisodeData {
        let item = &self.items[i];
        let image = self.images.get(i).cloned().flatten();
        // Algunas plataformas repiten la sinopsis de la serie en cada episodio: no sirve de nada.
        let repeats_series = self
            .series
            .as_ref()
            .is_some_and(|view| same_text(&view.item.description, &item.description));
        EpisodeData {
            number: item
                .episode
                .map_or_else(|| (i + 1).to_string(), |n| n.to_string())
                .into(),
            title: SharedString::from(item.title.as_str()),
            meta: item
                .duration_secs
                .map(crate::detail::format_duration)
                .unwrap_or_default()
                .into(),
            description: if repeats_series {
                SharedString::new()
            } else {
                SharedString::from(item.description.trim())
            },
            thumb: image.clone().unwrap_or_default(),
            has_thumb: image.is_some(),
            index: i as i32,
        }
    }

    pub fn row(&self, r: usize) -> RowData {
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

    /// Nombres de los chips: «Todas» más las categorías, o las temporadas de una serie.
    pub fn chip_names(&self) -> Vec<SharedString> {
        match &self.series {
            Some(view) => view
                .seasons
                .iter()
                .map(|s| SharedString::from(s.name.as_str()))
                .collect(),
            // Sin categorías (búsqueda, biblioteca): no hay chips.
            None if self.categories.is_empty() => Vec::new(),
            None => std::iter::once(SharedString::from("Todas"))
                .chain(
                    self.categories
                        .iter()
                        .map(|c| SharedString::from(c.title.as_str())),
                )
                .collect(),
        }
    }
}

/// Si dos textos dicen lo mismo, sin contar los espacios ni los saltos de línea.
fn same_text(a: &str, b: &str) -> bool {
    a.split_whitespace().eq(b.split_whitespace())
}

/// Lo que el usuario eligió: plataforma, sección, categoría y búsqueda, si hay una.
#[derive(Clone)]
pub struct Nav {
    /// Búsqueda en todas las plataformas a la vez.
    pub global: bool,
    /// Pantalla de la biblioteca (Inicio, historial, favoritos), si está abierta.
    pub library: Option<crate::library::Shelf>,
    pub platform: usize,
    pub section: usize,
    /// Id de la categoría elegida en el chip (`None`: todas).
    pub category: Option<String>,
    pub query: Option<String>,
}

/// Estado compartido de la aplicación.
pub struct App {
    pub weak: Weak<AppWindow>,
    pub providers: Vec<Arc<dyn Provider>>,
    /// Red: listados, detalles y series. Dos hilos, para que un pedido del usuario no espere a las imágenes.
    pub pool: Pool,
    /// Miniaturas: un hilo, para que no compitan con los pedidos de red.
    pub images: Pool,
    pub thumbs: Arc<ThumbCache>,
    pub generation: Arc<Generation>,
    pub store: Store,
    pub settings: RefCell<Settings>,
    pub nav: RefCell<Nav>,
    /// Proxy de playlists. `None` si no se pudo abrir el puerto local.
    pub proxy: Option<Arc<Proxy>>,
    pub player: RefCell<Option<Player>>,
    pub playback: RefCell<Option<Playback>>,
    pub history: RefCell<Vec<history::Entry>>,
    pub favorites: RefCell<Vec<Item>>,
    pub grid: RefCell<Grid>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// Registra la app del hilo principal, para que los resultados de los hilos de trabajo la encuentren.
pub fn install(app: Rc<App>) {
    APP.with(|cell| *cell.borrow_mut() = Some(app));
}

/// Ejecuta `f` con la app, si existe. Solo desde el hilo principal.
pub fn with_app<R>(f: impl FnOnce(&Rc<App>) -> R) -> Option<R> {
    APP.with(|cell| cell.borrow().as_ref().map(f))
}

/// Pone el texto de la barra de estado.
pub fn set_status(app: &App, text: impl Into<SharedString>) {
    if let Some(w) = app.weak.upgrade() {
        w.set_status(text.into());
        w.set_status_error(false);
    }
}

/// Pone un error en la barra de estado, en color de alerta, hasta el próximo mensaje.
pub fn set_error(app: &App, text: impl Into<SharedString>) {
    if let Some(w) = app.weak.upgrade() {
        w.set_status(text.into());
        w.set_status_error(true);
    }
}

/// Lugares del esqueleto de carga: uno por columna de la grilla. Se llama al cambiar el ancho.
pub fn sync_skeleton(app: &App) {
    let columns = app.grid.borrow().columns;
    if let Some(w) = app.weak.upgrade() {
        let slots: Vec<i32> = (0..columns as i32).collect();
        w.set_skeleton_slots(ModelRc::new(VecModel::from(slots)));
    }
}

/// Lo que la app está cargando: muestra la barra de progreso y, si es un catálogo, el esqueleto.
pub fn set_activity(app: &App, activity: Activity) {
    if let Some(w) = app.weak.upgrade() {
        w.set_activity(activity);
    }
}

/// Marca en el menú la plataforma y la sección de la navegación actual. Así la marca sigue al
/// clic también al volver de la biblioteca, que desmarca ambas.
pub fn sync_nav(app: &App) {
    let (platform, section) = {
        let nav = app.nav.borrow();
        // «Todas las plataformas» es la última entrada del menú (ver `main.rs`).
        let platform = if nav.global {
            app.providers.len()
        } else {
            nav.platform
        };
        (platform, nav.section)
    };
    if let Some(w) = app.weak.upgrade() {
        w.set_platform_index(platform as i32);
        w.set_section_index(section as i32);
    }
}

/// Actualiza los chips de la barra con los de la grilla actual.
pub fn sync_chips(app: &App) {
    let grid = app.grid.borrow();
    if let Some(w) = app.weak.upgrade() {
        w.set_chips(ModelRc::new(VecModel::from(grid.chip_names())));
        w.set_chip_index(grid.category as i32);
    }
}

/// Nombres para el menú de secciones, para inicializar la ventana.
pub fn section_labels() -> ModelRc<SharedString> {
    let names: Vec<SharedString> = Section::ALL
        .iter()
        .map(|s| SharedString::from(s.label()))
        .collect();
    ModelRc::new(VecModel::from(names))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_text_ignores_line_breaks_and_extra_spaces() {
        assert!(same_text("Uno.\n¿Dos?", "Uno.  ¿Dos? "));
        assert!(!same_text("Uno.", "Otra cosa."));
        assert!(same_text("", "  "));
    }
}
