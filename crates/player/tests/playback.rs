//! Prueba de punta a punta: mpv sin pantalla ni audio abre un HLS público a través del proxy
//! local y el tiempo de reproducción avanza. Necesita mpv y red, así que corre con:
//!   cargo test -p media-lite-player -- --ignored

use std::process::Command;
use std::time::{Duration, Instant};

use media_lite_core::http;
use media_lite_player::{mpv, Event, Filters, LoadOptions, Mode, Options, Player, Proxy};

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

/// Cerrar mpv desde su ventana lo termina y el `Player` tiene que notarlo: si no, la app reusa
/// ese socket muerto y falla con «Broken pipe». Después de cerrarlo, un `Player` nuevo tiene que
/// arrancar y aceptar comandos.
#[test]
#[ignore = "necesita mpv"]
fn player_notices_when_mpv_was_closed() {
    let options = Options {
        mode: Mode::Video,
        vo: "null".into(),
        ao: Some("null".into()),
        ..Options::default()
    };
    let mut player = Player::spawn(&options).expect("mpv instalado");
    assert!(player.is_running());

    // Cierra mpv desde afuera, como si el usuario cerrara su ventana.
    let socket = mpv::socket_path();
    let status = Command::new("pkill")
        .arg("-f")
        .arg(&socket)
        .status()
        .expect("pkill");
    assert!(status.success(), "pkill no encontró mpv");

    let deadline = Instant::now() + Duration::from_secs(5);
    while player.is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!player.is_running(), "el Player no notó que mpv se cerró");
    drop(player);

    let mut player = Player::spawn(&options).expect("mpv nuevo");
    assert!(player.is_running());
    player
        .load("av://lavfi:testsrc=duration=1", &Default::default())
        .expect("loadfile en el mpv nuevo");
}

/// Video y audio por separado, como los videos de YouTube: mpv tiene que abrir y elegir las dos
/// pistas. Antes el audio se agregaba con `audio-add` apenas después de `loadfile`, mpv lo
/// rechazaba porque el archivo todavía no estaba abierto, y el video quedaba sin sonido.
#[test]
#[ignore = "necesita mpv y ffmpeg"]
fn separate_audio_track_is_selected() {
    // Archivos de verdad: mpv solo une fuentes en las que se puede saltar (no sirve `av://lavfi`).
    let dir = std::env::temp_dir().join(format!("media-lite-dash-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("carpeta temporal");
    let video_file = dir.join("video.mp4");
    let audio_file = dir.join("audio.m4a");
    for (input, extra, out) in [
        ("testsrc=size=160x90:rate=10", "-an", &video_file),
        ("sine=frequency=440", "-vn", &audio_file),
    ] {
        let status = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                input,
            ])
            .args(["-t", "5", extra])
            .arg(out)
            .status()
            .expect("ffmpeg instalado");
        assert!(status.success(), "ffmpeg no generó {}", out.display());
    }

    let player = Player::spawn(&Options {
        mode: Mode::Video,
        vo: "null".into(),
        ao: Some("null".into()),
        ..Options::default()
    })
    .expect("mpv instalado");
    player
        .load(
            &video_file.display().to_string(),
            &LoadOptions {
                audio_url: Some(audio_file.display().to_string()),
            },
        )
        .expect("loadfile");

    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut video, mut audio) = (false, false);
    while Instant::now() < deadline && !(video && audio) {
        match player.events().recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Property { name, value }) if name == "track-list" => {
                let selected = |kind: &str| {
                    value.as_array().is_some_and(|tracks| {
                        tracks
                            .iter()
                            .any(|t| t["type"] == kind && t["selected"] == true)
                    })
                };
                video = selected("video");
                audio = selected("audio");
            }
            Ok(Event::EndFile { reason }) => panic!("mpv terminó el archivo: {reason}"),
            _ => {}
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(video, "mpv no eligió la pista de video");
    assert!(
        audio,
        "mpv no eligió la pista de audio: el video quedaría sin sonido"
    );
}
