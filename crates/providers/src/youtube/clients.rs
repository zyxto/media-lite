//! Los clientes de innertube (nombre, versión, user agent) y la cascada de reproducción, desde
//! `clients.toml`. El del AppImage viene incluido; si existe `~/.config/media-lite/clients.toml`,
//! sus bloques reemplazan a los del mismo nombre, y `[cascade]` reemplaza el orden.

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

#[derive(Deserialize, Default)]
struct Cascade {
    order: Vec<String>,
    #[serde(default)]
    cookies_order: Vec<String>,
    #[serde(default = "default_hedge")]
    hedge_ms: u64,
}

fn default_hedge() -> u64 {
    2000
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clients {
    by_name: BTreeMap<String, Client>,
    cascade: CascadeOrder,
}

/// El orden de la cascada y cuánto esperar antes de probar el siguiente cliente.
#[derive(Clone, Debug, PartialEq)]
pub struct CascadeOrder {
    pub order: Vec<String>,
    pub cookies_order: Vec<String>,
    pub hedge_ms: u64,
}

impl Clients {
    /// Los clientes del AppImage, con los reemplazos de `config_dir/clients.toml` si los hay.
    /// Un archivo que no se entiende se ignora (se avisa en el registro).
    pub fn load(config_dir: &Path) -> Clients {
        let mut clients =
            Clients::parse(DEFAULTS).expect("clients.toml incluido en el binario es válido");
        let path = config_dir.join(OVERRIDE_FILE);
        if let Ok(text) = fs::read_to_string(&path) {
            match Clients::parse(&text) {
                Ok(overrides) => {
                    clients.by_name.extend(overrides.by_name);
                    if !overrides.cascade.order.is_empty() {
                        clients.cascade = overrides.cascade;
                    }
                }
                Err(e) => log::warn!("clients.toml de la configuración ignorado: {e}"),
            }
        }
        clients
    }

    /// Lee un `clients.toml`: `[cascade]` y un bloque por cliente.
    fn parse(text: &str) -> Result<Clients, String> {
        let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut by_name = BTreeMap::new();
        let mut cascade = CascadeOrder {
            order: Vec::new(),
            cookies_order: Vec::new(),
            hedge_ms: default_hedge(),
        };
        for (key, value) in table {
            if key == "cascade" {
                let parsed: Cascade = value
                    .try_into()
                    .map_err(|e: toml::de::Error| e.to_string())?;
                cascade = CascadeOrder {
                    order: parsed.order,
                    cookies_order: parsed.cookies_order,
                    hedge_ms: parsed.hedge_ms,
                };
            } else {
                let client: Client = value
                    .try_into()
                    .map_err(|e: toml::de::Error| format!("[{key}]: {e}"))?;
                by_name.insert(key, client);
            }
        }
        Ok(Clients { by_name, cascade })
    }

    /// Un cliente por su clave (`web`, `visionos`, `tv_samsung`…).
    pub fn get(&self, key: &str) -> Option<&Client> {
        self.by_name.get(key)
    }

    pub fn cascade(&self) -> &CascadeOrder {
        &self.cascade
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_the_catalog_and_playback_clients() {
        let clients = Clients::parse(DEFAULTS).unwrap();
        let web = clients.get("web").expect("cliente web");
        assert_eq!(
            (web.name.as_str(), web.number, web.version.as_str()),
            ("WEB", 1, "2.20260708.00.00")
        );
        let visionos = clients.get("visionos").unwrap();
        assert_eq!(visionos.name, "VISIONOS");
        assert!(!visionos.needs_js);
        assert!(clients.get("tv_samsung").unwrap().needs_js);
        assert_eq!(
            clients.get("web_embedded").unwrap().embed_url.as_deref(),
            Some("https://www.reddit.com/")
        );
    }

    #[test]
    fn cascade_order_and_hedge_come_from_the_file() {
        let clients = Clients::parse(DEFAULTS).unwrap();
        assert_eq!(
            clients.cascade().order,
            vec!["visionos", "tv_samsung", "web_embedded"]
        );
        assert_eq!(clients.cascade().hedge_ms, 2000);
        assert_eq!(
            clients.cascade().cookies_order.last().map(String::as_str),
            Some("web_safari")
        );
    }

    #[test]
    fn override_replaces_only_its_own_block() {
        let dir = std::env::temp_dir().join(format!("ml-clients-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(OVERRIDE_FILE),
            "[cascade]\norder = [\"tv_samsung\"]\nhedge_ms = 500\n\n[web]\nname = \"WEB\"\nnumber = 1\nversion = \"2.99990101.00.00\"\n",
        )
        .unwrap();
        let clients = Clients::load(&dir);
        assert_eq!(clients.get("web").unwrap().version, "2.99990101.00.00");
        assert_eq!(clients.get("visionos").unwrap().name, "VISIONOS");
        assert_eq!(clients.cascade().order, vec!["tv_samsung"]);
        assert_eq!(clients.cascade().hedge_ms, 500);
        fs::remove_dir_all(&dir).unwrap();
    }
}
