//! Pruebas contra YouTube real que NO llaman a `/player`: el reproductor (`iframe_api` y
//! `base.js`) y su preparación con el solucionador. Necesitan red y el binario de la app:
//! `cargo build -p media-lite && MEDIA_LITE_EXE=target/debug/media-lite cargo test -p media-lite-providers --test youtube_live -- --ignored`.

use std::path::PathBuf;
use std::sync::Arc;

use media_lite_providers::youtube::innertube::Innertube;
use media_lite_providers::youtube::resolve::Resolver;

fn exe() -> PathBuf {
    PathBuf::from(
        std::env::var("MEDIA_LITE_EXE").expect("definir MEDIA_LITE_EXE con el binario media-lite"),
    )
}

#[test]
#[ignore = "necesita red y MEDIA_LITE_EXE"]
fn warm_prepares_the_player_and_keeps_its_timestamp() {
    let root = std::env::temp_dir().join(format!("ml-warm-{}", std::process::id()));
    let config = root.join("config");
    let cache = root.join("cache");
    std::fs::create_dir_all(&config).unwrap();
    let api = Arc::new(Innertube::new(&config));
    let resolver = Resolver::new(&config, &cache, api, exe());
    resolver.warm();

    let player_dir = cache.join("yt-player");
    let sts_file = std::fs::read_dir(&player_dir)
        .expect("carpeta del reproductor")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "sts"))
        .expect("archivo .sts");
    let sts = std::fs::read_to_string(&sts_file).unwrap();
    assert_eq!(
        sts.trim().len(),
        5,
        "signatureTimestamp de 5 dígitos: {sts}"
    );
    let js = sts_file.with_extension("js");
    assert!(js.exists(), "reproductor preparado");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
#[ignore = "necesita red (lrclib.net)"]
fn lrclib_has_synced_lyrics_for_a_known_song() {
    let agent = media_lite_core::http::agent();
    let lines = media_lite_providers::lyrics::fetch(
        &agent,
        "Never Gonna Give You Up",
        "Rick Astley",
        "",
        Some(213),
    )
    .expect("letra sincronizada");
    assert!(lines.len() > 10, "{} líneas", lines.len());
    assert!(lines.windows(2).all(|w| w[0].at <= w[1].at));
}

#[test]
#[ignore = "necesita red y MEDIA_LITE_EXE (prepara el reproductor y resuelve desafíos n)"]
fn the_solver_resolves_n_challenges_with_the_real_player() {
    use media_lite_ytsolver::cache::Cache;
    use media_lite_ytsolver::solve::{solve, Request};

    let root = std::env::temp_dir().join(format!("ml-solve-{}", std::process::id()));
    let config = root.join("config");
    let cache_root = root.join("cache");
    std::fs::create_dir_all(&config).unwrap();
    let api = Arc::new(Innertube::new(&config));
    let resolver = Resolver::new(&config, &cache_root, api, exe());
    resolver.warm();

    // El id del reproductor preparado: el nombre del archivo `.sts` que dejó `warm`.
    let player_dir = cache_root.join("yt-player");
    let player_id = std::fs::read_dir(&player_dir)
        .expect("carpeta del reproductor")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "sts"))
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .expect("id del reproductor");

    let cache = Cache::new(&player_dir);
    let challenges = vec!["AbCdEfGhIjKl".to_string(), "xYz123QwErTy".to_string()];
    let request = Request {
        player_id: &player_id,
        player_js: None,
        n: &challenges,
        sig: &[],
    };
    let solved = solve(&exe(), &cache, &request).expect("el solucionador resuelve los desafíos");
    for challenge in &challenges {
        let answer = solved
            .n
            .get(challenge)
            .expect("respuesta para cada desafío");
        assert!(
            !answer.is_empty() && answer != challenge,
            "respuesta: {answer}"
        );
    }
    std::fs::remove_dir_all(&root).ok();
}
