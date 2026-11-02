//! `media-lite-cli`: prueba los proveedores desde la terminal y responde con JSON.
//!
//! Uso:
//!   media-lite-cli platforms
//!   media-lite-cli list <plataforma> <sección> [--category <id>] [--cursor <c>]
//!   media-lite-cli search <plataforma> <consulta> [--cursor <c>]
//!   media-lite-cli categories <plataforma> <sección>
//!   media-lite-cli seasons <plataforma> <id-de-la-serie>
//!   media-lite-cli details <plataforma> <id> [--kind live|movie|series|episode]

use std::process::ExitCode;

use media_lite_core::{Item, Kind, Platform, Section};
use media_lite_providers::{find, registry, Provider};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // El solucionador de YouTube corre con este mismo binario (`--yt-solver`), como en la app.
    if args.first().map(String::as_str) == Some("--yt-solver") {
        return ExitCode::from(media_lite_ytsolver::child_main() as u8);
    }
    match run(&args) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "uso: media-lite-cli platforms | list <plataforma> <sección> | search <plataforma> <consulta> | categories <plataforma> <sección> | seasons <plataforma> <id> | details <plataforma> <id> | yt-resolve <id|url> [--client X] [--dump DIR] [--timing]";

fn run(args: &[String]) -> Result<String, String> {
    let providers = registry(&media_lite_core::paths::config_dir(), false);
    let (positional, flags) = split_flags(args);
    let cursor = flag(&flags, "--cursor");
    let category = flag(&flags, "--category");
    match positional.first().map(String::as_str) {
        Some("platforms") => Ok(platforms_json(&providers)),
        Some("list") => {
            let provider = provider_arg(&providers, positional.get(1))?;
            let section = positional
                .get(2)
                .and_then(|s| Section::from_key(s))
                .ok_or("sección desconocida (live, movies, series, videos, music)")?;
            let page = provider
                .list(section, category.as_deref(), cursor.as_deref())
                .map_err(|e| e.to_string())?;
            to_json(&page)
        }
        Some("search") => {
            let provider = provider_arg(&providers, positional.get(1))?;
            let query = positional.get(2).ok_or("falta la consulta")?;
            let page = provider
                .search(query, cursor.as_deref())
                .map_err(|e| e.to_string())?;
            to_json(&page)
        }
        Some("categories") => {
            let provider = provider_arg(&providers, positional.get(1))?;
            let section = positional
                .get(2)
                .and_then(|s| Section::from_key(s))
                .ok_or("sección desconocida (live, movies, series, videos, music)")?;
            let cats = provider.categories(section).map_err(|e| e.to_string())?;
            to_json(&cats)
        }
        Some("seasons") => {
            let provider = provider_arg(&providers, positional.get(1))?;
            let id = positional.get(2).ok_or("falta el id de la serie")?;
            let series = Item::new(provider.platform(), Kind::Series, id.clone(), String::new());
            let seasons = provider.seasons(&series).map_err(|e| e.to_string())?;
            to_json(&seasons)
        }
        Some("yt-resolve") => yt_resolve(positional.get(1), &flags),
        Some("details") => {
            let provider = provider_arg(&providers, positional.get(1))?;
            let id = positional.get(2).ok_or("falta el id")?;
            let kind = match flag(&flags, "--kind").as_deref() {
                Some("live") => Kind::Live,
                Some("channel") => Kind::Channel,
                Some("series") => Kind::Series,
                Some("episode") => Kind::Episode,
                Some("movie") => Kind::Movie,
                _ => Kind::Video,
            };
            let mut item = Item::new(provider.platform(), kind, id.clone(), String::new());
            item.live = matches!(kind, Kind::Live | Kind::Channel);
            let details = provider.details(&item).map_err(|e| e.to_string())?;
            to_json(&details)
        }
        _ => Err("comando desconocido".to_string()),
    }
}

fn provider_arg(
    providers: &[std::sync::Arc<dyn Provider>],
    key: Option<&String>,
) -> Result<std::sync::Arc<dyn Provider>, String> {
    let key = key.ok_or("falta la plataforma")?;
    let platform =
        Platform::from_key(key).ok_or_else(|| format!("plataforma desconocida: {key}"))?;
    find(providers, platform)
        .ok_or_else(|| format!("{} todavía no está disponible", platform.label()))
}

fn platforms_json(providers: &[std::sync::Arc<dyn Provider>]) -> String {
    let list: Vec<serde_json::Value> = providers
        .iter()
        .map(|p| {
            serde_json::json!({
                "key": p.platform().key(),
                "label": p.platform().label(),
                "caps": p.caps(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&list).unwrap_or_default()
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|e| e.to_string())
}

/// Separa los argumentos posicionales de los `--flag valor`.
fn split_flags(args: &[String]) -> (Vec<String>, Vec<(String, String)>) {
    let mut positional = Vec::new();
    let mut flags = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--timing" {
            flags.push((arg.clone(), "1".to_string()));
        } else if arg.starts_with("--") {
            if let Some(value) = iter.next() {
                flags.push((arg.clone(), value.clone()));
            }
        } else {
            positional.push(arg.clone());
        }
    }
    (positional, flags)
}

fn flag(flags: &[(String, String)], name: &str) -> Option<String> {
    flags
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

/// `yt-resolve <id|url>`: prueba la cascada de YouTube paso a paso y muestra cada cliente, sus
/// fuentes y los tiempos. Con `--dump DIR` guarda las respuestas crudas (sin cookies ni
/// `visitorData`) como fixtures. Con `--client X`, prueba solo ese cliente.
fn yt_resolve(input: Option<&String>, flags: &[(String, String)]) -> Result<String, String> {
    use media_lite_providers::youtube::innertube::Innertube;
    use media_lite_providers::youtube::resolve::Resolver;
    use media_lite_providers::youtube::video_id_of;
    use std::path::PathBuf;
    use std::sync::Arc;

    let input = input.ok_or("falta el id o la URL del video")?;
    let id = video_id_of(input).ok_or("no se reconoce el id del video")?;
    let config = media_lite_core::paths::config_dir();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let api = Arc::new(Innertube::new(&config));
    let resolver = Resolver::new(&config, &media_lite_core::paths::cache_dir(), api, exe);
    let keys = match flag(flags, "--client") {
        Some(client) => vec![client],
        None => resolver.order_keys(),
    };
    let dump = flag(flags, "--dump").map(PathBuf::from);
    if let Some(dir) = &dump {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }

    let mut attempts = Vec::new();
    let mut winner = None;
    for key in keys {
        let outcome = resolver.fetch_client(&key, &id).and_then(|(body, timing)| {
            if let Some(dir) = &dump {
                let path = dir.join(format!("player-{key}-{id}.json"));
                let mut scrubbed = body.clone();
                scrub_visitor(&mut scrubbed);
                std::fs::write(&path, scrubbed.to_string()).map_err(|e| {
                    media_lite_core::Error::new(media_lite_core::ErrorKind::Other, e.to_string())
                })?;
            }
            let mut timing = timing;
            resolver.build(&key, &id, &body, &mut timing)
        });
        match outcome {
            Ok(resolved) => {
                attempts.push(serde_json::json!({ "client": key, "ok": true, "timing_ms": timing_json(&resolved.timing) }));
                winner = Some(resolved);
                break;
            }
            Err(e) => {
                attempts.push(serde_json::json!({ "client": key, "ok": false, "error": e.message }))
            }
        }
    }
    let sources: Vec<serde_json::Value> = winner
        .as_ref()
        .map(|r| {
            r.sources
                .iter()
                .map(|s| serde_json::json!({ "label": s.label, "height": s.height, "live": s.live, "audio": s.audio_url.is_some(), "available_at": s.available_at }))
                .collect()
        })
        .unwrap_or_default();
    let report = serde_json::json!({
        "video": id,
        "winner": winner.as_ref().map(|r| r.client.clone()),
        "available_at": winner.as_ref().map(|r| r.available_at),
        "sources": sources,
        "attempts": attempts,
    });
    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
}

fn timing_json(t: &media_lite_providers::youtube::resolve::Timing) -> serde_json::Value {
    serde_json::json!({ "prep": t.prep_ms, "player": t.player_ms, "solver": t.solver_ms, "total": t.total_ms })
}

/// Quita `visitorData` de una respuesta (no se guarda en los fixtures).
fn scrub_visitor(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("visitorData");
            map.values_mut().for_each(scrub_visitor);
        }
        serde_json::Value::Array(list) => list.iter_mut().for_each(scrub_visitor),
        _ => {}
    }
}
