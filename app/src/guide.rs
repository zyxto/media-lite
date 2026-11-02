//! Guía de programación de un canal en vivo (Pluto TV): los programas de las próximas horas, en el
//! panel de la derecha, con el que está al aire marcado.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use media_lite_core::{Item, Kind, Platform, Program};
use slint::{ModelRc, SharedString, VecModel};

use crate::state::{with_app, App};
use crate::GuideRow;

/// Cuántos programas se muestran (el que está al aire y los siguientes).
const SHOWN: usize = 7;

#[derive(Default)]
struct Guide {
    session: u64,
    programs: Vec<Program>,
    current: Option<usize>,
    active: bool,
}

thread_local! {
    static GUIDE: RefCell<Guide> = RefCell::new(Guide::default());
}

/// ¿Tiene guía este elemento? Hoy, los canales en vivo de Pluto TV.
pub fn has_guide(item: &Item) -> bool {
    item.platform == Platform::Pluto && matches!(item.kind, Kind::Live | Kind::Channel)
}

/// Un canal empieza: se borra la guía anterior y se pide la nueva, en segundo plano.
pub fn begin(app: &Rc<App>, item: &Item) {
    let session = GUIDE.with(|g| {
        let mut g = g.borrow_mut();
        g.session += 1;
        g.programs.clear();
        g.current = None;
        g.active = false;
        g.session
    });
    sync(app);
    if !has_guide(item) {
        return;
    }
    let Some(provider) = app
        .providers
        .iter()
        .find(|p| p.platform() == item.platform)
        .cloned()
    else {
        return;
    };
    let channel = item.clone();
    app.pool.run(move || {
        let result = provider.guide(&channel);
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|app| deliver(app, session, result));
        });
    });
}

fn deliver(app: &Rc<App>, session: u64, result: media_lite_core::Result<Vec<Program>>) {
    let delivered = GUIDE.with(|g| {
        let mut g = g.borrow_mut();
        if g.session != session {
            return false;
        }
        match result {
            Ok(programs) => {
                g.programs = programs;
                g.active = true;
            }
            Err(e) => log::info!("guía: {}", e.message),
        }
        true
    });
    if delivered {
        pump(app);
    }
}

/// Se detuvo la reproducción: no queda guía.
pub fn clear(app: &Rc<App>) {
    GUIDE.with(|g| {
        let mut g = g.borrow_mut();
        g.session += 1;
        g.programs.clear();
        g.current = None;
        g.active = false;
    });
    sync(app);
}

/// Con la hora actual, marca el programa que está al aire. Solo redibuja si cambió.
pub fn pump(app: &Rc<App>) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let changed = GUIDE.with(|g| {
        let mut g = g.borrow_mut();
        if !g.active {
            return false;
        }
        // El programa que está al aire es el último que ya empezó (y todavía no terminó).
        let current = g
            .programs
            .iter()
            .position(|p| p.start <= now && now < p.stop);
        if current == g.current {
            return false;
        }
        g.current = current;
        true
    });
    if changed {
        sync(app);
    }
}

/// Lleva la guía a la ventana: desde el programa que está al aire (o el próximo).
fn sync(app: &Rc<App>) {
    let Some(window) = app.weak.upgrade() else {
        return;
    };
    let (active, rows) = GUIDE.with(|g| {
        let g = g.borrow();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let first = g
            .programs
            .iter()
            .position(|p| p.stop > now)
            .unwrap_or(g.programs.len());
        let rows: Vec<GuideRow> = g
            .programs
            .iter()
            .enumerate()
            .skip(first)
            .take(SHOWN)
            .map(|(i, p)| GuideRow {
                time: SharedString::from(local_hhmm(p.start).as_str()),
                title: SharedString::from(p.title.as_str()),
                current: g.current == Some(i),
            })
            .collect();
        (g.active, rows)
    });
    window.set_guide_open(active);
    window.set_guide_rows(ModelRc::new(VecModel::from(rows)));
}

/// «HH:MM» en la hora local de la máquina.
fn local_hhmm(epoch: u64) -> String {
    let t = epoch as libc::time_t;
    // SAFETY: `localtime_r` escribe solo en `tm`, que vive en este marco; `t` es un valor por copia.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::from("--:--");
        }
        tm
    };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pluto_live_channels_have_a_guide() {
        let live = Item::new(Platform::Pluto, Kind::Live, "1", "Canal");
        assert!(has_guide(&live));
        let movie = Item::new(Platform::Pluto, Kind::Movie, "2", "Peli");
        assert!(!has_guide(&movie));
        let twitch = Item::new(Platform::Twitch, Kind::Live, "3", "Directo");
        assert!(!has_guide(&twitch));
    }

    #[test]
    fn local_time_has_the_hh_mm_shape() {
        let s = local_hhmm(1_793_532_600);
        assert_eq!(s.len(), 5);
        assert_eq!(&s[2..3], ":");
    }
}
