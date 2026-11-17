//! Small bounded worker pool for heavy background work (image decoding,
//! LaTeX compilation, saving, export). Keeps that work off the UI thread.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

type Job = Box<dyn FnOnce() + Send + 'static>;

pub struct WorkerPool {
    tx: Option<Sender<Job>>,
    pending: Arc<AtomicUsize>,
    handles: Vec<JoinHandle<()>>,
}

impl WorkerPool {
    pub fn new(name: &str, threads: usize) -> Self {
        let (tx, rx) = channel::<Job>();
        let rx: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(rx));
        let pending = Arc::new(AtomicUsize::new(0));
        let handles = (0..threads.max(1))
            .map(|i| {
                let rx = Arc::clone(&rx);
                let pending = Arc::clone(&pending);
                std::thread::Builder::new()
                    .name(format!("{name}-{i}"))
                    .spawn(move || {
                        loop {
                            // Hold the lock only while receiving.
                            let job = match rx.lock() {
                                Ok(guard) => guard.recv(),
                                Err(_) => return,
                            };
                            let Ok(job) = job else { return };
                            // A panicking job must not take the worker down.
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                            pending.fetch_sub(1, Ordering::AcqRel);
                        }
                    })
                    .expect("failed to spawn worker thread")
            })
            .collect();
        Self {
            tx: Some(tx),
            pending,
            handles,
        }
    }

    pub fn spawn(&self, f: impl FnOnce() + Send + 'static) {
        if let Some(tx) = &self.tx {
            self.pending.fetch_add(1, Ordering::AcqRel);
            if tx.send(Box::new(f)).is_err() {
                self.pending.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }

    /// Jobs queued or running.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }

    /// Stop accepting work and wait for queued jobs to finish (tests, bench).
    pub fn join(mut self) {
        self.tx.take();
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        // Closing the channel lets idle workers exit; busy workers finish
        // their current job in the background (we do not block shutdown).
        self.tx.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_jobs_and_survives_panics() {
        let pool = WorkerPool::new("test", 2);
        let (tx, rx) = channel();
        for i in 0..10 {
            let tx = tx.clone();
            pool.spawn(move || {
                if i == 3 {
                    panic!("boom");
                }
                tx.send(i).unwrap();
            });
        }
        drop(tx);
        pool.join();
        let mut got: Vec<i32> = rx.iter().collect();
        got.sort();
        assert_eq!(got, vec![0, 1, 2, 4, 5, 6, 7, 8, 9]);
    }
}
