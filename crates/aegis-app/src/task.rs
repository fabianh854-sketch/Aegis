//! Kleine Hintergrund-Jobs (z. B. Argon2 beim Entsperren), damit die Oberfläche weiterzeichnet.

use std::sync::mpsc::{self, Receiver, TryRecvError};

pub struct Task<T> {
    rx: Receiver<Result<T, String>>,
}

impl<T: Send + 'static> Task<T> {
    /// Startet `job` in einem eigenen Thread. `on_done` läuft danach im selben Thread,
    /// z. B. um ein Neuzeichnen anzustoßen.
    pub fn spawn(
        job: impl FnOnce() -> Result<T, String> + Send + 'static,
        on_done: impl FnOnce() + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Ist der Empfänger schon weg (App beendet), ist das Ergebnis egal.
            let _ = tx.send(job());
            on_done();
        });
        Self { rx }
    }

    /// `None`, solange der Job läuft. Endet der Thread ohne Ergebnis (Panik), kommt ein Fehler.
    pub fn poll(&self) -> Option<Result<T, String>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("Vorgang unerwartet abgebrochen".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc,
        },
        time::{Duration, Instant},
    };

    fn wait<T>(task: &Task<T>) -> Result<T, String>
    where
        T: Send + 'static,
    {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(r) = task.poll() {
                return r;
            }
            assert!(Instant::now() < deadline, "Task wurde nicht fertig");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn pending_until_job_finishes() {
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let task = Task::spawn(
            move || {
                go_rx.recv().unwrap();
                Ok(42)
            },
            || {},
        );
        assert!(task.poll().is_none());
        go_tx.send(()).unwrap();
        assert_eq!(wait(&task), Ok(42));
    }

    #[test]
    fn job_error_is_reported() {
        let task: Task<i32> = Task::spawn(|| Err("x".to_string()), || {});
        assert_eq!(wait(&task), Err("x".to_string()));
    }

    #[test]
    fn panic_is_reported_not_pending_forever() {
        let task: Task<i32> = Task::spawn(|| panic!("boom"), || {});
        assert_eq!(wait(&task), Err("Vorgang unerwartet abgebrochen".to_string()));
    }

    #[test]
    fn on_done_runs_after_result() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let task = Task::spawn(|| Ok(1), move || flag.store(true, Ordering::SeqCst));
        assert_eq!(wait(&task), Ok(1));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !done.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
