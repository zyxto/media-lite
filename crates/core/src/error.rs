//! Errores de los proveedores. Los mensajes son para el usuario, en español.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// La plataforma no ofrece esta función.
    Unsupported,
    /// No hay conexión o el servidor no respondió.
    Network,
    /// El contenido no existe o no está disponible en este país.
    Unavailable,
    /// El contenido tiene DRM y no se puede reproducir.
    Drm,
    /// La plataforma pide iniciar sesión.
    Login,
    /// La respuesta llegó pero no se entendió (cambió el formato).
    Parse,
    /// Error local de disco o de otro tipo.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error {
            kind,
            message: message.into(),
        }
    }

    pub fn unsupported() -> Error {
        Error::new(
            ErrorKind::Unsupported,
            "Esta plataforma no tiene esta función",
        )
    }

    pub fn network(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Network, message)
    }

    pub fn parse(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Parse, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::new(ErrorKind::Other, e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
