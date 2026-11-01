//! Prueba de punta a punta: mpv sin pantalla ni audio abre un HLS público a través del proxy
//! local y el tiempo de reproducción avanza. Necesita mpv y red, así que corre con:
//!   cargo test -p media-lite-player -- --ignored

use std::time::{Duration, Instant};

use media_lite_core::http;
use media_lite_player::{Event, Filters, Mode, Options, Player, Proxy};

const MASTER: &str = "https://devstreaming-cdn.apple.com/videos/streaming/examples/img_bipbop_adv_example_ts/master.m3u8";

#[test]
#[ignore = "necesita mpv y conexión a internet"]
fn hls_through_proxy_plays_headless() {
    let proxy = Proxy::start(http::agent()).expect("proxy");
    let player = Player::spawn(&Options {
        mode: Mode::Video,
        vo: "null".into(),
        ao: Some("null".into()),
        ..Options::default()
    })
    .expect("mpv instalado");

    player
        .load(
            &proxy.url_for(MASTER, &Filters::default()),
            &Default::default(),
        )
        .expect("loadfile");

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut loaded = false;
    let mut advanced = false;
    while Instant::now() < deadline && !advanced {
        match player.events().recv_timeout(Duration::from_millis(200)) {
            Ok(Event::FileLoaded) => loaded = true,
            Ok(Event::Property { name, value })
                if name == "time-pos" && value.as_f64().is_some_and(|t| t > 1.0) =>
            {
                advanced = true;
            }
            Ok(Event::EndFile { reason }) => panic!("mpv terminó el archivo: {reason}"),
            _ => {}
        }
    }
    assert!(loaded, "mpv nunca avisó file-loaded");
    assert!(advanced, "time-pos no avanzó");
}
