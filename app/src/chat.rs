//! Chat junto al reproductor: el de un directo de Twitch o Kick, o las repeticiones del chat de
//! una grabación, sincronizadas con el tiempo del video. Se muestra como una columna superpuesta
//! a la derecha de la grilla.

use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use media_lite_chat::{Event, LiveChat, Message, Replay};
use media_lite_core::{Item, Kind, Platform};
use slint::{Color, ModelRc, VecModel};

use crate::state::{self, App};
use crate::ChatLine;

/// Mensajes que se guardan en memoria.
const KEEP: usize = 300;
/// Mensajes que se muestran (los más nuevos, abajo).
const SHOWN: usize = 60;
/// Cada cuánto se piden más mensajes de la repetición.
const REPLAY_EVERY: Duration = Duration::from_secs(1);
/// Color de los nombres sin color propio (el acento del tema).
const ACCENT: Color = Color::from_rgb_u8(0x7c, 0x9c, 0xff);

#[derive(Clone, Debug, PartialEq)]
struct Line {
    author: String,
    text: String,
    color: Option<String>,
    notice: bool,
}

/// Estado del chat del reproductor. Cada reproducción nueva empieza una `session`, y las
/// respuestas de una sesión vieja se descartan.
#[derive(Default)]
pub struct Chat {
    live: Option<LiveChat>,
    replay: Option<Arc<Mutex<Replay>>>,
    replay_at: Option<Instant>,
    replay_busy: bool,
    lines: VecDeque<Line>,
    open: bool,
    session: u64,
}

impl Chat {
    fn reset(&mut self) {
        let session = self.session + 1;
        *self = Chat {
            session,
            ..Chat::default()
        };
    }
}

/// Arranca el chat de un elemento que se empieza a reproducir (si tiene).
pub fn begin(app: &Rc<App>, item: &Item) {
    let session = {
        let mut chat = app.chat.borrow_mut();
        chat.reset();
        chat.session
    };
    match (item.platform, item.kind) {
        (Platform::Twitch, Kind::Live) => {
            app.chat.borrow_mut().live = Some(LiveChat::twitch(&item.id));
        }
        (Platform::Kick, Kind::Live) => {
            app.chat.borrow_mut().live = Some(LiveChat::kick(&item.id));
        }
        (Platform::Twitch, Kind::Video) => {
            app.chat.borrow_mut().replay = Some(Arc::new(Mutex::new(Replay::twitch(&item.id))));
        }
        (Platform::Kick, Kind::Video) => {
            // Kick necesita pedir la fecha de la grabación: se hace fuera de la interfaz.
            let uuid = item.id.clone();
            app.pool.run(move || {
                let replay = Replay::kick(&uuid);
                let _ = slint::invoke_from_event_loop(move || {
                    state::with_app(|app| match replay {
                        Ok(replay) => attach_replay(app, session, replay),
                        Err(e) => log::warn!("chat de la grabación de Kick: {e}"),
                    });
                });
            });
        }
        _ => {}
    }
    refresh_open(app);
    sync(app);
}

fn attach_replay(app: &Rc<App>, session: u64, replay: Replay) {
    {
        let mut chat = app.chat.borrow_mut();
        if chat.session != session {
            return;
        }
        chat.replay = Some(Arc::new(Mutex::new(replay)));
    }
    refresh_open(app);
    sync(app);
}

/// Corta el chat (se cerró o se detuvo el reproductor).
pub fn end(app: &Rc<App>) {
    app.chat.borrow_mut().reset();
    sync(app);
}

/// Llamado desde el temporizador del reproductor: trae lo nuevo del chat y, en una repetición,
/// pide los mensajes del tramo que se está viendo.
pub fn pump(app: &Rc<App>) {
    let position = app
        .playback
        .borrow()
        .as_ref()
        .map(|pb| (pb.position.max(0.0) * 1000.0) as u64);
    let mut changed = false;
    let mut request = None;
    {
        let mut chat = app.chat.borrow_mut();
        let Chat {
            live,
            lines,
            replay,
            replay_at,
            replay_busy,
            session,
            ..
        } = &mut *chat;
        if let Some(live) = live.as_ref() {
            for event in live.drain() {
                changed = true;
                push(lines, event);
            }
        }
        if let (Some(replay), Some(at)) = (replay.as_ref(), position) {
            let due = replay_at.is_none_or(|t| t.elapsed() >= REPLAY_EVERY);
            if due && !*replay_busy {
                *replay_busy = true;
                *replay_at = Some(Instant::now());
                request = Some((Arc::clone(replay), at, *session));
            }
        }
    }
    if let Some((replay, at, session)) = request {
        app.pool.run(move || {
            let messages = match replay.lock() {
                Ok(mut replay) => replay.around(at),
                Err(_) => Vec::new(),
            };
            let _ = slint::invoke_from_event_loop(move || {
                state::with_app(|app| deliver(app, session, at, messages));
            });
        });
    }
    if changed {
        sync(app);
    }
}

/// Llegaron los mensajes de una repetición: se muestran los que ya pasaron en el video.
fn deliver(app: &Rc<App>, session: u64, at: u64, messages: Vec<Message>) {
    {
        let mut chat = app.chat.borrow_mut();
        chat.replay_busy = false;
        if chat.session != session {
            return;
        }
        let mut shown: Vec<Message> = messages
            .into_iter()
            .filter(|m| m.time_ms.unwrap_or(0) <= at)
            .collect();
        shown.sort_by_key(|m| m.time_ms);
        let skip = shown.len().saturating_sub(KEEP);
        chat.lines = shown.into_iter().skip(skip).map(|m| line_of(&m)).collect();
    }
    sync(app);
}

fn push(lines: &mut VecDeque<Line>, event: Event) {
    let line = match event {
        Event::Message(m) => line_of(&m),
        Event::Notice(text) | Event::Closed(text) => Line {
            author: String::new(),
            text,
            color: None,
            notice: true,
        },
    };
    lines.push_back(line);
    while lines.len() > KEEP {
        lines.pop_front();
    }
}

fn line_of(m: &Message) -> Line {
    Line {
        author: m.author.clone(),
        text: m.text.clone(),
        color: m.color.clone(),
        notice: false,
    }
}

fn refresh_open(app: &Rc<App>) {
    let mut chat = app.chat.borrow_mut();
    chat.open = chat.live.is_some() || chat.replay.is_some();
}

/// Lleva el estado del chat a la ventana.
pub fn sync(app: &Rc<App>) {
    let Some(window) = app.weak.upgrade() else {
        return;
    };
    let chat = app.chat.borrow();
    window.set_chat_open(chat.open);
    let skip = chat.lines.len().saturating_sub(SHOWN);
    let rows: Vec<ChatLine> = chat.lines.iter().skip(skip).map(to_row).collect();
    window.set_chat_lines(ModelRc::new(VecModel::from(rows)));
}

fn to_row(line: &Line) -> ChatLine {
    ChatLine {
        author: line.author.as_str().into(),
        text: line.text.as_str().into(),
        color: line.color.as_deref().and_then(parse_hex).unwrap_or(ACCENT),
        notice: line.notice,
    }
}

/// `#RRGGBB` a color. Otro formato no se usa.
fn parse_hex(hex: &str) -> Option<Color> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&digits[range], 16).ok();
    Some(Color::from_rgb_u8(
        channel(0..2)?,
        channel(2..4)?,
        channel(4..6)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors_parse_and_others_do_not() {
        assert_eq!(parse_hex("#FF8000"), Some(Color::from_rgb_u8(255, 128, 0)));
        assert_eq!(parse_hex("FF8000"), None);
        assert_eq!(parse_hex("#FFF"), None);
        assert_eq!(parse_hex("#GG0000"), None);
    }

    #[test]
    fn only_the_newest_messages_are_kept() {
        let mut lines = VecDeque::new();
        for i in 0..(KEEP + 5) {
            push(&mut lines, Event::Notice(format!("{i}")));
        }
        assert_eq!(lines.len(), KEEP);
        assert_eq!(lines.front().unwrap().text, "5");
    }
}
