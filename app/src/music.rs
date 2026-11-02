//! Música: el aleatorio y la repetición de la cola, el siguiente elemento, las letras sincronizadas
//! junto al reproductor, y la radio al terminar la cola.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use media_lite_core::{Item, Kind, Platform, Result};
use media_lite_providers::lyrics::{self, Line};
use slint::{ModelRc, SharedString, VecModel};

use crate::state::{set_status, with_app, App};
use crate::LyricRow;

/// Líneas de la letra que se ven a la vez (la que suena y sus vecinas).
const WINDOW: usize = 11;

/// Qué se repite al terminar la cola.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    /// La cola entera, desde el principio.
    All,
    /// El elemento actual.
    One,
}

impl Repeat {
    fn next(self) -> Repeat {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }

    /// 0: no, 1: todo, 2: una. Es lo que usa el ícono de la barra.
    pub fn index(self) -> i32 {
        match self {
            Repeat::Off => 0,
            Repeat::All => 1,
            Repeat::One => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Repeat::Off => "Repetir: no",
            Repeat::All => "Repetir: todo",
            Repeat::One => "Repetir: una",
        }
    }
}

/// Los modos de la cola. Viven fuera de cada reproducción: sobreviven al cambio de canción.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub shuffle: bool,
    pub repeat: Repeat,
}

thread_local! {
    static MODES: Cell<Modes> = const { Cell::new(Modes { shuffle: false, repeat: Repeat::Off }) };
    static SEED: Cell<u64> = const { Cell::new(0) };
    static LYRICS: RefCell<Lyrics> = RefCell::new(Lyrics::default());
}

pub fn modes() -> Modes {
    MODES.with(Cell::get)
}

/// Enciende o apaga el aleatorio. Devuelve los modos nuevos.
pub fn toggle_shuffle() -> Modes {
    MODES.with(|m| {
        let mut modes = m.get();
        modes.shuffle = !modes.shuffle;
        m.set(modes);
        modes
    })
}

/// Pasa a la siguiente opción de repetir (no, todo, una). Devuelve los modos nuevos.
pub fn cycle_repeat() -> Modes {
    MODES.with(|m| {
        let mut modes = m.get();
        modes.repeat = modes.repeat.next();
        m.set(modes);
        modes
    })
}

/// Un número al azar menor que `n` (xorshift; no hace falta criptografía para elegir una canción).
fn random_below(n: usize) -> usize {
    SEED.with(|seed| {
        let mut x = seed.get();
        if x == 0 {
            x = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1)
                | 1;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        seed.set(x);
        (x % n as u64) as usize
    })
}

/// El elemento que sigue a `current` en la cola, según el aleatorio y la repetición. `neighbor(desde,
/// delta)` dice cuál es el reproducible más cercano (lo da la reproducción).
pub fn next_index(
    queue: &[Item],
    current: usize,
    modes: Modes,
    neighbor: &dyn Fn(usize, i64) -> Option<usize>,
) -> Option<usize> {
    if queue.is_empty() {
        return None;
    }
    if modes.repeat == Repeat::One {
        return neighbor(current, 0);
    }
    if modes.shuffle {
        let others: Vec<usize> = (0..queue.len())
            .filter(|i| *i != current && neighbor(*i, 0) == Some(*i))
            .collect();
        return (!others.is_empty()).then(|| others[random_below(others.len())]);
    }
    neighbor(current, 1).or_else(|| {
        (modes.repeat == Repeat::All)
            .then(|| neighbor(0, 0).or_else(|| neighbor(0, 1)))
            .flatten()
    })
}

/// Las letras de la canción que suena.
#[derive(Default)]
struct Lyrics {
    session: u64,
    lines: Vec<Line>,
    current: Option<usize>,
    active: bool,
}

/// Una canción empieza: se borran las letras anteriores y se piden las nuevas, en segundo plano.
pub fn begin_track(app: &Rc<App>, item: &Item) {
    let session = LYRICS.with(|l| {
        let mut l = l.borrow_mut();
        l.session += 1;
        l.lines.clear();
        l.current = None;
        l.active = false;
        l.session
    });
    sync(app);
    let title = item.title.clone();
    // El subtítulo de una canción es «artistas · álbum»: el artista es lo primero.
    let artist = item.subtitle.split(" · ").next().unwrap_or("").to_string();
    let duration = item.duration_secs;
    app.pool.run(move || {
        let agent = media_lite_core::http::agent();
        let result = lyrics::fetch(&agent, &title, &artist, "", duration);
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|app| deliver(app, session, result));
        });
    });
}

fn deliver(app: &Rc<App>, session: u64, result: Result<Vec<Line>>) {
    let delivered = LYRICS.with(|l| {
        let mut l = l.borrow_mut();
        if l.session != session {
            return false;
        }
        match result {
            Ok(lines) => {
                l.lines = lines;
                l.active = true;
            }
            Err(e) => log::info!("letras: {}", e.message),
        }
        true
    });
    if delivered {
        sync(app);
    }
}

/// Se detuvo la reproducción: no quedan letras.
pub fn clear(app: &Rc<App>) {
    LYRICS.with(|l| {
        let mut l = l.borrow_mut();
        l.session += 1;
        l.lines.clear();
        l.current = None;
        l.active = false;
    });
    sync(app);
}

/// Con la posición del video (segundos), marca la línea que suena. Solo redibuja si cambió.
pub fn pump(app: &Rc<App>, position: f64) {
    let changed = LYRICS.with(|l| {
        let mut l = l.borrow_mut();
        if !l.active {
            return false;
        }
        let current = lyrics::line_at(&l.lines, position);
        if current == l.current {
            return false;
        }
        l.current = current;
        true
    });
    if changed {
        sync(app);
    }
}

/// Lleva las letras a la ventana: el panel aparece si hay letra, con unas pocas líneas alrededor de
/// la que suena.
fn sync(app: &Rc<App>) {
    let Some(window) = app.weak.upgrade() else {
        return;
    };
    let (active, rows) = LYRICS.with(|l| {
        let l = l.borrow();
        let center = l.current.unwrap_or(0);
        let start = center.saturating_sub(WINDOW / 2);
        let rows: Vec<LyricRow> = l
            .lines
            .iter()
            .enumerate()
            .skip(start)
            .take(WINDOW)
            .map(|(i, line)| LyricRow {
                text: SharedString::from(line.text.as_str()),
                current: l.current == Some(i),
            })
            .collect();
        (l.active, rows)
    });
    window.set_lyrics_open(active);
    window.set_lyrics_rows(ModelRc::new(VecModel::from(rows)));
}

/// Al terminar la cola de música, busca canciones parecidas (la radio) y sigue con la primera.
pub fn start_radio(app: &Rc<App>, item: Item) {
    let Some(provider) = app
        .providers
        .iter()
        .find(|p| p.platform() == Platform::YouTubeMusic)
        .cloned()
    else {
        crate::playback::finish(app, true);
        return;
    };
    set_status(app, "Buscando más música parecida…");
    app.pool.run(move || {
        let result = provider.radio(&item);
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|app| match result {
                Ok(list) if !list.is_empty() => {
                    let first = list[0].clone();
                    crate::playback::request(app, first, list);
                }
                _ => crate::playback::finish(app, true),
            });
        });
    });
}

/// ¿Es una canción (de YouTube Music)?
pub fn is_track(item: &Item) -> bool {
    item.kind == Kind::Track && item.platform == Platform::YouTubeMusic
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(n: usize) -> Vec<Item> {
        (0..n)
            .map(|i| {
                Item::new(
                    Platform::YouTubeMusic,
                    Kind::Track,
                    i.to_string(),
                    format!("t{i}"),
                )
            })
            .collect()
    }

    #[test]
    fn next_follows_the_queue_and_stops_at_the_end() {
        let q = queue(3);
        let plain = Modes::default();
        let step = |from: usize, delta: i64| -> Option<usize> {
            let to = from as i64 + delta;
            (0..3).contains(&to).then_some(to as usize)
        };
        assert_eq!(next_index(&q, 0, plain, &step), Some(1));
        assert_eq!(next_index(&q, 2, plain, &step), None);
    }

    #[test]
    fn repeat_all_wraps_to_the_start_and_repeat_one_stays() {
        let q = queue(3);
        let step = |from: usize, delta: i64| -> Option<usize> {
            let to = from as i64 + delta;
            (0..3).contains(&to).then_some(to as usize)
        };
        let all = Modes {
            shuffle: false,
            repeat: Repeat::All,
        };
        assert_eq!(next_index(&q, 2, all, &step), Some(0));
        let one = Modes {
            shuffle: false,
            repeat: Repeat::One,
        };
        assert_eq!(next_index(&q, 1, one, &step), Some(1));
    }

    #[test]
    fn shuffle_picks_another_track_in_the_queue() {
        let q = queue(5);
        let step = |from: usize, delta: i64| -> Option<usize> {
            let to = from as i64 + delta;
            (0..5).contains(&to).then_some(to as usize)
        };
        let shuffle = Modes {
            shuffle: true,
            repeat: Repeat::Off,
        };
        for _ in 0..20 {
            let next = next_index(&q, 2, shuffle, &step).unwrap();
            assert_ne!(next, 2);
            assert!(next < 5);
        }
    }

    #[test]
    fn repeat_cycles_through_its_three_states() {
        assert_eq!(Repeat::Off.next(), Repeat::All);
        assert_eq!(Repeat::All.next(), Repeat::One);
        assert_eq!(Repeat::One.next(), Repeat::Off);
        assert_eq!(Repeat::All.label(), "Repetir: todo");
    }
}
