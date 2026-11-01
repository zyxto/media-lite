//! Pool fijo de hilos y contador de generaciones.
//!
//! Sin runtime async: dos o tres hilos bloqueantes alcanzan para una interfaz con pocas
//! conexiones a la vez, y cuestan poca memoria.

use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{unbounded, Sender};

type Job = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone)]
pub struct Pool {
    tx: Sender<Job>,
}

impl Pool {
    /// Crea `threads` hilos que esperan trabajos. Los hilos terminan cuando se cae el último `Pool`.
    pub fn new(threads: usize, name: &str) -> Pool {
        let (tx, rx) = unbounded::<Job>();
        for i in 0..threads.max(1) {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .stack_size(512 * 1024)
                .spawn(move || {
                    for job in rx {
                        job();
                    }
                })
                .expect("no se pudo crear un hilo de trabajo");
        }
        Pool { tx }
    }

    /// Encola un trabajo. No bloquea.
    pub fn run(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.tx.send(Box::new(job));
    }
}

/// Número de generación para descartar resultados viejos: si el usuario cambió de pantalla,
/// la respuesta del pedido anterior ya no se muestra.
#[derive(Debug, Default)]
pub struct Generation(AtomicU64);

impl Generation {
    /// Empieza una generación nueva y la devuelve.
    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn jobs_run_on_pool_threads() {
        let pool = Pool::new(2, "test");
        let (tx, rx) = mpsc::channel();
        for i in 0..10 {
            let tx = tx.clone();
            pool.run(move || tx.send(i * 2).unwrap());
        }
        let mut got: Vec<i32> = (0..10).map(|_| rx.recv().unwrap()).collect();
        got.sort();
        assert_eq!(got, (0..10).map(|i| i * 2).collect::<Vec<_>>());
    }

    #[test]
    fn generations_increase() {
        let g = Generation::default();
        assert_eq!(g.current(), 0);
        let a = g.next();
        let b = g.next();
        assert!(b > a);
        assert_eq!(g.current(), b);
    }
}
