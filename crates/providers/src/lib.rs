//! Proveedores: cada plataforma implementa `Provider`, y la interfaz solo ve este trait.
//!
//! Los proveedores son bloqueantes y se llaman desde hilos de trabajo, nunca desde la interfaz.

pub mod demo;

use std::sync::Arc;

use media_lite_core::{Caps, Category, Details, Error, Item, Page, Platform, Result, Section};

pub trait Provider: Send + Sync {
    fn platform(&self) -> Platform;

    fn caps(&self) -> Caps;

    /// Subcategorías de una sección (Acción, Comedia…). Vacío si no hay.
    fn categories(&self, _section: Section) -> Result<Vec<Category>> {
        Ok(Vec::new())
    }

    /// Una página de una sección. `cursor` viene de `Page::next` de la página anterior.
    fn list(&self, section: Section, category: Option<&str>, cursor: Option<&str>) -> Result<Page>;

    fn search(&self, _query: &str, _cursor: Option<&str>) -> Result<Page> {
        Err(Error::unsupported())
    }

    fn details(&self, _item: &Item) -> Result<Details> {
        Err(Error::unsupported())
    }
}

/// Proveedores activos, en el orden de la barra lateral.
pub fn registry() -> Vec<Arc<dyn Provider>> {
    vec![Arc::new(demo::Demo::new())]
}

/// Busca un proveedor por plataforma.
pub fn find(providers: &[Arc<dyn Provider>], platform: Platform) -> Option<Arc<dyn Provider>> {
    providers.iter().find(|p| p.platform() == platform).cloned()
}
