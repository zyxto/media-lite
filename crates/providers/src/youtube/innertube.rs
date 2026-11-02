//! Cliente de innertube (la API interna de YouTube, la que usa el sitio). Un `visitorData` guardado
//! 3 días va en cada pedido, como lo haría un navegador que ya visitó YouTube.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use media_lite_core::http::{self, Agent};
use media_lite_core::store::write_atomic;
use media_lite_core::{Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::clients::{Client, Clients};
use crate::util::now;

const YT: &str = "https://www.youtube.com";
/// Cuánto vale un `visitorData` guardado.
const VISITOR_TTL: u64 = 3 * 24 * 3600;
/// Respuestas grandes (una portada entera pesa varios MB).
const MAX_JSON: u64 = 16 * 1024 * 1024;
const STATE_FILE: &str = "youtube.json";

#[derive(Default, Serialize, Deserialize)]
struct Visitor {
    visitor_data: String,
    at: u64,
}

pub struct Innertube {
    agent: Agent,
    clients: Clients,
    visitor: Mutex<Visitor>,
    state_path: PathBuf,
}

impl Innertube {
    pub fn new(config_dir: &Path) -> Innertube {
        let state_path = config_dir.join(STATE_FILE);
        let saved: Visitor = std::fs::read(&state_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let visitor = if now().saturating_sub(saved.at) < VISITOR_TTL {
            saved
        } else {
            Visitor::default()
        };
        Innertube {
            agent: http::agent(),
            clients: Clients::load(config_dir),
            visitor: Mutex::new(visitor),
            state_path,
        }
    }

    /// Cliente web (el que sirve para catálogo, búsqueda y canales).
    pub fn web(&self) -> Result<Client> {
        self.clients
            .get("web")
            .cloned()
            .ok_or_else(|| Error::new(ErrorKind::Other, "falta el cliente web en clients.toml"))
    }

    /// Un `visitorData` vigente. Si no hay, lo pide a YouTube.
    pub fn visitor(&self) -> String {
        if let Ok(v) = self.visitor.lock() {
            if !v.visitor_data.is_empty() {
                return v.visitor_data.clone();
            }
        }
        // Sin visitorData, `visitor_id` lo da; si no, `browse` de la portada de «Para ti».
        let _ = self.post("visitor_id", json!({}));
        let _ = self.post("browse", json!({ "browseId": "FEwhat_to_watch" }));
        self.visitor
            .lock()
            .map(|v| v.visitor_data.clone())
            .unwrap_or_default()
    }

    /// Pedido al endpoint `endpoint` (`browse`, `search`, `next`…) con el cliente web.
    pub fn post(&self, endpoint: &str, body: Value) -> Result<Value> {
        let client = self.web()?;
        let visitor = self
            .visitor
            .lock()
            .map(|v| v.visitor_data.clone())
            .unwrap_or_default();
        let mut context =
            json!({ "clientName": client.name, "clientVersion": client.version, "hl": "es" });
        if !visitor.is_empty() {
            context["visitorData"] = json!(visitor);
        }
        let mut payload = json!({ "context": { "client": context } });
        if let (Some(target), Value::Object(extra)) = (payload.as_object_mut(), body) {
            target.extend(extra);
        }
        let number = client.number.to_string();
        let headers = [
            ("Content-Type", "application/json"),
            ("Origin", YT),
            ("X-Origin", YT),
            ("Referer", "https://www.youtube.com/"),
            ("X-Youtube-Client-Name", number.as_str()),
            ("X-Youtube-Client-Version", client.version.as_str()),
        ];
        let url = format!("{YT}/youtubei/v1/{endpoint}?prettyPrint=false");
        let text = payload.to_string();
        let response = http::request(&self.agent, "POST", &url, &headers, Some(&text), MAX_JSON)?;
        if !response.ok() {
            return Err(Error::network(format!(
                "YouTube respondió con error {}",
                response.status
            )));
        }
        let value: Value = serde_json::from_slice(&response.body)
            .map_err(|_| Error::parse("YouTube: la respuesta no se entiende"))?;
        self.remember_visitor(&value);
        Ok(value)
    }

    /// Guarda el `visitorData` que YouTube manda en cada respuesta.
    fn remember_visitor(&self, value: &Value) {
        let Some(fresh) = value["responseContext"]["visitorData"].as_str() else {
            return;
        };
        let Ok(mut visitor) = self.visitor.lock() else {
            return;
        };
        if fresh == visitor.visitor_data {
            return;
        }
        visitor.visitor_data = fresh.to_string();
        visitor.at = now();
        match serde_json::to_vec(&*visitor) {
            Ok(bytes) => {
                if let Err(e) = write_atomic(&self.state_path, &bytes, 0o644) {
                    log::warn!("no se pudo guardar el visitorData de YouTube: {e}");
                }
            }
            Err(e) => log::warn!("visitorData de YouTube: {e}"),
        }
    }
}
