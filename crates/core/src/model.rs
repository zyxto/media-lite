//! Modelo común a todas las plataformas: lo que devuelven los proveedores y lo que muestra la interfaz.

use serde::{Deserialize, Serialize};

/// Plataformas conocidas. El orden es el de la barra lateral.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Pluto,
    Runtime,
    Plex,
    YouTube,
    YouTubeMusic,
    Twitch,
    Kick,
    Demo,
}

impl Platform {
    pub const ALL: [Platform; 8] = [
        Platform::Pluto,
        Platform::Runtime,
        Platform::Plex,
        Platform::YouTube,
        Platform::YouTubeMusic,
        Platform::Twitch,
        Platform::Kick,
        Platform::Demo,
    ];

    /// Clave estable para CLI, configuración y caché.
    pub fn key(self) -> &'static str {
        match self {
            Platform::Pluto => "pluto",
            Platform::Runtime => "runtime",
            Platform::Plex => "plex",
            Platform::YouTube => "youtube",
            Platform::YouTubeMusic => "ytmusic",
            Platform::Twitch => "twitch",
            Platform::Kick => "kick",
            Platform::Demo => "demo",
        }
    }

    /// Nombre para mostrar.
    pub fn label(self) -> &'static str {
        match self {
            Platform::Pluto => "Pluto TV",
            Platform::Runtime => "Runtime",
            Platform::Plex => "Plex",
            Platform::YouTube => "YouTube",
            Platform::YouTubeMusic => "YouTube Music",
            Platform::Twitch => "Twitch",
            Platform::Kick => "Kick",
            Platform::Demo => "Demo",
        }
    }

    pub fn from_key(key: &str) -> Option<Platform> {
        Platform::ALL.into_iter().find(|p| p.key() == key)
    }
}

/// Secciones de una plataforma (las pestañas de la barra lateral).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    Live,
    Movies,
    Series,
    Videos,
    Music,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Live,
        Section::Movies,
        Section::Series,
        Section::Videos,
        Section::Music,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Section::Live => "live",
            Section::Movies => "movies",
            Section::Series => "series",
            Section::Videos => "videos",
            Section::Music => "music",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Section::Live => "En vivo",
            Section::Movies => "Películas",
            Section::Series => "Series",
            Section::Videos => "Videos",
            Section::Music => "Música",
        }
    }

    pub fn from_key(key: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// Qué es cada elemento de una lista.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Channel,
    Movie,
    Series,
    Episode,
    Video,
    Short,
    Live,
    Track,
    Album,
    Artist,
}

/// Un elemento de cualquier lista: tarjeta de canal, película, video, canción…
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub platform: Platform,
    pub kind: Kind,
    /// Identificador que entiende el proveedor (ID de canal, de video, de título…).
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    /// URL `http(s)://`, `file://` o `data:` de la miniatura.
    #[serde(default)]
    pub thumb: Option<String>,
    /// Imagen apaisada y más grande, para el fondo de la pantalla de detalle. Si falta, ese
    /// fondo se arma con `thumb`.
    #[serde(default)]
    pub backdrop: Option<String>,
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub duration_secs: Option<u64>,
    /// Sinopsis, para la pantalla de detalle.
    #[serde(default)]
    pub description: String,
    /// Dato opaco que entiende solo el proveedor (ruta del stream, clave de la parte…).
    #[serde(default)]
    pub extra: Option<String>,
    /// Temporada y episodio, si es un episodio.
    #[serde(default)]
    pub season: Option<u32>,
    #[serde(default)]
    pub episode: Option<u32>,
}

impl Item {
    pub fn new(
        platform: Platform,
        kind: Kind,
        id: impl Into<String>,
        title: impl Into<String>,
    ) -> Item {
        Item {
            platform,
            kind,
            id: id.into(),
            title: title.into(),
            subtitle: String::new(),
            thumb: None,
            backdrop: None,
            live: false,
            duration_secs: None,
            description: String::new(),
            extra: None,
            season: None,
            episode: None,
        }
    }
}

/// Una página de resultados. `next` es un cursor opaco para pedir la siguiente.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub items: Vec<Item>,
    pub next: Option<String>,
}

/// Una fila de la portada ("Seguir viendo", "Recomendados"…).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shelf {
    pub title: String,
    pub items: Vec<Item>,
}

/// Subcategoría de una sección (Acción, Comedia, Noticias…).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub title: String,
}

/// Una fuente reproducible: una URL con su calidad y, si hace falta, cabeceras.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub url: String,
    pub label: String,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub live: bool,
    /// Pista de audio separada (video y audio en DASH).
    #[serde(default)]
    pub audio_url: Option<String>,
    /// Fragmentos de URL de segmentos que se quitan al reproducir (anuncios de Pluto TV, por ejemplo).
    #[serde(default)]
    pub drop: Vec<String>,
    /// Quitar los cortes publicitarios de Twitch (`EXT-X-DATERANGE` de `twitch-stitched-ad`).
    #[serde(default)]
    pub twitch_ads: bool,
    /// Desde cuándo se puede ver (segundos desde la época). Si es posterior a ahora, YouTube sirve
    /// el video después de un anuncio previo: la app espera y lo dice.
    #[serde(default)]
    pub available_at: Option<u64>,
}

/// Una temporada de una serie, con sus episodios.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Season {
    pub number: u32,
    pub name: String,
    pub episodes: Vec<Item>,
}

/// Información completa de un elemento, para la pantalla de detalle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Details {
    pub item: Item,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sources: Vec<Source>,
}

/// Lo que soporta una plataforma. Se usa para mostrar u ocultar botones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    pub live: bool,
    pub movies: bool,
    pub series: bool,
    pub videos: bool,
    pub music: bool,
    pub search: bool,
    pub guide: bool,
    pub chat: bool,
    pub login: bool,
}
