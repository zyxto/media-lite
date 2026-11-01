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
use crate::{AppWindow, CardData, RowData};

/// Ancho mínimo de una tarjeta; define cuántas columnas caben (la misma cuenta que en Slint).
pub const CARD_MIN_WIDTH: f32 = 240.0;
pub const GAP: f32 = 16.0;
pub const SIDEBAR: f32 = 200.0;
pub const GUTTER: f32 = 40.0;

/// Vista de una serie: sus temporadas, y la que se muestra en la grilla.
pub struct SeriesView {
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
    /// Miniaturas ya pedidas, para no volver a pedirlas al desplazarse.
    pub requested: Vec<bool>,
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
            requested: Vec::new(),
        }
    }

    pub fn rebuild(&mut self) {
        let count = self.items.len().div_ceil(self.columns);
        let rows: Vec<RowData> = (0..count).map(|r| self.row(r)).collect();
        self.rows.set_vec(rows);
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

/// Lo que el usuario eligió: plataforma, sección, categoría y búsqueda, si hay una.
#[derive(Clone)]
pub struct Nav {
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
