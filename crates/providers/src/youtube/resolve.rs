//! Reproducción de YouTube: la cascada de clientes de `/player`, la caché de streams y la
//! clasificación de `playabilityStatus`. Portado de `Sanson-FLinux/plugins/youtube/script.js`
//! (`player`, `sources`, `playable`) y de la cascada de yt-dlp.
//!
//! Orden: `visionos` (sin JavaScript) primero. Si no responde en `hedge_ms` o falla, sale el
//! siguiente cliente en paralelo, y se usa el primero que sirva. Nunca se rota IP ni se generan PO
//! tokens; si YouTube pide confirmar que no sos un bot, se avisa (y con cookies se usan otros clientes).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use media_lite_core::http::{self, Agent};
use media_lite_core::store::write_atomic;
use media_lite_core::{Error, ErrorKind, Result, Source};
use media_lite_ytsolver::cache::{Cache, MAX_AGE};
use media_lite_ytsolver::solve::{self, Request, Solutions};
use serde_json::{json, Value};

use super::clients::{Client, Clients};
use super::cookies::{self, Auth};
use super::external;
use super::innertube::Innertube;
use super::player_js;
use super::streams::{self, Format};
use crate::util::now;

/// Cuánto vale una URL resuelta (las de YouTube vencen a las ~6 h).
pub const STREAM_TTL: Duration = Duration::from_secs(5 * 3600);
/// Cada cuánto se revisa qué reproductor usa YouTube.
const PLAYER_TTL: Duration = Duration::from_secs(6 * 3600);
const MAX_JSON: u64 = 16 * 1024 * 1024;
const YT: &str = "https://www.youtube.com";

pub const BOT_MESSAGE: &str = "YouTube pide confirmar que no sos un bot. Pasa con VPN o redes marcadas: desactivá la VPN o cargá tus cookies en Ajustes.";
pub const SOLVER_MESSAGE: &str = "YouTube cambió; actualizá el soporte en Ajustes.";

/// Un video resuelto: sus fuentes, desde cuándo se puede ver y qué cliente lo entregó.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub video_id: String,
    pub sources: Vec<Source>,
    /// Segundos desde la época. Si es posterior a ahora, YouTube hace esperar un anuncio previo.
    pub available_at: u64,
    pub client: String,
    pub live: bool,
    /// Solo el audio (AAC), para la música.
    pub audio: Option<String>,
    pub timing: Timing,
}

/// Cuánto tardó cada paso de la resolución, en milisegundos (para `media-lite-cli yt-resolve --timing`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    /// Preparar el reproductor (solo si no estaba en caché).
    pub prep_ms: u64,
    /// `POST /player`.
    pub player_ms: u64,
    /// El solucionador de `sig` y `n`.
    pub solver_ms: u64,
    pub total_ms: u64,
}

struct Cached {
    resolved: Resolved,
    at: Instant,
}

#[derive(Clone)]
struct Player {
    id: String,
    sts: String,
    at: Instant,
}

pub struct Resolver {
    agent: Agent,
    config_dir: PathBuf,
    clients: Clients,
    innertube: Arc<Innertube>,
    exe: PathBuf,
    cache: Cache,
    cache_dir: PathBuf,
    auth: Option<Auth>,
    player: Mutex<Option<Player>>,
    streams: Mutex<HashMap<String, Cached>>,
    last_ok: Mutex<Option<String>>,
    /// Id del reproductor con el que se resolvió el último video que necesitó el solucionador.
    last_player: Mutex<Option<String>>,
}

impl Resolver {
    /// `cache_root` es la caché de media-lite; el reproductor preparado va en `yt-player/`.
    /// `exe` es el propio binario, que corre el solucionador con `--yt-solver`.
    pub fn new(
        config_dir: &Path,
        cache_root: &Path,
        innertube: Arc<Innertube>,
        exe: PathBuf,
    ) -> Resolver {
        let cache_dir = cache_root.join("yt-player");
        Resolver {
            agent: http::agent(),
            config_dir: config_dir.to_path_buf(),
            clients: Clients::load(config_dir),
            innertube,
            exe,
            cache: Cache::new(&cache_dir),
            cache_dir,
            auth: Auth::load(config_dir),
            player: Mutex::new(None),
            streams: Mutex::new(HashMap::new()),
            last_ok: Mutex::new(None),
            last_player: Mutex::new(None),
        }
    }

    /// Arranque en segundo plano: el reproductor de YouTube preparado y la poda de la caché.
    pub fn warm(&self) {
        if let Err(e) = self.player() {
            log::warn!("YouTube: no se pudo preparar el reproductor: {}", e.message);
        }
        if let Err(e) = self.cache.prune(MAX_AGE) {
            log::warn!("YouTube: no se pudo podar la caché del reproductor: {e}");
        }
    }

    /// Las fuentes de un video: desde la caché si no venció, o con la cascada.
    pub fn resolve(self: &Arc<Self>, video_id: &str) -> Result<Resolved> {
        if !valid_video_id(video_id) {
            return Err(Error::parse("El id del video de YouTube no es válido"));
        }
        if let Some(hit) = self.cached(video_id) {
            return Ok(hit);
        }
        let resolved = match race(self, video_id) {
            Ok(resolved) => resolved,
            // Último recurso: el yt-dlp del usuario, si existe y sirve.
            Err(error) => match self.external(video_id) {
                Some(resolved) => resolved,
                None => return Err(error),
            },
        };
        if let Ok(mut last) = self.last_ok.lock() {
            *last = Some(resolved.client.clone());
        }
        if let Ok(mut streams) = self.streams.lock() {
            streams.insert(
                video_id.to_string(),
                Cached {
                    resolved: resolved.clone(),
                    at: Instant::now(),
                },
            );
        }
        Ok(resolved)
    }

    /// yt-dlp instalado por el usuario (si sirve), como último recurso.
    fn external(&self, video_id: &str) -> Option<Resolved> {
        if !external::available() {
            return None;
        }
        let cookies = cookies::path(&self.config_dir);
        let sources = external::sources(video_id, Some(&cookies)).ok()?;
        let live = sources.first().is_some_and(|s| s.live);
        Some(Resolved {
            video_id: video_id.to_string(),
            available_at: now(),
            sources,
            client: "yt-dlp".into(),
            live,
            audio: None,
            timing: Timing::default(),
        })
    }

    /// Los clientes de la cascada en su orden (con cookies, el de la cuenta).
    pub fn order_keys(&self) -> Vec<String> {
        self.order()
    }

    /// Tira lo guardado de un video (la URL venció o la IP cambió).
    pub fn forget(&self, video_id: &str) {
        if let Ok(mut streams) = self.streams.lock() {
            streams.remove(video_id);
        }
    }

    fn cached(&self, video_id: &str) -> Option<Resolved> {
        let streams = self.streams.lock().ok()?;
        let hit = streams.get(video_id)?;
        (hit.at.elapsed() < STREAM_TTL).then(|| hit.resolved.clone())
    }

    /// Orden de los clientes: con cookies, el de la cuenta; y primero el que anduvo la última vez.
    fn order(&self) -> Vec<String> {
        let cascade = self.clients.cascade();
        let mut order = if self.auth.is_some() {
            cascade.cookies_order.clone()
        } else {
            cascade.order.clone()
        };
        let last = self.last_ok.lock().ok().and_then(|l| l.clone());
        if let Some(last) = last {
            if let Some(pos) = order.iter().position(|k| *k == last) {
                let key = order.remove(pos);
                order.insert(0, key);
            }
        }
        order
    }

    /// El id y el `signatureTimestamp` del reproductor de YouTube. La primera vez baja el
    /// `base.js` y lo prepara (unos segundos); después sale de la caché.
    fn player(&self) -> Result<Player> {
        if let Some(p) = self.player.lock().ok().and_then(|g| g.clone()) {
            if p.at.elapsed() < PLAYER_TTL {
                return Ok(p);
            }
        }
        let id = player_js::current_player_id(&self.agent)?;
        let sts_path = self.cache_dir.join(format!("{id}.sts"));
        let prepared = self.cache_dir.join(format!("{id}.js")).exists();
        let cached_sts = std::fs::read_to_string(&sts_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let sts = match cached_sts {
            Some(sts) if prepared => sts,
            _ => {
                let js = player_js::download(&self.agent, &id)?;
                let sts =
                    player_js::prepare(&self.exe, &self.cache, &id, &js)?.ok_or_else(|| {
                        Error::parse(
                            "No se encontró signatureTimestamp en el reproductor de YouTube",
                        )
                    })?;
                if let Err(e) = write_atomic(&sts_path, sts.as_bytes(), 0o644) {
                    log::warn!("YouTube: no se pudo guardar el signatureTimestamp: {e}");
                }
                sts
            }
        };
        let player = Player {
            id,
            sts,
            at: Instant::now(),
        };
        if let Ok(mut slot) = self.player.lock() {
            *slot = Some(player.clone());
        }
        Ok(player)
    }

    /// Un intento con un cliente: `/player`, la clasificación, el solucionador y los formatos.
    fn attempt(&self, key: &str, video_id: &str) -> Result<Resolved> {
        let started = Instant::now();
        let (body, mut timing) = self.fetch_client(key, video_id)?;
        let mut resolved = self.build(key, video_id, &body, &mut timing)?;
        timing.total_ms = started.elapsed().as_millis() as u64;
        resolved.timing = timing;
        Ok(resolved)
    }

    /// La respuesta cruda de `/player` con un cliente, sin clasificar. Sirve para probar y para
    /// guardar fixtures (`media-lite-cli yt-resolve --dump`).
    pub fn fetch_client(&self, key: &str, video_id: &str) -> Result<(Value, Timing)> {
        let client = self.client(key)?;
        let mut timing = Timing::default();
        let prep = Instant::now();
        let player = if client.needs_js {
            Some(self.player()?)
        } else {
            None
        };
        timing.prep_ms = prep.elapsed().as_millis() as u64;
        let request = Instant::now();
        let body =
            self.player_request(&client, video_id, player.as_ref().map(|p| p.sts.as_str()))?;
        timing.player_ms = request.elapsed().as_millis() as u64;
        if let (Some(p), Ok(mut last)) = (player, self.last_player.lock()) {
            *last = Some(p.id);
        }
        Ok((body, timing))
    }

    /// Las fuentes de una respuesta de `/player`, con el solucionador si el cliente lo necesita.
    pub fn build(
        &self,
        key: &str,
        video_id: &str,
        body: &Value,
        timing: &mut Timing,
    ) -> Result<Resolved> {
        let client = self.client(key)?;
        classify(body, video_id)?;
        let formats = streams::formats(&body["streamingData"]);
        let live = body["videoDetails"]["isLive"].as_bool().unwrap_or(false);
        let hls = body["streamingData"]["hlsManifestUrl"]
            .as_str()
            .map(str::to_string);
        let solved = if client.needs_js && !formats.is_empty() {
            let player_id = self
                .last_player
                .lock()
                .ok()
                .and_then(|l| l.clone())
                .ok_or_else(|| Error::new(ErrorKind::Other, "falta el reproductor de YouTube"))?;
            let started = Instant::now();
            let solved = self.solve(&player_id, &formats)?;
            timing.solver_ms = started.elapsed().as_millis() as u64;
            solved
        } else {
            Solutions::default()
        };
        let usable = streams::playable(&formats, &solved, client.needs_js);
        let mut sources = streams::choose_sources(&usable, hls.as_deref(), live);
        if sources.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "YouTube no entregó formatos que se puedan reproducir.",
            ));
        }
        let available_at = streams::available_at(body, now());
        for source in &mut sources {
            source.available_at = Some(available_at);
        }
        Ok(Resolved {
            video_id: video_id.to_string(),
            sources,
            available_at,
            client: key.to_string(),
            live,
            audio: streams::audio_only_url(&usable),
            timing: *timing,
        })
    }

    fn client(&self, key: &str) -> Result<Client> {
        self.clients.get(key).cloned().ok_or_else(|| {
            Error::new(
                ErrorKind::Other,
                format!("falta el cliente {key} en clients.toml"),
            )
        })
    }

    /// Resuelve los desafíos `sig` y `n` de estos formatos, con una sola llamada al solucionador.
    fn solve(&self, player_id: &str, formats: &[Format]) -> Result<Solutions> {
        let usable: Vec<&Format> = formats.iter().filter(|f| !f.drm).collect();
        let (sig, n) = streams::challenges(&usable);
        if sig.is_empty() && n.is_empty() {
            return Ok(Solutions::default());
        }
        let request = Request {
            player_id,
            player_js: None,
            n: &n,
            sig: &sig,
        };
        solve::solve(&self.exe, &self.cache, &request).map_err(|e| {
            log::warn!("solucionador de YouTube: {e}");
            Error::new(ErrorKind::Solver, SOLVER_MESSAGE)
        })
    }

    /// `POST /youtubei/v1/player` con el cliente pedido. Con cookies, va firmado con SAPISIDHASH.
    fn player_request(&self, client: &Client, video_id: &str, sts: Option<&str>) -> Result<Value> {
        let visitor = self.innertube.visitor();
        let mut context = json!({
            "clientName": client.name,
            "clientVersion": client.version,
            "hl": "es",
            "visitorData": visitor,
        });
        if let Some(v) = &client.device_make {
            context["deviceMake"] = json!(v);
        }
        if let Some(v) = &client.device_model {
            context["deviceModel"] = json!(v);
        }
        if let Some(v) = &client.os_name {
            context["osName"] = json!(v);
        }
        if let Some(v) = &client.os_version {
            context["osVersion"] = json!(v);
        }
        let mut body = json!({
            "context": { "client": context },
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true,
        });
        if let Some(embed) = &client.embed_url {
            body["context"]["client"]["clientScreen"] = json!("EMBED");
            body["context"]["thirdParty"] = json!({ "embedUrl": embed });
        }
        if let Some(sts) = sts.and_then(|s| s.parse::<u64>().ok()) {
            body["playbackContext"] = json!({
                "contentPlaybackContext": { "html5Preference": "HTML5_PREF_WANTS", "signatureTimestamp": sts }
            });
        }

        let mut headers: Vec<(String, String)> = vec![
            ("Content-Type".into(), "application/json".into()),
            (
                "User-Agent".into(),
                client
                    .user_agent
                    .clone()
                    .unwrap_or_else(|| http::USER_AGENT.to_string()),
            ),
            ("X-YouTube-Client-Name".into(), client.number.to_string()),
            ("X-YouTube-Client-Version".into(), client.version.clone()),
            ("X-Goog-Visitor-Id".into(), visitor),
            ("Origin".into(), YT.into()),
            ("X-Origin".into(), YT.into()),
        ];
        if let Some(auth) = &self.auth {
            headers.push(("Cookie".into(), auth.cookie_header.clone()));
            headers.push(("X-Goog-AuthUser".into(), "0".into()));
            headers.push((
                "Authorization".into(),
                cookies::sapisid_hash(&auth.sapisid, YT, now()),
            ));
        }
        let refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let url = format!("{YT}/youtubei/v1/player?prettyPrint=false");
        let text = body.to_string();
        let response = http::request(&self.agent, "POST", &url, &refs, Some(&text), MAX_JSON)?;
        if !response.ok() {
            return Err(Error::network(format!(
                "YouTube respondió con error {}",
                response.status
            )));
        }
        serde_json::from_slice(&response.body)
            .map_err(|_| Error::parse("YouTube: la respuesta del reproductor no se entiende"))
    }
}

/// Lanza los clientes en orden: el primero, y el siguiente si no responde en `hedge_ms` o si uno
/// falla. Se usa el primero que sirva.
fn race(this: &Arc<Resolver>, video_id: &str) -> Result<Resolved> {
    let order = this.order();
    if order.is_empty() {
        return Err(Error::new(
            ErrorKind::Other,
            "clients.toml no tiene cascada",
        ));
    }
    let hedge = Duration::from_millis(this.clients.cascade().hedge_ms);
    let (tx, rx) = mpsc::channel::<Result<Resolved>>();
    let mut next = 0;
    let mut running = 0;
    let mut errors: Vec<Error> = Vec::new();

    let start = |next: &mut usize, running: &mut usize| {
        let key = order[*next].clone();
        *next += 1;
        *running += 1;
        let me = Arc::clone(this);
        let id = video_id.to_string();
        let tx = tx.clone();
        let spawned = thread::Builder::new()
            .name(format!("yt-{key}"))
            .spawn(move || {
                let _ = tx.send(me.attempt(&key, &id));
            });
        if spawned.is_err() {
            log::warn!("YouTube: no se pudo lanzar el cliente");
        }
    };

    start(&mut next, &mut running);
    loop {
        if running == 0 && next >= order.len() {
            break;
        }
        let wait = if next < order.len() {
            hedge
        } else {
            Duration::from_secs(120)
        };
        match rx.recv_timeout(wait) {
            Ok(Ok(resolved)) => return Ok(resolved),
            Ok(Err(e)) => {
                running -= 1;
                log::debug!("YouTube: cliente sin resultado: {}", e.message);
                errors.push(e);
                if next < order.len() {
                    start(&mut next, &mut running);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if next < order.len() {
                    start(&mut next, &mut running);
                } else {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Err(best_error(errors, this.auth.is_some()))
}

/// El error que se muestra cuando ningún cliente sirvió.
fn best_error(errors: Vec<Error>, logged_in: bool) -> Error {
    if !logged_in && errors.iter().any(|e| e.kind == ErrorKind::BotCheck) {
        return Error::new(ErrorKind::BotCheck, BOT_MESSAGE);
    }
    if let Some(e) = errors.iter().find(|e| e.kind == ErrorKind::Solver) {
        return e.clone();
    }
    if let Some(e) = errors
        .iter()
        .find(|e| matches!(e.kind, ErrorKind::Unavailable | ErrorKind::Login))
    {
        return e.clone();
    }
    errors
        .into_iter()
        .next()
        .unwrap_or_else(|| Error::network("YouTube no respondió"))
}

/// Un id de video de YouTube: 11 caracteres de letras, números, `-` y `_`.
pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Lee `playabilityStatus` y el video que respondió. Un video distinto al pedido es un error.
pub fn classify(body: &Value, video_id: &str) -> Result<()> {
    if let Some(answered) = body["videoDetails"]["videoId"].as_str() {
        if answered != video_id {
            return Err(Error::parse("YouTube respondió con otro video"));
        }
    }
    let status = body["playabilityStatus"]["status"].as_str().unwrap_or("");
    let reason = body["playabilityStatus"]["reason"].as_str().unwrap_or("");
    match status {
        "OK" => Ok(()),
        "LOGIN_REQUIRED" if reason.to_lowercase().contains("bot") => {
            Err(Error::new(ErrorKind::BotCheck, reason))
        }
        "LOGIN_REQUIRED" => Err(Error::new(
            ErrorKind::Login,
            "Este video pide iniciar sesión (edad).",
        )),
        "UNPLAYABLE" if reason.is_empty() => Err(Error::new(
            ErrorKind::Unavailable,
            "Video no disponible en tu país.",
        )),
        "UNPLAYABLE" => Err(Error::new(ErrorKind::Unavailable, reason)),
        _ => Err(Error::new(
            ErrorKind::Unavailable,
            if reason.is_empty() {
                "Video no disponible."
            } else {
                reason
            },
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ids_are_eleven_url_safe_characters() {
        assert!(valid_video_id("dQw4w9WgXcQ"));
        assert!(!valid_video_id("../etc/passwd"));
        assert!(!valid_video_id("short"));
    }

    #[test]
    fn bot_check_is_its_own_error_and_age_gate_is_login() {
        let bot = json!({ "playabilityStatus": { "status": "LOGIN_REQUIRED", "reason": "Sign in to confirm you're not a bot" } });
        assert_eq!(
            classify(&bot, "dQw4w9WgXcQ").unwrap_err().kind,
            ErrorKind::BotCheck
        );
        let age = json!({ "playabilityStatus": { "status": "LOGIN_REQUIRED", "reason": "Verifica tu edad" } });
        assert_eq!(
            classify(&age, "dQw4w9WgXcQ").unwrap_err().kind,
            ErrorKind::Login
        );
    }

    #[test]
    fn other_video_in_the_answer_is_rejected() {
        let body = json!({ "videoDetails": { "videoId": "AAAAAAAAAAA" }, "playabilityStatus": { "status": "OK" } });
        assert!(classify(&body, "dQw4w9WgXcQ").is_err());
        let ok = json!({ "videoDetails": { "videoId": "dQw4w9WgXcQ" }, "playabilityStatus": { "status": "OK" } });
        assert!(classify(&ok, "dQw4w9WgXcQ").is_ok());
    }

    #[test]
    fn unplayable_without_reason_says_not_in_your_country() {
        let body = json!({ "playabilityStatus": { "status": "UNPLAYABLE" } });
        let e = classify(&body, "dQw4w9WgXcQ").unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unavailable);
        assert!(e.message.contains("país"));
    }

    #[test]
    fn bot_check_wins_when_no_cookies_are_loaded() {
        let errors = vec![
            Error::new(ErrorKind::Unavailable, "otro"),
            Error::new(ErrorKind::BotCheck, "bot"),
        ];
        let e = best_error(errors, false);
        assert_eq!(e.kind, ErrorKind::BotCheck);
        assert_eq!(e.message, BOT_MESSAGE);
    }

    /// Una respuesta de `/player` sintética (no es de YouTube): para probar el armado de fuentes.
    fn synthetic_visionos(video_id: &str) -> Value {
        json!({
            "videoDetails": { "videoId": video_id, "isLive": false },
            "playabilityStatus": { "status": "OK" },
            "streamingData": {
                "hlsManifestUrl": "https://manifest.example/master.m3u8",
                "adaptiveFormats": [
                    { "itag": 136, "mimeType": "video/mp4; codecs=\"avc1.4d401f\"", "url": "https://cdn.example/136?expire=1", "height": 720, "bitrate": 900000 },
                    { "itag": 135, "mimeType": "video/mp4; codecs=\"avc1.4d401e\"", "url": "https://cdn.example/135?expire=1", "height": 480, "bitrate": 500000 },
                    { "itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "url": "https://cdn.example/140", "bitrate": 128000 },
                    { "itag": 247, "mimeType": "video/webm; codecs=\"vp9\"", "url": "https://cdn.example/247", "height": 720, "bitrate": 999999 }
                ]
            }
        })
    }

    #[test]
    fn visionos_answer_becomes_dash_sources_without_the_solver() {
        let resolver = test_resolver();
        let body = synthetic_visionos("dQw4w9WgXcQ");
        let resolved = resolver
            .build("visionos", "dQw4w9WgXcQ", &body, &mut Timing::default())
            .unwrap();
        let heights: Vec<u32> = resolved.sources.iter().filter_map(|s| s.height).collect();
        assert_eq!(heights, vec![720, 480]);
        assert!(resolved
            .sources
            .iter()
            .all(|s| s.audio_url.as_deref() == Some("https://cdn.example/140")));
        assert!(!resolved.live);
        assert_eq!(resolved.client, "visionos");
    }

    #[test]
    fn a_pre_roll_ad_sets_when_the_video_can_start() {
        let resolver = test_resolver();
        let mut body = synthetic_visionos("dQw4w9WgXcQ");
        body["adPlacements"] = json!([{ "adPlacementRenderer": {
            "config": { "adPlacementConfig": { "kind": "AD_PLACEMENT_KIND_START" } },
            "renderer": { "instreamVideoAdRenderer": { "playerVars": "length_seconds=6" } }
        }}]);
        let resolved = resolver
            .build("visionos", "dQw4w9WgXcQ", &body, &mut Timing::default())
            .unwrap();
        assert!(resolved.available_at >= now() + 5);
        assert!(resolved
            .sources
            .iter()
            .all(|s| s.available_at == Some(resolved.available_at)));
    }

    #[test]
    fn a_wrong_video_or_a_bot_check_never_becomes_sources() {
        let resolver = test_resolver();
        let body = synthetic_visionos("AAAAAAAAAAA");
        assert!(resolver
            .build("visionos", "dQw4w9WgXcQ", &body, &mut Timing::default())
            .is_err());
        let bot = json!({ "playabilityStatus": { "status": "LOGIN_REQUIRED", "reason": "not a bot check" } });
        let e = resolver
            .build("visionos", "dQw4w9WgXcQ", &bot, &mut Timing::default())
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::BotCheck);
    }

    fn test_resolver() -> Resolver {
        let root = std::env::temp_dir().join(format!("ml-resolver-test-{}", std::process::id()));
        let config = root.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let api = Arc::new(Innertube::new(&config));
        Resolver::new(
            &config,
            &root.join("cache"),
            api,
            PathBuf::from("/nonexistent"),
        )
    }

    #[test]
    fn solver_failure_says_youtube_changed() {
        let e = best_error(vec![Error::new(ErrorKind::Solver, SOLVER_MESSAGE)], false);
        assert_eq!(e.message, SOLVER_MESSAGE);
    }
}
