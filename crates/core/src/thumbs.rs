//! Caché de miniaturas: memoria (LRU con tope en bytes) y disco.
//!
//! La memoria guarda la miniatura ya reducida y **comprimida en PNG**, no los píxeles: un PNG de
//! 256×144 pesa unos pocos KB, y el RGBA de la misma imagen unos 147 KB. Los píxeles solo viven
//! mientras la interfaz los muestra. Así la caché de 24 MB alcanza para cientos de miniaturas.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use base64::Engine;
use lru::LruCache;
use sha2::{Digest, Sha256};

use crate::error::{Error, ErrorKind, Result};
use crate::http::{self, Agent};

/// Tamaño máximo de una imagen descargada. Las miniaturas pesan decenas de KB.
const MAX_DOWNLOAD: u64 = 4 * 1024 * 1024;

/// Una miniatura lista para mostrar: píxeles RGBA8 sin relleno.
#[derive(Clone, Debug)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct Memory {
    /// URL → PNG reducido.
    lru: LruCache<String, Arc<Vec<u8>>>,
    bytes: usize,
    budget: usize,
}

pub struct ThumbCache {
    memory: Mutex<Memory>,
    disk_dir: Option<PathBuf>,
    agent: Agent,
    max_w: u32,
    max_h: u32,
}

impl ThumbCache {
    /// `budget_bytes`: tope de la caché en memoria. `disk_dir`: carpeta de la caché en disco (opcional).
    pub fn new(
        agent: Agent,
        disk_dir: Option<PathBuf>,
        budget_bytes: usize,
        max_w: u32,
        max_h: u32,
    ) -> ThumbCache {
        ThumbCache {
            memory: Mutex::new(Memory {
                lru: LruCache::unbounded(),
                bytes: 0,
                budget: budget_bytes,
            }),
            disk_dir,
            agent,
            max_w,
            max_h,
        }
    }

    /// Devuelve la miniatura de `url`, de la memoria, del disco o de la red, en ese orden.
    /// Puede bloquear: llamar desde un hilo de trabajo, no desde la interfaz.
    pub fn get(&self, url: &str) -> Result<Thumb> {
        if let Some(png) = self.memory_get(url) {
            return decode(&png, self.max_w, self.max_h);
        }
        let disk_path = self.disk_path(url);
        if let Some(png) = disk_path.as_ref().and_then(|p| fs::read(p).ok()) {
            if let Ok(thumb) = decode(&png, self.max_w, self.max_h) {
                self.memory_put(url, png);
                return Ok(thumb);
            }
        }
        let raw = fetch(&self.agent, url)?;
        let thumb = decode(&raw, self.max_w, self.max_h)?;
        if let Some(png) = encode_png(&thumb) {
            if let Some(path) = disk_path {
                if let Err(e) = crate::store::write_atomic(&path, &png, 0o644) {
                    log::debug!("no se pudo guardar la miniatura en disco: {e}");
                }
            }
            self.memory_put(url, png);
        }
        Ok(thumb)
    }

    fn memory_get(&self, url: &str) -> Option<Arc<Vec<u8>>> {
        let mut mem = self.memory.lock().ok()?;
        mem.lru.get(url).cloned()
    }

    fn memory_put(&self, url: &str, png: Vec<u8>) {
        if let Ok(mut mem) = self.memory.lock() {
            let size = png.len();
            if let Some(old) = mem.lru.put(url.to_string(), Arc::new(png)) {
                mem.bytes -= old.len();
            }
            mem.bytes += size;
            while mem.bytes > mem.budget {
                match mem.lru.pop_lru() {
                    Some((_, evicted)) => mem.bytes -= evicted.len(),
                    None => break,
                }
            }
        }
    }

    fn disk_path(&self, url: &str) -> Option<PathBuf> {
        let dir = self.disk_dir.as_ref()?;
        let digest = Sha256::digest(url.as_bytes());
        let name: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        Some(dir.join(format!("{name}.png")))
    }

    /// Bytes que ocupan las miniaturas comprimidas en memoria.
    pub fn memory_bytes(&self) -> usize {
        self.memory.lock().map(|m| m.bytes).unwrap_or(0)
    }
}

fn fetch(agent: &Agent, url: &str) -> Result<Vec<u8>> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, payload) = rest
            .split_once(',')
            .ok_or_else(|| Error::parse("data: sin contenido"))?;
        return if meta.ends_with(";base64") {
            base64::engine::general_purpose::STANDARD
                .decode(payload)
                .map_err(|e| Error::parse(format!("base64 inválido: {e}")))
        } else {
            Ok(payload.as_bytes().to_vec())
        };
    }
    if let Some(path) = url.strip_prefix("file://") {
        return fs::read(path).map_err(|e| Error::new(ErrorKind::Other, format!("{path}: {e}")));
    }
    http::get_bytes(agent, url, MAX_DOWNLOAD)
}

/// Decodifica y reduce la imagen dentro de `max_w` × `max_h`, conservando la proporción.
/// Solo se achica: una imagen que ya cabe no se agranda.
pub fn decode(bytes: &[u8], max_w: u32, max_h: u32) -> Result<Thumb> {
    let image = image::load_from_memory(bytes)
        .map_err(|e| Error::parse(format!("imagen no válida: {e}")))?;
    let small = if image.width() <= max_w && image.height() <= max_h {
        image.to_rgba8()
    } else {
        image.thumbnail(max_w, max_h).to_rgba8()
    };
    let (width, height) = small.dimensions();
    Ok(Thumb {
        width,
        height,
        rgba: small.into_raw(),
    })
}

fn encode_png(thumb: &Thumb) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            &thumb.rgba,
            thumb.width,
            thumb.height,
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(out)
}

/// Borra las miniaturas más viejas hasta quedar debajo de `max_bytes`.
pub fn prune_disk(dir: &Path, max_bytes: u64) -> std::io::Result<u64> {
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            files.push((
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                meta.len(),
                entry.path(),
            ));
        }
    }
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.0);
    let mut removed = 0;
    for (_, size, path) in files {
        if total <= max_bytes {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= size;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, 128, 255]));
        encode_png(&Thumb {
            width: w,
            height: h,
            rgba: img.into_raw(),
        })
        .unwrap()
    }

    fn data_url(png: &[u8]) -> String {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        )
    }

    #[test]
    fn decode_shrinks_within_bounds_keeping_aspect() {
        let thumb = decode(&tiny_png(640, 360), 320, 180).unwrap();
        assert_eq!((thumb.width, thumb.height), (320, 180));
        assert_eq!(thumb.rgba.len(), 320 * 180 * 4);
    }

    #[test]
    fn small_images_are_not_enlarged() {
        let thumb = decode(&tiny_png(64, 36), 320, 180).unwrap();
        assert_eq!((thumb.width, thumb.height), (64, 36));
    }

    #[test]
    fn data_urls_are_served_without_network_and_kept_compressed() {
        let cache = ThumbCache::new(http::agent(), None, 1 << 20, 320, 180);
        let thumb = cache.get(&data_url(&tiny_png(640, 360))).unwrap();
        assert_eq!((thumb.width, thumb.height), (320, 180));
        let compressed = cache.memory_bytes();
        assert!(compressed > 0);
        assert!(
            compressed < thumb.rgba.len(),
            "la memoria guarda el PNG, no los píxeles"
        );
    }

    #[test]
    fn memory_budget_evicts_old_entries() {
        let cache = ThumbCache::new(http::agent(), None, 2000, 320, 180);
        for i in 0..50 {
            cache.get(&data_url(&tiny_png(64 + i, 36))).unwrap();
        }
        assert!(cache.memory_bytes() <= 2000);
    }
}
