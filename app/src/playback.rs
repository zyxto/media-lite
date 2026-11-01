//! Reproducción: mpv en su propia ventana, controlado desde la barra de la app. El historial
//! guarda la posición para retomar el video donde quedó, y si la imagen se traba la app vuelve a
//! cargar el video en la misma posición, con una calidad menor.

use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use media_lite_core::history::{self, Entry};
use media_lite_core::store::Settings;
use media_lite_core::{Details, Item, Platform, Result, Source};
use media_lite_player::{Event, Filters, LoadOptions, Mode, Options, Player};
use serde_json::Value;

use crate::state::{set_status, with_app, App};

/// Cada cuántos segundos de reproducción se guarda la posición.
const SAVE_EVERY_SECS: f64 = 10.0;
/// Caché de red de mpv, en MB. Acota la RAM del reproductor.
const CACHE_MB: u32 = 24;
/// Si la imagen se queda esperando datos más de esto, se vuelve a cargar.
const STALL_LIMIT: f64 = 20.0;
/// Entre dos recargas tiene que pasar al menos este tiempo.
const RELOAD_COOLDOWN: f64 = 30.0;
/// Alturas de calidad, de mayor a menor, para bajar un escalón cuando se traba.
const QUALITY_STEPS: [u32; 5] = [1080, 720, 480, 360, 240];

/// Estado de lo que se está reproduciendo, según lo que informa mpv.
#[derive(Clone, Debug)]
pub struct Playback {
    pub platform: Platform,
    pub id: String,
    pub title: String,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub buffering: bool,
    pub fullscreen: bool,
    pub audio: Vec<(i64, String)>,
    pub audio_selected: Option<i64>,
    pub subs: Vec<(i64, String)>,
    pub sub_selected: Option<i64>,
    /// Posición de la última vez que se guardó en el historial.
    pub saved_at: f64,
    /// Fuente que se está viendo, para poder volver a cargarla.
    pub source: Source,
    /// Altura máxima con la que se cargó (baja si la imagen se traba).
    pub max_height: u32,
    /// Desde cuándo la imagen está esperando datos.
    pub stall_since: Option<Instant>,
    /// Última recarga por traba.
    pub last_reload: Option<Instant>,
}

impl Playback {
    fn new(item: &Item, position: f64, source: &Source, max_height: u32) -> Playback {
        Playback {
            platform: item.platform,
            id: item.id.clone(),
            title: item.title.clone(),
            position,
            duration: item.duration_secs.unwrap_or(0) as f64,
            paused: false,
            buffering: true,
            fullscreen: false,
            audio: Vec::new(),
            audio_selected: None,
            subs: Vec::new(),
            sub_selected: None,
            saved_at: position,
            source: source.clone(),
            max_height,
            stall_since: None,
            last_reload: None,
        }
    }
}

/// Pide los detalles del elemento (en un hilo de trabajo) y arranca su mejor fuente.
pub fn request(app: &Rc<App>, item: Item) {
    let Some(provider) = app
        .providers
        .iter()
        .find(|p| p.platform() == item.platform)
        .cloned()
    else {
        return;
    };
    set_status(app, "Preparando reproducción…");
    app.pool.run(move || {
        let result = provider.details(&item);
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|app| begin(app, item, result));
        });
    });
}

fn begin(app: &Rc<App>, item: Item, result: Result<Details>) {
    match result {
        Ok(details) => {
            let quality = app.settings.borrow().quality;
            match choose_source(&details.sources, quality) {
                Some(source) => start(app, &item, &source),
                None => set_status(app, "Este título no tiene fuentes reproducibles todavía."),
            }
        }
        Err(e) => set_status(app, e.to_string()),
    }
}

/// URL que mpv abre para una fuente: las HLS pasan por el proxy, con la calidad pedida.
fn url_for(app: &App, source: &Source, max_height: u32) -> String {
    match (&app.proxy, source.url.contains(".m3u8")) {
        (Some(proxy), true) => proxy.url_for(
            &source.url,
            &Filters {
                max_height: Some(max_height),
                drop: Vec::new(),
            },
        ),
        _ => source.url.clone(),
    }
}

fn start(app: &Rc<App>, item: &Item, source: &Source) {
    let quality = app.settings.borrow().quality;
    let url = url_for(app, source, quality);

    if app.player.borrow().is_none() {
        let options = options_for(&app.settings.borrow());
        match Player::spawn(&options) {
            Ok(player) => *app.player.borrow_mut() = Some(player),
            Err(e) => {
                set_status(app, e.to_string());
                return;
            }
        }
    }

    let resume = history::find(&app.history.borrow(), item.platform, &item.id)
        .and_then(history::resume_point);
    let load = LoadOptions {
        start: resume,
        headers: Vec::new(),
        audio_url: source.audio_url.clone(),
    };
    let loaded = match app.player.borrow().as_ref() {
        Some(player) => player.load(&url, &load),
        None => return,
    };
    match loaded {
        Ok(()) => {
            *app.playback.borrow_mut() =
                Some(Playback::new(item, resume.unwrap_or(0.0), source, quality));
            set_status(app, format!("Reproduciendo «{}»", item.title));
            sync_ui(app);
        }
        Err(e) => set_status(app, format!("No se pudo abrir el video: {e}")),
    }
}

/// Lo llama un temporizador de la interfaz: lee los eventos de mpv, revisa si se trabó y refresca la barra.
pub fn pump(app: &Rc<App>) {
    let events: Vec<Event> = match app.player.borrow().as_ref() {
        Some(player) => player.events().try_iter().collect(),
        None => Vec::new(),
    };
    for event in events {
        apply(app, event);
    }
    check_stall(app);
    maybe_save(app, false);
    sync_ui(app);
}

fn apply(app: &Rc<App>, event: Event) {
    match event {
        Event::Property { name, value } => {
            let mut guard = app.playback.borrow_mut();
            let Some(pb) = guard.as_mut() else {
                return;
            };
            match name.as_str() {
                "time-pos" => pb.position = value.as_f64().unwrap_or(pb.position),
                "duration" => pb.duration = value.as_f64().unwrap_or(pb.duration),
                "pause" => pb.paused = value.as_bool().unwrap_or(pb.paused),
                "core-idle" | "paused-for-cache" => {
                    let waiting = value.as_bool().unwrap_or(false);
                    pb.buffering = waiting;
                    match (waiting, pb.stall_since) {
                        (true, None) => pb.stall_since = Some(Instant::now()),
                        (false, _) => pb.stall_since = None,
                        _ => {}
                    }
                }
                "fullscreen" => pb.fullscreen = value.as_bool().unwrap_or(false),
                "track-list" => {
                    let (audio, audio_selected) = tracks(&value, "audio");
                    let (subs, sub_selected) = tracks(&value, "sub");
                    pb.audio = audio;
                    pb.audio_selected = audio_selected;
                    pb.subs = subs;
                    pb.sub_selected = sub_selected;
                }
                _ => {}
            }
        }
        Event::FileLoaded => {
            if let Some(pb) = app.playback.borrow_mut().as_mut() {
                pb.buffering = false;
                pb.stall_since = None;
            }
        }
        Event::EndFile { reason } => match reason.as_str() {
            // Un cambio de archivo (`loadfile replace`) también avisa `stop`: no es el fin.
            "eof" => finish(app, true),
            "error" => {
                set_status(app, "Se cortó la reproducción.");
                finish(app, false);
            }
            "quit" => finish(app, false),
            _ => {}
        },
    }
}

/// Si la imagen lleva demasiado esperando datos, vuelve a cargar el video en la misma posición
/// con una calidad menor. Así un equipo lento termina viendo algo fluido en vez de congelado.
fn check_stall(app: &Rc<App>) {
    let reload = {
        let guard = app.playback.borrow();
        let Some(pb) = guard.as_ref() else {
            return;
        };
        let stalled = pb
            .stall_since
            .is_some_and(|t| t.elapsed().as_secs_f64() >= STALL_LIMIT);
        let cooled = pb
            .last_reload
            .is_none_or(|t| t.elapsed().as_secs_f64() >= RELOAD_COOLDOWN);
        if !(stalled && cooled) {
            return;
        }
        (pb.source.clone(), pb.position, lower_quality(pb.max_height))
    };
    let (source, position, quality) = reload;
    log::info!("la imagen se trabó: se vuelve a cargar a {quality}p");
    let url = url_for(app, &source, quality);
    let load = LoadOptions {
        start: Some(position),
        headers: Vec::new(),
        audio_url: source.audio_url.clone(),
    };
    if let Some(player) = app.player.borrow().as_ref() {
        let _ = player.load(&url, &load);
    }
    if let Some(pb) = app.playback.borrow_mut().as_mut() {
        pb.max_height = quality;
        pb.stall_since = None;
        pb.last_reload = Some(Instant::now());
        pb.buffering = true;
    }
    set_status(app, format!("Se trabó la imagen: ahora en {quality}p"));
}

/// Siguiente escalón de calidad hacia abajo. Si ya está en el mínimo, se queda igual.
pub fn lower_quality(current: u32) -> u32 {
    QUALITY_STEPS
        .iter()
        .copied()
        .find(|&step| step < current)
        .unwrap_or(current)
}

/// Termina la reproducción actual: guarda el historial y limpia la barra.
fn finish(app: &Rc<App>, completed: bool) {
    maybe_save(app, completed);
    *app.playback.borrow_mut() = None;
    sync_ui(app);
}

/// Guarda la posición en el historial, si ya pasó el intervalo (o siempre, si `force`).
fn maybe_save(app: &Rc<App>, force: bool) {
    let snapshot = {
        let mut guard = app.playback.borrow_mut();
        match guard.as_mut() {
            Some(pb) if force || (pb.position - pb.saved_at).abs() >= SAVE_EVERY_SECS => {
                pb.saved_at = pb.position;
                Some(pb.clone())
            }
            _ => None,
        }
    };
    let Some(pb) = snapshot else {
        return;
    };
    let position = if force && pb.duration > 0.0 && pb.position >= pb.duration - 1.0 {
        pb.duration
    } else {
        pb.position
    };
    let entry = Entry {
        platform: pb.platform,
        id: pb.id,
        title: pb.title,
        position,
        duration: pb.duration,
        at: now(),
    };
    let mut list = app.history.borrow_mut();
    history::upsert(&mut list, entry);
    if let Err(e) = app.store.save_list("history.json", &list[..]) {
        log::warn!("no se pudo guardar el historial: {e}");
    }
}

/// Pausa o sigue.
pub fn toggle_pause(app: &Rc<App>) {
    let paused = match app.playback.borrow().as_ref() {
        Some(pb) => pb.paused,
        None => return,
    };
    if let Some(player) = app.player.borrow().as_ref() {
        let _ = player.set_pause(!paused);
    }
    if let Some(pb) = app.playback.borrow_mut().as_mut() {
        pb.paused = !paused;
    }
    sync_ui(app);
}

/// Salta `seconds` (negativo: atrás).
pub fn seek_relative(app: &Rc<App>, seconds: f64) {
    if let Some(player) = app.player.borrow().as_ref() {
        let _ = player.seek(seconds, false);
    }
}

/// Va a una fracción (0 a 1) de la duración.
pub fn seek_ratio(app: &Rc<App>, ratio: f64) {
    let duration = match app.playback.borrow().as_ref() {
        Some(pb) => pb.duration,
        None => return,
    };
    if duration > 0.0 {
        if let Some(player) = app.player.borrow().as_ref() {
            let _ = player.seek(ratio.clamp(0.0, 1.0) * duration, true);
        }
    }
}

/// Cierra la reproducción y la ventana de mpv.
pub fn stop(app: &Rc<App>) {
    maybe_save(app, true);
    *app.playback.borrow_mut() = None;
    *app.player.borrow_mut() = None;
    sync_ui(app);
}

pub fn toggle_fullscreen(app: &Rc<App>) {
    let fullscreen = match app.playback.borrow().as_ref() {
        Some(pb) => pb.fullscreen,
        None => return,
    };
    if let Some(player) = app.player.borrow().as_ref() {
        let _ = player.set_fullscreen(!fullscreen);
    }
}

/// Pasa a la siguiente pista de audio.
pub fn cycle_audio(app: &Rc<App>) {
    let next = match app.playback.borrow().as_ref() {
        Some(pb) if !pb.audio.is_empty() => {
            let ids: Vec<i64> = pb.audio.iter().map(|t| t.0).collect();
            next_in_cycle(&ids, pb.audio_selected)
        }
        _ => return,
    };
    if let (Some(id), Some(player)) = (next, app.player.borrow().as_ref()) {
        let _ = player.set_audio_track(Some(id));
    }
}

/// Pasa a la siguiente pista de subtítulos, con «apagados» al final.
pub fn cycle_subtitles(app: &Rc<App>) {
    let (options, current) = match app.playback.borrow().as_ref() {
        Some(pb) if !pb.subs.is_empty() => {
            let mut options: Vec<Option<i64>> = vec![None];
            options.extend(pb.subs.iter().map(|t| Some(t.0)));
            (options, pb.sub_selected)
        }
        _ => return,
    };
    let next = next_in_cycle(&options, Some(current));
    if let Some(player) = app.player.borrow().as_ref() {
        let _ = player.set_subtitle_track(next.flatten());
    }
}

/// Siguiente valor de `options` después de `current`, dando la vuelta al final.
fn next_in_cycle<T: PartialEq + Copy>(options: &[T], current: Option<T>) -> Option<T> {
    if options.is_empty() {
        return None;
    }
    let position = current.and_then(|c| options.iter().position(|o| *o == c));
    let next = match position {
        Some(i) => (i + 1) % options.len(),
        None => 0,
    };
    Some(options[next])
}

/// Actualiza la barra de controles con el estado actual.
pub fn sync_ui(app: &Rc<App>) {
    let Some(window) = app.weak.upgrade() else {
        return;
    };
    let playback = app.playback.borrow();
    match playback.as_ref() {
        Some(pb) => {
            window.set_playing(true);
            window.set_play_title(pb.title.as_str().into());
            window.set_play_time(format!("{} / {}", clock(pb.position), clock(pb.duration)).into());
            let progress = if pb.duration > 0.0 {
                (pb.position / pb.duration).clamp(0.0, 1.0)
            } else {
                0.0
            };
            window.set_play_progress(progress as f32);
            window.set_play_paused(pb.paused);
            let status = if pb.buffering {
                "Cargando…"
            } else if pb.paused {
                "En pausa"
            } else {
                "Reproduciendo"
            };
            window.set_play_status(status.into());
        }
        None => {
            window.set_playing(false);
        }
    }
}

/// Elige la fuente de calidad más alta que no pase de `quality`. Sin alturas, la primera.
pub fn choose_source(sources: &[Source], quality: u32) -> Option<Source> {
    sources
        .iter()
        .filter(|s| s.height.is_some_and(|h| h <= quality))
        .max_by_key(|s| s.height)
        .or_else(|| {
            sources
                .iter()
                .filter(|s| s.height.is_some())
                .min_by_key(|s| s.height)
        })
        .or_else(|| sources.first())
        .cloned()
}

fn options_for(settings: &Settings) -> Options {
    Options {
        mode: Mode::Pip,
        vo: settings.vo.clone(),
        hwdec: settings.hwdec.clone(),
        low_video: settings.low_video,
        cache_mb: CACHE_MB,
        ao: None,
    }
}

/// Pistas de un tipo (`audio` o `sub`) de la lista que informa mpv, y cuál está activa.
fn tracks(list: &Value, kind: &str) -> (Vec<(i64, String)>, Option<i64>) {
    let mut out = Vec::new();
    let mut selected = None;
    for track in list.as_array().into_iter().flatten() {
        if track.get("type").and_then(Value::as_str) != Some(kind) {
            continue;
        }
        let Some(id) = track.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let lang = track.get("lang").and_then(Value::as_str).unwrap_or("");
        let title = track.get("title").and_then(Value::as_str).unwrap_or("");
        let label = match (lang, title) {
            ("", "") => format!("Pista {id}"),
            (l, "") => l.to_string(),
            ("", t) => t.to_string(),
            (l, t) => format!("{l} · {t}"),
        };
        if track.get("selected").and_then(Value::as_bool) == Some(true) {
            selected = Some(id);
        }
        out.push((id, label));
    }
    (out, selected)
}

/// `m:ss` o `h:mm:ss`.
pub fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(height: Option<u32>, url: &str) -> Source {
        Source {
            url: url.into(),
            label: String::new(),
            height,
            live: false,
            audio_url: None,
        }
    }

    #[test]
    fn picks_best_source_within_quality() {
        let sources = [
            source(Some(1080), "a"),
            source(Some(480), "b"),
            source(Some(360), "c"),
        ];
        assert_eq!(choose_source(&sources, 480).unwrap().url, "b");
        assert_eq!(choose_source(&sources, 2000).unwrap().url, "a");
    }

    #[test]
    fn falls_back_to_smallest_when_all_too_big() {
        let sources = [source(Some(1080), "a"), source(Some(720), "b")];
        assert_eq!(choose_source(&sources, 360).unwrap().url, "b");
    }

    #[test]
    fn unknown_heights_use_first() {
        let sources = [source(None, "x"), source(None, "y")];
        assert_eq!(choose_source(&sources, 480).unwrap().url, "x");
        assert!(choose_source(&[], 480).is_none());
    }

    #[test]
    fn cycle_wraps_around() {
        assert_eq!(next_in_cycle(&[1, 2, 3], Some(3)), Some(1));
        assert_eq!(next_in_cycle(&[1, 2, 3], Some(1)), Some(2));
        assert_eq!(next_in_cycle(&[1, 2, 3], None), Some(1));
        assert_eq!(next_in_cycle::<i32>(&[], None), None);
    }

    #[test]
    fn clock_formats() {
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(clock(-3.0), "0:00");
    }

    #[test]
    fn quality_steps_down_to_the_minimum() {
        assert_eq!(lower_quality(480), 360);
        assert_eq!(lower_quality(720), 480);
        assert_eq!(lower_quality(240), 240);
        assert_eq!(lower_quality(1000), 720);
    }

    #[test]
    fn reads_tracks_of_a_kind() {
        let list: Value = serde_json::from_str(
            r#"[{"type":"video","id":1},{"type":"audio","id":2,"lang":"es","selected":true},{"type":"audio","id":3,"title":"Comentario"}]"#,
        )
        .unwrap();
        let (audio, selected) = tracks(&list, "audio");
        assert_eq!(
            audio,
            vec![(2, "es".to_string()), (3, "Comentario".to_string())]
        );
        assert_eq!(selected, Some(2));
    }
}
