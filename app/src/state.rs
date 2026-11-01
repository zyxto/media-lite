//! Estado de la interfaz. Todo vive en el hilo principal: los hilos de trabajo nunca lo tocan,
//! solo mandan resultados con `invoke_from_event_loop`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use media_lite_core::history;
use media_lite_core::pool::{Generation, Pool};
use media_lite_core::store::{Settings, Store};
use media_lite_core::thumbs::ThumbCache;
use media_lite_core::{Item, Section};
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

/// Listado que se muestra: las tarjetas y sus miniaturas ya decodificadas.
pub struct Grid {
    pub items: Vec<Item>,
    pub images: Vec<Option<Image>>,
    pub rows: Rc<VecModel<RowData>>,
    pub columns: usize,
    /// Generación del listado que se muestra. Resultados de otras generaciones se descartan.
    pub listing: u64,
    /// Tarjeta abierta en el detalle, si hay una.
    pub detail: Option<usize>,
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
}

/// Lo que el usuario eligió: plataforma, sección y búsqueda, si hay una.
#[derive(Clone)]
pub struct Nav {
    pub platform: usize,
    pub section: usize,
    pub query: Option<String>,
}

/// Estado compartido de la aplicación.
pub struct App {
    pub weak: Weak<AppWindow>,
    pub providers: Vec<Arc<dyn Provider>>,
    pub pool: Pool,
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

/// Nombres para el menú de plataformas y secciones, para inicializar la ventana.
pub fn section_labels() -> ModelRc<SharedString> {
    let names: Vec<SharedString> = Section::ALL
        .iter()
        .map(|s| SharedString::from(s.label()))
        .collect();
    ModelRc::new(VecModel::from(names))
}
