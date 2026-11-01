//! Resuelve los desafíos de un reproductor, usando la caché y el proceso aparte.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::cache::{valid_id, Cache, Solved};
use crate::run_child;

/// Resultados por desafío, por tipo (`n` o `sig`).
#[derive(Debug, Default, PartialEq)]
pub struct Solutions {
    pub n: HashMap<String, String>,
    pub sig: HashMap<String, String>,
}

/// Pedido: desafíos `n` y `sig` de un reproductor. `player_js` solo hace falta si no hay caché.
pub struct Request<'a> {
    pub player_id: &'a str,
    pub player_js: Option<&'a str>,
    pub n: &'a [String],
    pub sig: &'a [String],
}

/// Resuelve lo que falte, usando la caché cuando puede.
pub fn solve(exe: &Path, cache: &Cache, req: &Request) -> Result<Solutions, String> {
    if !valid_id(req.player_id) {
        return Err("id de reproductor inválido".into());
    }
    let mut solved: Solved = cache.solved(req.player_id);
    let mut out = Solutions::default();
    let mut pending_n: Vec<String> = Vec::new();
    let mut pending_sig: Vec<String> = Vec::new();
    for c in req.n {
        match solved.get(&format!("n|{c}")) {
            Some(v) => {
                out.n.insert(c.clone(), v.clone());
            }
            None => pending_n.push(c.clone()),
        }
    }
    for c in req.sig {
        match solved.get(&format!("sig|{c}")) {
            Some(v) => {
                out.sig.insert(c.clone(), v.clone());
            }
            None => pending_sig.push(c.clone()),
        }
    }
    if pending_n.is_empty() && pending_sig.is_empty() {
        return Ok(out);
    }

    let mut requests = Vec::new();
    if !pending_n.is_empty() {
        requests.push(json!({ "type": "n", "challenges": pending_n }));
    }
    if !pending_sig.is_empty() {
        requests.push(json!({ "type": "sig", "challenges": pending_sig }));
    }
    let input = match cache.preprocessed(req.player_id) {
        Some(pre) => {
            json!({ "type": "preprocessed", "preprocessed_player": pre, "requests": requests })
        }
        None => {
            let player = req
                .player_js
                .ok_or("hace falta el base.js (no hay versión preprocesada)")?;
            json!({ "type": "player", "player": player, "output_preprocessed": true, "requests": requests })
        }
    };
    let reply = run_child(exe, &input, Duration::from_secs(60))?;
    if let Some(pre) = reply.get("preprocessed_player").and_then(Value::as_str) {
        if let Err(e) = cache.store_preprocessed(req.player_id, pre) {
            log::warn!("no se pudo guardar el reproductor preprocesado: {e}");
        }
    }

    let responses = reply
        .get("responses")
        .and_then(Value::as_array)
        .ok_or("respuesta sin desafíos")?;
    for (request, response) in requests.iter().zip(responses) {
        let kind = request["type"].as_str().unwrap_or("");
        if response.get("type").and_then(Value::as_str) != Some("result") {
            let message = response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("error desconocido");
            return Err(format!("no se pudo resolver el desafío {kind}: {message}"));
        }
        let data = response
            .get("data")
            .and_then(Value::as_object)
            .ok_or("desafío sin datos")?;
        for (challenge, value) in data {
            let Some(value) = value.as_str() else {
                continue;
            };
            solved.insert(format!("{kind}|{challenge}"), value.to_string());
            match kind {
                "n" => out.n.insert(challenge.clone(), value.to_string()),
                _ => out.sig.insert(challenge.clone(), value.to_string()),
            };
        }
    }
    if let Err(e) = cache.store_solved(req.player_id, &solved) {
        log::warn!("no se pudieron guardar los desafíos resueltos: {e}");
    }
    Ok(out)
}
