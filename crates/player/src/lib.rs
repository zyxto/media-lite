//! Reproducción: mpv como proceso aparte (controlado por JSON IPC) y un proxy local que reescribe
//! playlists HLS para quitar anuncios. La interfaz nunca decodifica video.

pub mod ipc;
pub mod mpv;
pub mod playlist;
pub mod proxy;

pub use ipc::Event;
pub use mpv::{LoadOptions, Mode, Options, Player, PlayerError};
pub use proxy::{Filters, Proxy};
