//! Núcleo de media-lite: modelo común, errores, ajustes y listas en disco, pool de hilos,
//! HTTP y caché de miniaturas. No sabe nada de plataformas ni de la interfaz.

pub mod error;
pub mod history;
pub mod http;
pub mod model;
pub mod paths;
pub mod pool;
pub mod store;
pub mod thumbs;

pub use error::{Error, ErrorKind, Result};
pub use model::{Caps, Category, Details, Item, Kind, Page, Platform, Section, Shelf, Source};
