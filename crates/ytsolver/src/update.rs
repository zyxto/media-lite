//! Actualiza los scripts de yt-dlp/ejs desde GitHub, sin recompilar. Cada archivo se verifica
//! contra el digest sha256 que publica la API del release antes de guardarse. La copia instalada
//! se usa solo si su manifiesto y sus hashes siguen coincidiendo; si no, se usan los embebidos.

use std::fs;
use std::io;
use std::path::Path;

use media_lite_core::http::{self, Agent};
use media_lite_core::store::write_atomic;
use media_lite_core::{Error, Result};
use serde_json::{json, Value};

use crate::{sha256_hex, Scripts};

pub const LATEST_API: &str = "https://api.github.com/repos/yt-dlp/ejs/releases/latest";
const LIB_NAME: &str = "yt.solver.lib.min.js";
const CORE_NAME: &str = "yt.solver.core.min.js";
const LIMIT: u64 = 4 * 1024 * 1024;

/// Un release descargado y verificado, listo para instalar.
pub struct Update {
    pub version: String,
    pub scripts: Scripts,
}

/// Baja el último release de ejs y verifica cada archivo contra el digest de su asset.
pub fn fetch_latest(agent: &Agent) -> Result<Update> {
    let api = http::get(
        agent,
        LATEST_API,
        &[("Accept", "application/vnd.github+json")],
        LIMIT,
    )?;
    let release: Value = serde_json::from_slice(&api.body)
        .map_err(|_| Error::parse("respuesta de GitHub ilegible"))?;
    let version = release
        .get("tag_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let assets = release
        .get("assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let lib = download_verified(agent, &assets, LIB_NAME)?;
    let core = download_verified(agent, &assets, CORE_NAME)?;
    Ok(Update {
        version,
        scripts: Scripts { lib, core },
    })
}

fn download_verified(agent: &Agent, assets: &[Value], name: &str) -> Result<Vec<u8>> {
    let asset = assets
        .iter()
        .find(|a| a.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| Error::parse(format!("el release no trae {name}")))?;
    let digest = asset
        .get("digest")
        .and_then(Value::as_str)
        .and_then(|d| d.strip_prefix("sha256:"))
        .ok_or_else(|| Error::parse(format!("el release no publica el hash de {name}")))?;
    let url = asset
        .get("browser_download_url")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::parse(format!("sin dirección de descarga para {name}")))?;
    let bytes = http::get_bytes(agent, url, LIMIT)?;
    if sha256_hex(&bytes) != digest {
        return Err(Error::parse(format!(
            "el hash de {name} no coincide: se descarta"
        )));
    }
    Ok(bytes)
}

/// Guarda el release en `dir` con un manifiesto (versión y hashes).
pub fn install(dir: &Path, update: &Update) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    write_atomic(&dir.join(LIB_NAME), &update.scripts.lib, 0o644)?;
    write_atomic(&dir.join(CORE_NAME), &update.scripts.core, 0o644)?;
    let manifest = json!({
        "version": update.version,
        "lib_sha256": sha256_hex(&update.scripts.lib),
        "core_sha256": sha256_hex(&update.scripts.core),
    });
    write_atomic(
        &dir.join("manifest.json"),
        manifest.to_string().as_bytes(),
        0o644,
    )
}

/// Versión instalada en `dir`, si su manifiesto y sus archivos cuadran.
pub fn load_installed(dir: &Path) -> Option<(String, Scripts)> {
    let manifest: Value =
        serde_json::from_slice(&fs::read(dir.join("manifest.json")).ok()?).ok()?;
    let lib = fs::read(dir.join(LIB_NAME)).ok()?;
    let core = fs::read(dir.join(CORE_NAME)).ok()?;
    let scripts = Scripts { lib, core };
    let lib_ok = manifest.get("lib_sha256").and_then(Value::as_str)
        == Some(sha256_hex(&scripts.lib).as_str());
    let core_ok = manifest.get("core_sha256").and_then(Value::as_str)
        == Some(sha256_hex(&scripts.core).as_str());
    if lib_ok && core_ok {
        let version = manifest
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Some((version, scripts))
    } else {
        None
    }
}

/// Los scripts a usar: la copia instalada si es válida, o la embebida.
pub fn best(dir: &Path) -> (String, Scripts) {
    match load_installed(dir) {
        Some(found) => found,
        None => (crate::EJS_VERSION.to_string(), Scripts::embedded()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_copy_is_used_only_when_hashes_match() {
        let dir = std::env::temp_dir().join(format!("media-lite-ejs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let update = Update {
            version: "9.9.9".into(),
            scripts: Scripts {
                lib: b"lib-de-prueba".to_vec(),
                core: b"core-de-prueba".to_vec(),
            },
        };
        install(&dir, &update).unwrap();
        let (version, scripts) = best(&dir);
        assert_eq!(version, "9.9.9");
        assert_eq!(scripts.lib, b"lib-de-prueba");
        // Un archivo adulterado deja de usarse: vuelven los embebidos.
        fs::write(dir.join(LIB_NAME), b"adulterado").unwrap();
        assert!(load_installed(&dir).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;

    /// Baja el último release de ejs desde GitHub (necesita red). Corre con `--ignored`.
    #[test]
    #[ignore = "necesita conexión a GitHub"]
    fn latest_release_verifies_against_api_digests() {
        let update = fetch_latest(&media_lite_core::http::agent()).expect("release verificado");
        assert!(!update.version.is_empty());
        assert!(update.scripts.lib.len() > 100_000);
        assert!(update.scripts.core.len() > 1_000);
    }
}
