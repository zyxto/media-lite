//! Los clientes de innertube (nombre, versión, user agent) desde `clients.toml`. El del AppImage
//! viene incluido; si existe `~/.config/media-lite/clients.toml`, sus bloques reemplazan a los del
//! mismo nombre.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

const DEFAULTS: &str = include_str!("../../clients.toml");
const OVERRIDE_FILE: &str = "clients.toml";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Client {
    /// Valor de `context.client.clientName`.
    pub name: String,
    /// Valor de la cabecera `X-YouTube-Client-Name`.
    pub number: u32,
    /// Valor de `context.client.clientVersion`.
    pub version: String,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub device_make: Option<String>,
    #[serde(default)]
    pub device_model: Option<String>,
    #[serde(default)]
    pub os_name: Option<String>,
    #[serde(default)]
    pub os_version: Option<String>,
    /// Si la respuesta de `/player` necesita el solucionador de JavaScript (`sig` y `n`).
    #[serde(default)]
    pub needs_js: bool,
    /// Solo `web_embedded`: la página que se declara como origen del video incrustado.
    #[serde(default)]
    pub embed_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clients {
    by_name: BTreeMap<String, Client>,
}

impl Clients {
    /// Los clientes del AppImage, con los reemplazos de `config_dir/clients.toml` si los hay.
    /// Un archivo que no se entiende se ignora (se avisa en el registro).
    pub fn load(config_dir: &Path) -> Clients {
        let mut by_name: BTreeMap<String, Client> =
            toml::from_str(DEFAULTS).expect("clients.toml incluido en el binario es válido");
        let path = config_dir.join(OVERRIDE_FILE);
        if let Ok(text) = fs::read_to_string(&path) {
            match toml::from_str::<BTreeMap<String, Client>>(&text) {
                Ok(overrides) => by_name.extend(overrides),
                Err(e) => log::warn!("clients.toml de la configuración ignorado: {e}"),
            }
        }
        Clients { by_name }
    }

    /// Un cliente por su clave (`web`, `visionos`, `tv_samsung`…).
    pub fn get(&self, key: &str) -> Option<&Client> {
        self.by_name.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_include_the_web_client() {
        let clients = Clients {
            by_name: toml::from_str(DEFAULTS).unwrap(),
        };
        let web = clients.get("web").expect("cliente web");
        assert_eq!(web.name, "WEB");
        assert_eq!(web.number, 1);
        assert_eq!(web.version, "2.20260708.00.00");
    }

    #[test]
    fn override_replaces_only_its_own_block() {
        let dir = std::env::temp_dir().join(format!("ml-clients-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(OVERRIDE_FILE),
            "[web]\nname = \"WEB\"\nnumber = 1\nversion = \"2.99990101.00.00\"\n",
        )
        .unwrap();
        let clients = Clients::load(&dir);
        assert_eq!(clients.get("web").unwrap().version, "2.99990101.00.00");
        fs::remove_dir_all(&dir).unwrap();
    }
}
