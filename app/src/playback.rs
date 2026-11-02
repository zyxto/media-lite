//! Reproducción: mpv en su propia ventana, controlado desde la barra de la app.
//!
//! - El historial guarda la posición para retomar el video donde quedó.
//! - Si la imagen se traba, se vuelve a cargar en la misma posición con una calidad menor.
//! - La cola es la lista de la que se abrió el video: anterior y siguiente recorren esa lista.
//!   Al terminar, cuenta cinco segundos y pasa al siguiente.
//! - El idioma de audio elegido sigue en el próximo episodio de la cola.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use media_lite_core::history::{self, Entry};
use media_lite_core::store::Settings;
use media_lite_core::{Details, Item, Kind, Platform, Result, Source};
use media_lite_player::{Event, Filters, LoadOptions, Mode, Options, Player};
use media_lite_providers::text::lang_name;
use serde_json::Value;

use crate::state::{self, set_error, set_status, with_app, App};

/// Cada cuántos segundos de reproducción se guarda la posición.
const SAVE_EVERY_SECS: f64 = 10.0;
/// Caché de red de mpv, en MB. Acota la RAM del reproductor.
const CACHE_MB: u32 = 24;
/// Si la imagen se queda esperando datos más de esto, se vuelve a cargar.
const STALL_LIMIT: f64 = 20.0;
/// Antes de que mpv confirme el archivo se espera más: abrir el stream puede tardar y bajar la calidad no lo acelera.
const LOAD_LIMIT: f64 = 45.0;
/// Entre dos recargas tiene que pasar al menos este tiempo.
const RELOAD_COOLDOWN: f64 = 30.0;
/// Segundos de cuenta regresiva antes del siguiente elemento de la cola.
const NEXT_COUNTDOWN: Duration = Duration::from_secs(5);
/// Alturas de calidad, de mayor a menor, para bajar un escalón cuando se traba.
const QUALITY_STEPS: [u32; 5] = [1080, 720, 480, 360, 240];

/// Estado de lo que se está reproduciendo, según lo que informa mpv.
#[derive(Clone, Debug)]
pub struct Playback {
    /// El elemento completo que se reproduce.
    pub item: Item,
    pub platform: Platform,
    pub id: String,
    pub title: String,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub buffering: bool,
    /// `core-idle` de mpv. Ojo: también vale `true` en pausa, así que por sí solo no significa «esperando datos».
    pub core_idle: bool,
    /// `paused-for-cache` de mpv: se quedó sin datos y espera la red.
    pub cache_wait: bool,
    pub fullscreen: bool,
    pub audio: Vec<(i64, String)>,
    pub audio_selected: Option<i64>,
    /// Idioma de la pista de audio activa, para recordarlo en el próximo elemento.
    pub audio_lang: Option<String>,
    /// Idioma de audio que hay que buscar en cuanto mpv informe las pistas.
    pub carry_audio: Option<String>,
    /// Pista de audio que se aplica en el próximo ciclo (pendiente de `track-list`).
    pub pending_audio: Option<i64>,
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
    /// Última vez que la posición avanzó (o se reanudó). Si no avanza sin pausa, la imagen está trabada,
    /// aunque mpv no informe que espera datos.
    pub last_progress: Instant,
    /// Posición en la que se marcó `last_progress`. El progreso se mide contra ella y no contra el
    /// evento anterior: mpv avisa `time-pos` en cada cuadro (~0,04 s), que sola no pasa ningún umbral.
    pub progress_pos: f64,
    /// Lista de la que se abrió el video.
    pub queue: Vec<Item>,
    pub queue_index: usize,
    /// Terminó el video: la cuenta regresiva para el siguiente elemento corre desde aquí.
    pub countdown: Option<Instant>,
    /// mpv ya cargó el archivo al menos una vez. Antes de eso el estado es «conectando».
    pub started: bool,
    /// Segundos de video ya bajados por delante de la reproducción (`demuxer-cache-duration`).
    pub buffer_secs: Option<f64>,
    /// Posición a la que saltar cuando mpv confirme el archivo: para retomar, o para seguir tras una recarga.
    pub pending_seek: Option<f64>,
    /// Volumen de mpv, en porcentaje (100 por defecto).
    pub volume: f64,
    /// Anuncios de Twitch que el proxy había quitado la última vez que se miró, y cuándo fue la última quita.
    pub ads_seen: u64,
    pub ads_at: Option<Instant>,
}

impl Playback {
    fn new(
        item: &Item,
        position: f64,
        source: &Source,
        max_height: u32,
        queue: Vec<Item>,
        index: usize,
        carry: Option<String>,
    ) -> Playback {
        Playback {
            item: item.clone(),
            platform: item.platform,
            id: item.id.clone(),
            title: item.title.clone(),
            position,
            duration: item.duration_secs.unwrap_or(0) as f64,
            paused: false,
            buffering: true,
            core_idle: false,
            cache_wait: false,
            fullscreen: false,
            audio: Vec::new(),
            audio_selected: None,
            audio_lang: None,
            carry_audio: carry,
            pending_audio: None,
            subs: Vec::new(),
            sub_selected: None,
            saved_at: position,
            source: source.clone(),
            max_height,
            stall_since: None,
            last_reload: None,
            last_progress: Instant::now(),
            progress_pos: position,
            queue,
            queue_index: index,
            countdown: None,
            started: false,
            buffer_secs: None,
            pending_seek: None,
            volume: 100.0,
            ads_seen: 0,
            ads_at: None,
        }
    }
}

/// Un elemento que se puede reproducir (las series no: se abren para ver sus episodios).
fn playable(item: &Item) -> bool {
    item.kind != Kind::Series
}

/// Índice del siguiente (`delta` = 1) o anterior (`delta` = -1) elemento reproducible de la cola.
pub fn neighbor(queue: &[Item], from: usize, delta: i64) -> Option<usize> {
    let mut i = from as i64 + delta;
    while i >= 0 && (i as usize) < queue.len() {
        if playable(&queue[i as usize]) {
            return Some(i as usize);
        }
        i += delta;
    }
    None
}

/// Pide los detalles del elemento (en un hilo de trabajo) y arranca su mejor fuente.
/// `queue` es la lista de la que viene el elemento; vacía si no hay lista.
pub fn request(app: &Rc<App>, item: Item, queue: Vec<Item>) {
    request_with(app, item, queue, None);
}

fn request_with(app: &Rc<App>, item: Item, queue: Vec<Item>, carry: Option<String>) {
    let Some(provider) = app
        .providers
        .iter()
        .find(|p| p.platform() == item.platform)
        .cloned()
    else {
        return;
    };
    let queue = if queue
        .iter()
        .any(|q| q.id == item.id && q.platform == item.platform)
    {
        queue
    } else {
        vec![item.clone()]
    };
    let index = queue
        .iter()
        .position(|q| q.id == item.id && q.platform == item.platform)
        .unwrap_or(0);
    set_status(app, "Preparando reproducción…");
    state::set_activity(app, crate::Activity::Preparing);
    app.pool.run(move || {
        let result = provider.details(&item);
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|app| begin(app, item, result, queue, index, carry));
        });
    });
}

thread_local! {
    /// Cambia cada vez que empieza una reproducción o se detiene: cancela las esperas pendientes.
    static TICKET: Cell<u64> = const { Cell::new(0) };
    /// Video que ya se volvió a pedir tras un error de mpv (una sola vez, para no reintentar sin fin).
    static RETRIED: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn new_ticket() -> u64 {
    TICKET.with(|t| {
        t.set(t.get() + 1);
        t.get()
    })
}

fn begin(
    app: &Rc<App>,
    item: Item,
    result: Result<Details>,
    queue: Vec<Item>,
    index: usize,
    carry: Option<String>,
) {
    // Terminó la preparación, salga como salga: el botón vuelve a estar disponible.
    state::set_activity(app, crate::Activity::Idle);
    let ticket = new_ticket();
    match result {
        Ok(details) => {
            let quality = app.settings.borrow().quality;
            match choose_source(&details.sources, quality) {
                Some(source) => {
                    // YouTube sirve algunos videos después de un anuncio previo: se avisa y se espera.
                    let wait = source
                        .available_at
                        .map(|at| at.saturating_sub(now()))
                        .unwrap_or(0);
                    if wait == 0 {
                        start(app, &item, &source, queue, index, carry);
                    } else {
                        set_status(app, format!("YouTube hace esperar {wait} s"));
                        slint::Timer::single_shot(Duration::from_secs(wait), move || {
                            with_app(|app| {
                                if TICKET.with(|t| t.get()) == ticket {
                                    start(app, &item, &source, queue, index, carry);
                                }
                            });
                        });
                    }
                }
                None => set_error(app, "Este título no tiene fuentes reproducibles todavía."),
            }
        }
        Err(e) => set_error(app, e.to_string()),
    }
}

/// Una URL de YouTube que venció (mpv recibe un error, casi siempre un 403) se vuelve a pedir una
/// vez. Retoma en la posición guardada en el historial. Devuelve `false` si no corresponde.
fn retry_expired_youtube(app: &Rc<App>) -> bool {
    let (item, queue) = {
        let guard = app.playback.borrow();
        let Some(pb) = guard.as_ref() else {
            return false;
        };
        if pb.platform != Platform::YouTube {
            return false;
        }
        (pb.item.clone(), pb.queue.clone())
    };
    let first_try = RETRIED.with(|r| {
        let mut retried = r.borrow_mut();
        if retried.as_deref() == Some(item.id.as_str()) {
            false
        } else {
            *retried = Some(item.id.clone());
            true
        }
    });
    if !first_try {
        return false;
    }
    if let Some(provider) = app.providers.iter().find(|p| p.platform() == item.platform) {
        provider.invalidate(&item);
    }
    set_status(app, "La URL del video venció: se pide de nuevo…");
    request(app, item, queue);
    true
}

/// URL que mpv abre para una fuente: las HLS pasan por el proxy, con la calidad pedida.
fn url_for(app: &App, source: &Source, max_height: u32) -> String {
    match (&app.proxy, source.url.contains(".m3u8")) {
        (Some(proxy), true) => proxy.url_for(
            &source.url,
            &Filters {
                max_height: Some(max_height),
                drop: source.drop.clone(),
                twitch_ads: source.twitch_ads,
            },
        ),
        _ => source.url.clone(),
    }
}

fn start(
    app: &Rc<App>,
    item: &Item,
    source: &Source,
    queue: Vec<Item>,
    index: usize,
    carry: Option<String>,
) {
    let quality = app.settings.borrow().quality;
    let url = url_for(app, source, quality);
    crate::chat::begin(app, item);

    // mpv se puede cerrar desde su ventana: si el proceso guardado ya no existe, se descarta
    // (antes de lanzar otro) y se lanza uno nuevo.
    let running = app
        .player
        .borrow_mut()
        .as_mut()
        .is_some_and(|player| player.is_running());
    if !running {
        *app.player.borrow_mut() = None;
    }

    if app.player.borrow().is_none() {
        let options = options_for(&app.settings.borrow());
        match Player::spawn(&options) {
            Ok(player) => *app.player.borrow_mut() = Some(player),
            Err(e) => {
                set_error(app, e.to_string());
                return;
            }
        }
    }

    let resume = history::find(&app.history.borrow(), item.platform, &item.id)
        .and_then(history::resume_point);
    let load = LoadOptions {
        audio_url: source.audio_url.clone(),
    };
    // Una canción suena sin abrir la imagen; un video la muestra.
    let track = crate::music::is_track(item);
    if track {
        crate::music::begin_track(app, item);
    } else {
        crate::music::clear(app);
    }
    let loaded = match app.player.borrow().as_ref() {
        Some(player) => {
            let _ = player.set_video_enabled(!track);
            player.load(&url, &load)
        }
        None => return,
    };
    match loaded {
        Ok(()) => {
            let mut pb = Playback::new(
                item,
                resume.unwrap_or(0.0),
                source,
                quality,
                queue,
                index,
                carry,
            );
            // Retomar: el salto se hace cuando mpv confirme el archivo (ver `LoadOptions`).
            pb.pending_seek = resume.filter(|t| *t > 0.0);
            // Los anuncios que se quitaron antes de este video no cuentan.
            pb.ads_seen = app.proxy.as_ref().map_or(0, |p| p.ads_removed());
            *app.playback.borrow_mut() = Some(pb);
            // Todavía no está confirmado: mpv puede rechazarlo o no abrirlo. Se confirma al cargar.
            set_status(app, format!("Abriendo «{}»…", item.title));
            sync_ui(app);
        }
        Err(e) => set_error(app, format!("No se pudo abrir el video: {e}")),
    }
}

/// Pasa al elemento anterior (`delta` -1) o siguiente (+1) de la cola, con el idioma de audio actual.
pub fn step(app: &Rc<App>, delta: i64) {
    let target = {
        let guard = app.playback.borrow();
        let Some(pb) = guard.as_ref() else {
            return;
        };
        let next = if delta > 0 {
            let neighbor = |from: usize, d: i64| neighbor(&pb.queue, from, d);
            crate::music::next_index(&pb.queue, pb.queue_index, crate::music::modes(), &neighbor)
        } else {
            neighbor(&pb.queue, pb.queue_index, delta)
        };
        next.map(|i| {
            (
                pb.queue[i].clone(),
                pb.queue.clone(),
                i,
                pb.audio_lang.clone(),
            )
        })
    };
    if let Some((item, queue, _, lang)) = target {
        request_with(app, item, queue, lang);
    }
}

/// Lo llama un temporizador de la interfaz: lee los eventos de mpv, revisa trabas y cuentas, y refresca la barra.
pub fn pump(app: &Rc<App>) {
    let events: Vec<Event> = match app.player.borrow().as_ref() {
        Some(player) => player.events().try_iter().collect(),
        None => Vec::new(),
    };
    for event in events {
        apply(app, event);
    }
    apply_pending_audio(app);
    check_stall(app);
    maybe_save(app, false);
    if let Some(position) = app.playback.borrow().as_ref().map(|pb| pb.position) {
        crate::music::pump(app, position);
    }
    crate::chat::pump(app);
    let countdown_done = {
        let guard = app.playback.borrow();
        guard
            .as_ref()
            .and_then(|pb| pb.countdown)
            .is_some_and(|t| t.elapsed() >= NEXT_COUNTDOWN)
    };
    if countdown_done {
        step(app, 1);
    }
    sync_ui(app);
}

/// Aplica la pista de audio que había que buscar (la del idioma elegido en el episodio anterior).
fn apply_pending_audio(app: &Rc<App>) {
    let pending = app
        .playback
        .borrow_mut()
        .as_mut()
        .and_then(|pb| pb.pending_audio.take());
    if let Some(id) = pending {
        if let Some(player) = app.player.borrow().as_ref() {
            let _ = player.set_audio_track(Some(id));
        }
    }
}

fn apply(app: &Rc<App>, event: Event) {
    match event {
        Event::Property { name, value } => {
            let mut guard = app.playback.borrow_mut();
            let Some(pb) = guard.as_mut() else {
                return;
            };
            if matches!(name.as_str(), "core-idle" | "paused-for-cache" | "pause") {
                log::debug!("mpv {name} = {value} (started={})", pb.started);
            }
            match name.as_str() {
                "time-pos" => {
                    if let Some(pos) = value.as_f64() {
                        if moved(pb.progress_pos, pos) {
                            pb.last_progress = Instant::now();
                            pb.progress_pos = pos;
                        }
                        pb.position = pos;
                    }
                }
                "duration" => pb.duration = value.as_f64().unwrap_or(pb.duration),
                "pause" => {
                    pb.paused = value.as_bool().unwrap_or(pb.paused);
                    if !pb.paused {
                        pb.last_progress = Instant::now();
                    }
                    refresh_waiting(pb);
                }
                "core-idle" => {
                    pb.core_idle = value.as_bool().unwrap_or(false);
                    refresh_waiting(pb);
                }
                "paused-for-cache" => {
                    pb.cache_wait = value.as_bool().unwrap_or(false);
                    refresh_waiting(pb);
                }
                "fullscreen" => pb.fullscreen = value.as_bool().unwrap_or(false),
                "demuxer-cache-duration" => pb.buffer_secs = value.as_f64(),
                "track-list" => {
                    let (audio, audio_selected) = tracks(&value, "audio");
                    let (subs, sub_selected) = tracks(&value, "sub");
                    pb.audio_lang = selected_lang(&value, "audio");
                    if let Some(wanted) = pb.carry_audio.take() {
                        if let Some(id) = audio
                            .iter()
                            .find(|(_, label)| label_lang(label) == wanted)
                            .map(|(id, _)| *id)
                        {
                            if Some(id) != audio_selected {
                                pb.pending_audio = Some(id);
                            }
                        }
                    }
                    pb.audio = audio;
                    pb.audio_selected = audio_selected;
                    pb.subs = subs;
                    pb.sub_selected = sub_selected;
                }
                _ => {}
            }
        }
        Event::FileLoaded => {
            log::debug!("mpv file-loaded");
            let confirmed = app.playback.borrow_mut().as_mut().map(|pb| {
                pb.started = true;
                // Desde aquí cuenta la reproducción: lo que tardó en abrir no es una traba.
                pb.last_progress = Instant::now();
                refresh_waiting(pb);
                (pb.title.clone(), pb.pending_seek.take())
            });
            if let Some((title, seek)) = confirmed {
                if let (Some(at), Some(player)) = (seek, app.player.borrow().as_ref()) {
                    let _ = player.seek(at, true);
                }
                set_status(app, format!("Reproduciendo «{title}»"));
            }
        }
        Event::EndFile { reason } => match reason.as_str() {
            // Un cambio de archivo (`loadfile replace`) también avisa `stop`: no es el fin.
            "eof" => on_eof(app),
            "error" => {
                if retry_expired_youtube(app) {
                    return;
                }
                set_error(app, "Se cortó la reproducción.");
                finish(app, false);
            }
            "quit" => finish(app, false),
            _ => {}
        },
    }
}

/// Diferencia mínima, en segundos, para considerar que la reproducción avanzó (o saltó).
const MOVED_EPSILON: f64 = 0.25;

/// ¿Se movió la posición lo suficiente respecto de `reference` como para contar como progreso?
pub fn moved(reference: f64, pos: f64) -> bool {
    (pos - reference).abs() > MOVED_EPSILON
}

/// ¿Está mpv esperando datos de verdad? `core-idle` solo cuenta si no hay pausa (en pausa siempre vale `true`).
pub fn is_waiting(core_idle: bool, cache_wait: bool, paused: bool) -> bool {
    cache_wait || (core_idle && !paused)
}

/// Recalcula `buffering` y el reloj de espera a partir de lo que informó mpv.
fn refresh_waiting(pb: &mut Playback) {
    let waiting = is_waiting(pb.core_idle, pb.cache_wait, pb.paused);
    // Antes de confirmar el archivo la barra sigue en «conectando».
    pb.buffering = waiting || !pb.started;
    match (waiting, pb.stall_since) {
        (true, None) => pb.stall_since = Some(Instant::now()),
        (false, _) => pb.stall_since = None,
        _ => {}
    }
}

/// Terminó el video: se guarda como visto. Si hay siguiente en la cola, empieza la cuenta regresiva.
fn on_eof(app: &Rc<App>) {
    maybe_save(app, true);
    // Lo que sigue: el siguiente de la cola (según el aleatorio y la repetición), o la radio si es
    // una canción y la cola se terminó.
    let (next, radio_seed) = {
        let guard = app.playback.borrow();
        let Some(pb) = guard.as_ref() else {
            return;
        };
        let neighbor = |from: usize, d: i64| neighbor(&pb.queue, from, d);
        let next =
            crate::music::next_index(&pb.queue, pb.queue_index, crate::music::modes(), &neighbor);
        let seed = (next.is_none() && crate::music::is_track(&pb.item)).then(|| pb.item.clone());
        (next.is_some(), seed)
    };
    if next {
        if let Some(pb) = app.playback.borrow_mut().as_mut() {
            pb.countdown = Some(Instant::now());
        }
        return;
    }
    match radio_seed {
        Some(item) => crate::music::start_radio(app, item),
        None => finish(app, true),
    }
}

/// Si la imagen lleva demasiado esperando datos, vuelve a cargar el video en la misma posición
/// con una calidad menor. Así un equipo lento termina viendo algo fluido en vez de congelado.
fn check_stall(app: &Rc<App>) {
    let trabado = {
        let guard = app.playback.borrow();
        let Some(pb) = guard.as_ref() else {
            return;
        };
        // Espera datos (mpv lo informa) o la posición no avanza sin estar en pausa (mpv quedó en cero).
        let limit = if pb.started { STALL_LIMIT } else { LOAD_LIMIT };
        let waiting = pb
            .stall_since
            .is_some_and(|t| t.elapsed().as_secs_f64() >= limit);
        let frozen = !pb.paused && pb.last_progress.elapsed().as_secs_f64() >= limit;
        let cooled = pb
            .last_reload
            .is_none_or(|t| t.elapsed().as_secs_f64() >= RELOAD_COOLDOWN);
        if !((waiting || frozen) && cooled) || pb.countdown.is_some() {
            return;
        }
        (pb.source.clone(), pb.position, pb.max_height, pb.started)
    };
    let (source, position, max_height, started) = trabado;
    {
        let guard = app.playback.borrow();
        if let Some(pb) = guard.as_ref() {
            log::debug!(
                "traba: started={} stall_since={:?} last_progress={:.1}s core_idle={} cache_wait={} paused={} pos={:.1}",
                pb.started,
                pb.stall_since.map(|t| t.elapsed().as_secs_f32()),
                pb.last_progress.elapsed().as_secs_f32(),
                pb.core_idle,
                pb.cache_wait,
                pb.paused,
                pb.position
            );
        }
    }
    let quality = lower_quality(max_height);
    if !started || quality == max_height {
        // Sin abrir todavía, o ya en el mínimo: volver a cargar a menor calidad no cambia nada.
        // Se avisa para que pruebe otra cosa.
        let text = if started {
            format!("La imagen no avanza ni a {quality}p. Probá otra salida de video en Ajustes.")
        } else {
            "El video tarda demasiado en abrir. Revisá la conexión o el registro (MEDIA_LITE_LOG=debug)."
                .to_string()
        };
        log::info!("sin avance (started={started}, {max_height}p)");
        set_error(app, text);
        if let Some(pb) = app.playback.borrow_mut().as_mut() {
            pb.stall_since = None;
            pb.last_progress = Instant::now();
        }
        return;
    }
    log::info!("la imagen se trabó: se vuelve a cargar a {quality}p");
    let url = url_for(app, &source, quality);
    let load = LoadOptions {
        audio_url: source.audio_url.clone(),
    };
    if let Some(player) = app.player.borrow().as_ref() {
        let _ = player.load(&url, &load);
    }
    if let Some(pb) = app.playback.borrow_mut().as_mut() {
        pb.max_height = quality;
        pb.stall_since = None;
        pb.last_reload = Some(Instant::now());
        pb.last_progress = Instant::now();
        pb.buffering = true;
        // Sigue desde donde se trabó, cuando mpv confirme la recarga.
        pb.pending_seek = (position > 0.0).then_some(position);
    }
    set_status(app, format!("Se trabó la imagen: ahora en {quality}p"));
}

/// Sube o baja el volumen en `delta` puntos (de 0 a 130, como mpv).
pub fn volume_step(app: &Rc<App>, delta: f64) {
    let volume = {
        let mut guard = app.playback.borrow_mut();
        let Some(pb) = guard.as_mut() else {
            return;
        };
        pb.volume = (pb.volume + delta).clamp(0.0, 130.0);
        pb.volume
    };
    if let Some(player) = app.player.borrow().as_ref() {
        let _ = player.set_volume(volume);
    }
    set_status(app, format!("Volumen {volume:.0}%"));
}

/// Pasa a la calidad siguiente hacia arriba (240 → 360 → … → 1080 → 240). Recarga el video en la
/// misma posición y la deja como preferida.
pub fn cycle_quality(app: &Rc<App>) {
    let (source, position, current) = {
        let guard = app.playback.borrow();
        let Some(pb) = guard.as_ref() else {
            return;
        };
        (pb.source.clone(), pb.position, pb.max_height)
    };
    let next = higher_quality(current);
    app.settings.borrow_mut().quality = next;
    if let Err(e) = app.store.save_settings(&app.settings.borrow()) {
        log::warn!("no se pudieron guardar los ajustes: {e}");
    }
    let url = url_for(app, &source, next);
    let load = LoadOptions {
        audio_url: source.audio_url.clone(),
    };
    if let Some(player) = app.player.borrow().as_ref() {
        let _ = player.load(&url, &load);
    }
    if let Some(pb) = app.playback.borrow_mut().as_mut() {
        pb.max_height = next;
        pb.buffering = true;
        pb.pending_seek = (position > 0.0).then_some(position);
    }
    set_status(app, format!("Calidad: {next}p"));
}

/// Siguiente escalón de calidad hacia arriba. Si ya está en el máximo, vuelve al mínimo.
pub fn higher_quality(current: u32) -> u32 {
    let mut steps = QUALITY_STEPS;
    steps.reverse();
    steps
        .iter()
        .copied()
        .find(|&step| step > current)
        .unwrap_or(steps[0])
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
pub fn finish(app: &Rc<App>, completed: bool) {
    maybe_save(app, completed);
    *app.playback.borrow_mut() = None;
    crate::chat::end(app);
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
        item: Some(pb.item),
    };
    let mut list = app.history.borrow_mut();
    history::upsert(&mut list, entry);
    if let Err(e) = app.store.save_list("history.json", &list[..]) {
        log::warn!("no se pudo guardar el historial: {e}");
    }
}

/// Pausa o sigue. Si había una cuenta regresiva, también la cancela.
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
        pb.countdown = None;
        // La pausa no cuenta como traba: el reloj de avance arranca al reanudar.
        pb.last_progress = Instant::now();
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
    new_ticket();
    crate::music::clear(app);
    RETRIED.with(|r| *r.borrow_mut() = None);
    *app.playback.borrow_mut() = None;
    *app.player.borrow_mut() = None;
    crate::chat::end(app);
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

/// Pasa a la siguiente pista de audio. Recuerda su idioma para el próximo episodio.
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
    if let (Some(id), Some(pb)) = (next, app.playback.borrow_mut().as_mut()) {
        pb.audio_selected = Some(id);
        pb.audio_lang = pb
            .audio
            .iter()
            .find(|t| t.0 == id)
            .map(|t| label_lang(&t.1));
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
/// Cuánto dura el aviso «Saltando el anuncio de Twitch…» después del último anuncio quitado.
const AD_NOTICE: Duration = Duration::from_secs(6);

/// Anota cuándo el proxy quitó anuncios de Twitch en este video, para mostrar el aviso.
fn note_ads(app: &Rc<App>) {
    let total = app.proxy.as_ref().map_or(0, |p| p.ads_removed());
    if let Some(pb) = app.playback.borrow_mut().as_mut() {
        if pb.source.twitch_ads && total > pb.ads_seen {
            pb.ads_seen = total;
            pb.ads_at = Some(Instant::now());
        }
    }
}

pub fn sync_ui(app: &Rc<App>) {
    let Some(window) = app.weak.upgrade() else {
        return;
    };
    note_ads(app);
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
            window.set_can_step_prev(neighbor(&pb.queue, pb.queue_index, -1).is_some());
            let modes = crate::music::modes();
            window.set_shuffle_on(modes.shuffle);
            window.set_repeat_label(modes.repeat.label().into());
            let can_next = {
                let neighbor = |from: usize, d: i64| neighbor(&pb.queue, from, d);
                crate::music::next_index(&pb.queue, pb.queue_index, modes, &neighbor).is_some()
            };
            window.set_can_step_next(can_next);
            window.set_play_buffering(pb.buffering && pb.countdown.is_none());
            let status = match pb.countdown {
                Some(since) => {
                    let left = NEXT_COUNTDOWN
                        .saturating_sub(since.elapsed())
                        .as_secs_f32()
                        .ceil() as u32;
                    format!("Siguiente en {left} s")
                }
                None if pb.ads_at.is_some_and(|t| t.elapsed() < AD_NOTICE) => {
                    "Saltando el anuncio de Twitch…".to_string()
                }
                // Antes del primer cuadro: mpv todavía no abrió el stream.
                None if pb.buffering && !pb.started => "Conectando con el video…".to_string(),
                None if pb.buffering => match pb.buffer_secs {
                    Some(secs) => format!("Cargando buffer · {secs:.0} s"),
                    None => "Cargando buffer…".to_string(),
                },
                None if pb.paused => format!("En pausa · {}p", pb.max_height),
                None => format!("Reproduciendo · {}p", pb.max_height),
            };
            window.set_play_status(status.into());
            window.set_quality_label(format!("{}p", pb.max_height).into());
        }
        None => {
            window.set_playing(false);
            window.set_play_buffering(false);
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

/// El archivo de teclas del AppImage, si estamos corriendo desde uno.
fn input_conf_path() -> Option<std::path::PathBuf> {
    let appdir = std::env::var("APPDIR").ok()?;
    let path = std::path::PathBuf::from(appdir).join("usr/share/media-lite/input.conf");
    path.exists().then_some(path)
}

fn options_for(settings: &Settings) -> Options {
    Options {
        input_conf: input_conf_path(),
        mode: Mode::Pip,
        vo: settings.vo.clone(),
        hwdec: settings.hwdec.clone(),
        low_video: settings.low_video,
        cache_mb: CACHE_MB,
        ao: None,
        // Con `MEDIA_LITE_LOG=debug` mpv también escribe sus mensajes detallados en el registro.
        verbose: log::log_enabled!(log::Level::Debug),
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
        let name = lang_name(lang, title);
        let label = if title.is_empty() || title == name {
            name
        } else {
            format!("{name} · {title}")
        };
        if track.get("selected").and_then(Value::as_bool) == Some(true) {
            selected = Some(id);
        }
        out.push((id, label));
    }
    (out, selected)
}

/// Idioma de una etiqueta de pista (`es · Comentario` → `es`).
fn label_lang(label: &str) -> String {
    label.split(" · ").next().unwrap_or(label).to_string()
}

/// Idioma (con el nombre en español) de la pista activa de un tipo, según la lista de mpv.
fn selected_lang(list: &Value, kind: &str) -> Option<String> {
    list.as_array()?
        .iter()
        .find(|t| {
            t.get("type").and_then(Value::as_str) == Some(kind)
                && t.get("selected").and_then(Value::as_bool) == Some(true)
        })
        .map(|t| {
            let lang = t.get("lang").and_then(Value::as_str).unwrap_or("");
            let title = t.get("title").and_then(Value::as_str).unwrap_or("");
            lang_name(lang, title)
        })
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
            drop: Vec::new(),
            twitch_ads: false,
            available_at: None,
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
    fn quality_steps_up_and_wraps_to_the_minimum() {
        assert_eq!(higher_quality(480), 720);
        assert_eq!(higher_quality(1080), 240);
        assert_eq!(higher_quality(240), 360);
    }

    #[test]
    fn quality_steps_down_to_the_minimum() {
        assert_eq!(lower_quality(480), 360);
        assert_eq!(lower_quality(720), 480);
        assert_eq!(lower_quality(240), 240);
        assert_eq!(lower_quality(1000), 720);
    }

    #[test]
    fn frame_sized_steps_add_up_to_progress() {
        // 24 fps: un evento cada ~0,042 s. Cada paso solo no alcanza, pero la suma sí.
        let mut reference = 10.0;
        let mut marks = 0;
        for n in 1..=240 {
            let pos = 10.0 + n as f64 / 24.0;
            if moved(reference, pos) {
                reference = pos;
                marks += 1;
            }
        }
        assert!(marks >= 30, "marcó {marks} veces en 10 s de video");
        // Posición quieta: nunca cuenta.
        assert!(!moved(5.0, 5.0));
    }

    #[test]
    fn pause_is_not_waiting_for_data() {
        // mpv pone core-idle en true al pausar: no es una traba.
        assert!(!is_waiting(true, false, true));
        assert!(is_waiting(true, false, false));
        assert!(is_waiting(true, true, true));
        assert!(!is_waiting(false, false, false));
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
            vec![(2, "Español".to_string()), (3, "Comentario".to_string())]
        );
        assert_eq!(selected, Some(2));
    }
}
