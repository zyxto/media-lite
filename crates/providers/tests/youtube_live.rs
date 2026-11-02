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
