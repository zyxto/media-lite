//! `media-lite-cli`: prueba los proveedores desde la terminal y responde con JSON.
//!
//! Uso:
//!   media-lite-cli platforms
//!   media-lite-cli list <plataforma> <sección> [--category <id>] [--cursor <c>]
//!   media-lite-cli search <plataforma> <consulta> [--cursor <c>]
//!   media-lite-cli details <plataforma> <id>

use std::process::ExitCode;

use media_lite_core::{Item, Kind, Platform, Section};
use media_lite_providers::{find, registry, Provider};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
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

const USAGE: &str = "uso: media-lite-cli platforms | list <plataforma> <sección> | search <plataforma> <consulta> | details <plataforma> <id>";

fn run(args: &[String]) -> Result<String, String> {
    let providers = registry();
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
        Some("details") => {
            let provider = provider_arg(&providers, positional.get(1))?;
            let id = positional.get(2).ok_or("falta el id")?;
            let item = Item::new(provider.platform(), Kind::Video, id.clone(), String::new());
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
        if arg.starts_with("--") {
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
