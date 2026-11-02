//! Chat en vivo de Twitch (IRC sobre WebSocket) y de Kick (WebSocket de Centrifugo), y las
//! repeticiones de las grabaciones. Portado de `Sanson-FLinux/plugins/{twitch,kick}/script.js`.
//!
//! Cada conexión corre en su hilo y entrega eventos por un canal. Al soltar el `LiveChat`, el hilo
//! deja de leer y cierra el socket.

pub mod irc;
pub mod kick;
pub mod live;
pub mod replay;

pub use live::LiveChat;
pub use replay::Replay;

/// Un emote del mensaje: su nombre (como aparece en el texto) y la imagen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emote {
    pub name: String,
    pub url: String,
}

/// Un mensaje del chat. `time_ms` solo viene en las repeticiones (ms desde el inicio del video).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub author: String,
    pub text: String,
    /// Color del nombre, como `#RRGGBB`, si la plataforma lo da.
    pub color: Option<String>,
    pub emotes: Vec<Emote>,
    pub time_ms: Option<u64>,
}

/// Lo que llega del chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Message(Message),
    /// Suscripciones, raids y otros avisos del sistema.
    Notice(String),
    /// La conexión terminó. El texto dice por qué, en español.
    Closed(String),
}
