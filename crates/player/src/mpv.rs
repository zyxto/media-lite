//! mpv como proceso aparte. La app lo lanza, le manda comandos por IPC y lo cierra al salir.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use media_lite_core::paths;
use serde_json::json;

use crate::ipc::{Event, Ipc};

/// Cómo se muestra el video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ventana flotante chica, arriba de todo y sin bordes, en una esquina: se puede trabajar debajo.
    Pip,
    /// Ventana normal.
    Video,
    /// Solo audio (música): casi no usa CPU.
    AudioOnly,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub mode: Mode,
    /// Salida de video: `xv` (liviana en GMA 4500), `gpu` o `null` (para pruebas sin pantalla).
    pub vo: String,
    /// Decodificación por hardware: `no` por defecto; `auto-safe` o `vaapi` si el equipo lo soporta.
    pub hwdec: String,
    /// Video liviano: se saltan cuadros para que el equipo responda mejor.
    pub low_video: bool,
    /// Tope del caché de red, en MB. Acota la RAM de mpv.
    pub cache_mb: u32,
    /// Driver de audio. `None` deja el de mpv; `null` sirve para pruebas.
    pub ao: Option<String>,
    /// Mensajes detallados de mpv (para diagnosticar). Salen por el registro de la app.
    pub verbose: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            mode: Mode::Pip,
            vo: "xv".to_string(),
            hwdec: "no".to_string(),
            low_video: false,
            cache_mb: 24,
            ao: None,
            verbose: false,
        }
    }
}

/// Qué pedir al cargar un archivo, además de la URL.
///
/// No lleva opciones por archivo (como `start`): mpv 0.38 en adelante rechaza `loadfile` con un
/// mapa de opciones en la posición que la app usaba («invalid parameter»). Para retomar, la app
/// hace un `seek` cuando mpv confirma el archivo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadOptions {
    /// Pista de audio separada (DASH: video y audio por separado).
    pub audio_url: Option<String>,
}

#[derive(Debug)]
pub enum PlayerError {
    /// mpv no está instalado. El mensaje dice cómo instalarlo.
    NotInstalled,
    Io(std::io::Error),
}

impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlayerError::NotInstalled => {
                write!(f, "No se encontró mpv. Instalalo con: sudo apt install mpv")
            }
            PlayerError::Io(e) => write!(f, "Error con mpv: {e}"),
        }
    }
}

impl From<std::io::Error> for PlayerError {
    fn from(e: std::io::Error) -> PlayerError {
        PlayerError::Io(e)
    }
}

/// Propiedades que la app observa para actualizar la barra de controles.
pub const OBSERVED: [&str; 9] = [
    "time-pos",
    "duration",
    "pause",
    "volume",
    "track-list",
    "core-idle",
    "paused-for-cache",
    "fullscreen",
    "demuxer-cache-duration",
];

pub struct Player {
    child: Child,
    socket: PathBuf,
    ipc: Ipc,
    events: Receiver<Event>,
}

impl Player {
    /// Lanza mpv con `options` y se conecta a su socket.
    pub fn spawn(options: &Options) -> Result<Player, PlayerError> {
        let socket = socket_path();
        if let Some(parent) = socket.parent() {
            fs::create_dir_all(parent)?;
        }
        let _ = fs::remove_file(&socket);

        let mut child = Command::new("mpv")
            .args(build_args(options, &socket))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => PlayerError::NotInstalled,
                _ => PlayerError::Io(e),
            })?;
        if let Some(stderr) = child.stderr.take() {
            // mpv escribe avisos por stderr; se leen para que nunca se llene el pipe.
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::debug!("mpv: {line}");
                }
            });
        }

        let (ipc, events) = match Ipc::connect(&socket, Duration::from_secs(5)) {
            Ok(pair) => pair,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(PlayerError::Io(e));
            }
        };
        for (id, name) in OBSERVED.iter().enumerate() {
            ipc.observe(id as u64 + 1, name)?;
        }
        Ok(Player {
            child,
            socket,
            ipc,
            events,
        })
    }

    /// Canal de eventos de mpv. Llamar `try_recv` desde un temporizador de la interfaz.
    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }

    /// `false` si mpv ya se cerró (por ejemplo, desde su ventana). Ese `Player` no sirve más:
    /// hay que lanzar otro.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Carga un archivo o URL, reemplazando lo que se estaba viendo.
    pub fn load(&self, url: &str, options: &LoadOptions) -> std::io::Result<()> {
        // Sin mapa de opciones: esta forma la aceptan todas las versiones de mpv.
        self.ipc
            .command(&[json!("loadfile"), json!(url), json!("replace")])?;
        if let Some(audio) = &options.audio_url {
            self.ipc
                .command(&[json!("audio-add"), json!(audio), json!("select")])?;
        }
        Ok(())
    }

    pub fn set_pause(&self, pause: bool) -> std::io::Result<()> {
        self.ipc
            .command(&[json!("set_property"), json!("pause"), json!(pause)])
    }

    /// Salta `seconds` (negativo para atrás) o va a la posición absoluta.
    pub fn seek(&self, seconds: f64, absolute: bool) -> std::io::Result<()> {
        let mode = if absolute { "absolute" } else { "relative" };
        self.ipc
            .command(&[json!("seek"), json!(seconds), json!(mode)])
    }

    /// Volumen de 0 a 130.
    pub fn set_volume(&self, volume: f64) -> std::io::Result<()> {
        self.ipc.command(&[
            json!("set_property"),
            json!("volume"),
            json!(volume.clamp(0.0, 130.0)),
        ])
    }

    pub fn set_fullscreen(&self, on: bool) -> std::io::Result<()> {
        self.ipc
            .command(&[json!("set_property"), json!("fullscreen"), json!(on)])
    }

    /// Cambia la pista de audio (`aid`). `None` la apaga.
    pub fn set_audio_track(&self, id: Option<i64>) -> std::io::Result<()> {
        let value = id.map(|i| json!(i)).unwrap_or(json!("no"));
        self.ipc
            .command(&[json!("set_property"), json!("aid"), value])
    }

    /// Cambia la pista de subtítulos (`sid`). `None` los apaga.
    pub fn set_subtitle_track(&self, id: Option<i64>) -> std::io::Result<()> {
        let value = id.map(|i| json!(i)).unwrap_or(json!("no"));
        self.ipc
            .command(&[json!("set_property"), json!("sid"), value])
    }

    /// Detiene la reproducción sin cerrar mpv (queda esperando el próximo archivo).
    pub fn stop(&self) -> std::io::Result<()> {
        self.ipc.command(&[json!("stop")])
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.ipc.command(&[json!("quit")]);
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() < Duration::from_secs(1) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        let _ = fs::remove_file(&self.socket);
    }
}

/// Argumentos de mpv según las opciones. Se separa de `spawn` para poder probarlo.
pub fn build_args(options: &Options, socket: &Path) -> Vec<String> {
    let mut args = vec![
        "--idle=yes".to_string(),
        "--title=media-lite".to_string(),
        "--ytdl=no".to_string(),
        "--osc=no".to_string(),
        "--osd-level=1".to_string(),
        "--keep-open=no".to_string(),
        format!(
            "--msg-level={}",
            if options.verbose { "all=v" } else { "all=warn" }
        ),
        format!("--input-ipc-server={}", socket.display()),
        format!("--hwdec={}", options.hwdec),
        "--vd-lavc-skiploopfilter=all".to_string(),
        "--cache=yes".to_string(),
        format!("--demuxer-max-bytes={}MiB", options.cache_mb),
        format!(
            "--demuxer-max-back-bytes={}MiB",
            (options.cache_mb / 3).max(1)
        ),
    ];
    if options.low_video {
        args.push("--framedrop=decoder+vo".to_string());
    } else {
        args.push("--framedrop=vo".to_string());
    }
    if let Some(ao) = &options.ao {
        args.push(format!("--ao={ao}"));
    }
    match options.mode {
        Mode::Pip => {
            args.push("--force-window=no".into());
            args.push("--ontop=yes".into());
            args.push("--no-border".into());
            args.push("--geometry=480x270-16-16".into());
            args.push(format!("--vo={}", options.vo));
        }
        Mode::Video => {
            args.push(format!("--vo={}", options.vo));
        }
        Mode::AudioOnly => {
            args.push("--no-video".into());
        }
    }
    args
}

/// Límite de `sun_path` en Linux (108 bytes, con el terminador). Con margen.
const MAX_SOCKET_PATH: usize = 100;

/// Socket de control: en `$XDG_RUNTIME_DIR/media-lite/` (o la caché) y con el PID en el nombre.
/// Si la ruta es demasiado larga para un socket Unix, se usa `/tmp`.
pub fn socket_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("media-lite"))
        .unwrap_or_else(paths::cache_dir);
    let preferred = base.join(format!("mpv-{}.sock", std::process::id()));
    if preferred.as_os_str().len() < MAX_SOCKET_PATH {
        return preferred;
    }
    std::env::temp_dir().join(format!("media-lite-mpv-{}.sock", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pip_mode_is_floating_and_small() {
        let args = build_args(&Options::default(), Path::new("/tmp/s.sock"));
        assert!(args.contains(&"--ontop=yes".to_string()));
        assert!(args.contains(&"--no-border".to_string()));
        assert!(args.iter().any(|a| a.starts_with("--geometry=480x270")));
        assert!(args.contains(&"--ytdl=no".to_string()));
    }

    #[test]
    fn audio_only_has_no_video() {
        let opts = Options {
            mode: Mode::AudioOnly,
            ..Options::default()
        };
        let args = build_args(&opts, Path::new("/tmp/s.sock"));
        assert!(args.contains(&"--no-video".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--vo=")));
    }

    #[test]
    fn cache_is_bounded() {
        let opts = Options {
            cache_mb: 24,
            ..Options::default()
        };
        let args = build_args(&opts, Path::new("/tmp/s.sock"));
        assert!(args.contains(&"--demuxer-max-bytes=24MiB".to_string()));
        assert!(args.contains(&"--demuxer-max-back-bytes=8MiB".to_string()));
    }

    #[test]
    fn socket_path_is_short_enough_for_unix_sockets() {
        let path = socket_path();
        assert!(path.as_os_str().len() < 108, "{}", path.display());
    }

    #[test]
    fn verbose_asks_mpv_for_details() {
        let quiet = build_args(&Options::default(), Path::new("/tmp/s.sock"));
        assert!(quiet.contains(&"--msg-level=all=warn".to_string()));
        let opts = Options {
            verbose: true,
            ..Options::default()
        };
        let args = build_args(&opts, Path::new("/tmp/s.sock"));
        assert!(args.contains(&"--msg-level=all=v".to_string()));
    }

    #[test]
    fn low_video_skips_decoder_frames() {
        let opts = Options {
            low_video: true,
            ..Options::default()
        };
        let args = build_args(&opts, Path::new("/tmp/s.sock"));
        assert!(args.contains(&"--framedrop=decoder+vo".to_string()));
    }
}
