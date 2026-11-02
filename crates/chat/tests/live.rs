//! Pruebas contra los chats reales. Necesitan red y un canal en vivo:
//! `cargo test -p media-lite-chat -- --ignored`. Por defecto se saltean.

use std::time::{Duration, Instant};

use media_lite_chat::{Event, LiveChat, Replay};

/// Espera hasta `secs` a que llegue un mensaje, o un aviso de cierre. Devuelve lo recibido.
fn wait_for_messages(chat: &LiveChat, secs: u64) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        seen.extend(chat.drain());
        if seen.iter().any(|e| matches!(e, Event::Message(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    seen
}

#[test]
#[ignore = "necesita red y un canal de Twitch en vivo"]
fn twitch_chat_delivers_messages() {
    let chat = LiveChat::twitch("auronplay");
    let seen = wait_for_messages(&chat, 60);
    assert!(
        seen.iter().any(|e| matches!(e, Event::Message(_))),
        "sin mensajes: {seen:?}"
    );
}

#[test]
#[ignore = "necesita red y un canal de Kick en vivo"]
fn kick_chat_delivers_history_or_messages() {
    let chat = LiveChat::kick("odablock");
    let seen = wait_for_messages(&chat, 60);
    assert!(
        seen.iter().any(|e| matches!(e, Event::Message(_))),
        "sin mensajes: {seen:?}"
    );
}

#[test]
#[ignore = "necesita red y una grabación de Twitch (cambiar el id si ya no existe)"]
fn twitch_replay_has_comments_around_a_minute_in() {
    let mut replay = Replay::twitch("2912661812");
    let messages = replay.around(60_000);
    assert!(!messages.is_empty(), "sin comentarios en el minuto 1");
    assert!(messages.iter().all(|m| m.time_ms.is_some()));
}
