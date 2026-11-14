//! Zwischenablage für Passwörter: ohne Windows-Verlauf und Cloud-Sync kopieren und später
//! nur dann leeren, wenn dort noch genau unser Wert liegt.

use std::time::{Duration, Instant};

use zeroize::Zeroizing;

/// Abstand für einen neuen Versuch, wenn die Zwischenablage gerade nicht lesbar ist.
const RETRY: Duration = Duration::from_secs(1);

pub trait Backend {
    /// Kopiert `text` geschützt (ohne Verlauf/Cloud). `Err`, wenn das nicht ging.
    fn set_secret(&mut self, text: &str) -> Result<(), ()>;
    /// Aktueller Text. `None`, wenn nicht lesbar (gesperrt oder kein Text).
    fn read(&mut self) -> Option<String>;
    fn clear(&mut self) -> bool;
}

/// Systemzwischenablage über arboard. Jeder Aufruf öffnet sie neu, damit sie nicht blockiert bleibt.
pub struct SystemClipboard;

impl Backend for SystemClipboard {
    fn set_secret(&mut self, text: &str) -> Result<(), ()> {
        let mut clipboard = arboard::Clipboard::new().map_err(|_| ())?;
        let set = clipboard.set();
        #[cfg(windows)]
        let set = {
            use arboard::SetExtWindows;
            set.exclude_from_monitoring()
        };
        set.text(text).map_err(|_| ())
    }

    fn read(&mut self) -> Option<String> {
        arboard::Clipboard::new().ok()?.get_text().ok()
    }

    fn clear(&mut self) -> bool {
        arboard::Clipboard::new().and_then(|mut c| c.clear()).is_ok()
    }
}

pub struct ClipboardGuard<B: Backend> {
    backend: B,
    held: Option<Zeroizing<String>>,
    clear_at: Option<Instant>,
}

impl<B: Backend> ClipboardGuard<B> {
    pub fn new(backend: B) -> Self {
        Self { backend, held: None, clear_at: None }
    }

    /// Kopiert `text`. Bei `Err` muss der Aufrufer selbst kopieren (Rückfall) und einen Hinweis zeigen;
    /// der Wert wird trotzdem gemerkt. `clear_after: None` heißt: nicht automatisch leeren.
    pub fn copy(&mut self, text: &str, clear_after: Option<Duration>, now: Instant) -> Result<(), ()> {
        self.held = Some(Zeroizing::new(text.to_string()));
        self.clear_at = clear_after.map(|d| now + d);
        self.backend.set_secret(text)
    }

    /// Leert nach Ablauf, falls noch unser Wert drin ist. Gibt zurück, wann erneut geprüft werden soll.
    pub fn tick(&mut self, now: Instant) -> Option<Duration> {
        let at = self.clear_at?;
        if now < at {
            return Some(at - now);
        }
        match self.backend.read().map(Zeroizing::new) {
            None => Some(RETRY),
            Some(current) => {
                if self.holds(&current) {
                    self.backend.clear();
                }
                self.forget();
                None
            }
        }
    }

    /// Sperren/Beenden: leert, wenn unser Wert drin ist oder sich das nicht prüfen lässt.
    pub fn clear_now(&mut self) {
        if self.held.is_none() {
            return;
        }
        let foreign = matches!(self.backend.read().map(Zeroizing::new), Some(current) if !self.holds(&current));
        if !foreign {
            self.backend.clear();
        }
        self.forget();
    }

    fn holds(&self, current: &str) -> bool {
        self.held.as_deref().is_some_and(|h| h.as_str() == current)
    }

    fn forget(&mut self) {
        self.held = None;
        self.clear_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        content: Option<String>,
        unreadable: bool,
        fail_set: bool,
        clears: usize,
    }

    impl Backend for &mut Fake {
        fn set_secret(&mut self, text: &str) -> Result<(), ()> {
            if self.fail_set {
                return Err(());
            }
            self.content = Some(text.to_string());
            Ok(())
        }
        fn read(&mut self) -> Option<String> {
            if self.unreadable { None } else { Some(self.content.clone().unwrap_or_default()) }
        }
        fn clear(&mut self) -> bool {
            self.content = None;
            self.clears += 1;
            true
        }
    }

    const S30: Duration = Duration::from_secs(30);
    fn secs(t0: Instant, s: u64) -> Instant {
        t0 + Duration::from_secs(s)
    }

    #[test]
    fn clears_own_value_after_timeout() {
        let mut fake = Fake::default();
        let t0 = Instant::now();
        let mut g = ClipboardGuard::new(&mut fake);
        g.copy("pw", Some(S30), t0).unwrap();
        assert_eq!(g.tick(secs(t0, 29)), Some(Duration::from_secs(1)));
        assert_eq!(g.tick(secs(t0, 30)), None);
        drop(g);
        assert_eq!(fake.clears, 1);
        assert_eq!(fake.content, None);
    }

    #[test]
    fn keeps_foreign_value() {
        let mut fake = Fake::default();
        let t0 = Instant::now();
        let mut g = ClipboardGuard::new(&mut fake);
        g.copy("pw", Some(S30), t0).unwrap();
        g.backend.content = Some("anderes".into());
        assert_eq!(g.tick(secs(t0, 30)), None);
        drop(g);
        assert_eq!(fake.clears, 0);
        assert_eq!(fake.content.as_deref(), Some("anderes"));
    }

    #[test]
    fn unreadable_retries_every_second() {
        let mut fake = Fake::default();
        let t0 = Instant::now();
        let mut g = ClipboardGuard::new(&mut fake);
        g.copy("pw", Some(S30), t0).unwrap();
        g.backend.unreadable = true;
        assert_eq!(g.tick(secs(t0, 30)), Some(Duration::from_secs(1)));
        g.backend.unreadable = false;
        assert_eq!(g.tick(secs(t0, 31)), None);
        drop(g);
        assert_eq!(fake.clears, 1);
    }

    #[test]
    fn clear_now_clears_unreadable() {
        let mut fake = Fake::default();
        let mut g = ClipboardGuard::new(&mut fake);
        g.copy("pw", None, Instant::now()).unwrap();
        g.backend.unreadable = true;
        g.clear_now();
        drop(g);
        assert_eq!(fake.clears, 1);
    }

    #[test]
    fn clear_now_keeps_foreign_value() {
        let mut fake = Fake::default();
        let mut g = ClipboardGuard::new(&mut fake);
        g.copy("pw", None, Instant::now()).unwrap();
        g.backend.content = Some("anderes".into());
        g.clear_now();
        drop(g);
        assert_eq!(fake.clears, 0);
    }

    #[test]
    fn clear_now_without_copy_does_nothing() {
        let mut fake = Fake { content: Some("fremd".into()), ..Default::default() };
        let mut g = ClipboardGuard::new(&mut fake);
        g.clear_now();
        drop(g);
        assert_eq!(fake.clears, 0);
    }

    #[test]
    fn second_copy_restarts_timer() {
        let mut fake = Fake::default();
        let t0 = Instant::now();
        let mut g = ClipboardGuard::new(&mut fake);
        g.copy("a", Some(S30), t0).unwrap();
        g.copy("b", Some(S30), secs(t0, 20)).unwrap();
        g.tick(secs(t0, 30));
        assert_eq!(g.backend.clears, 0);
        g.tick(secs(t0, 50));
        drop(g);
        assert_eq!(fake.clears, 1);
    }

    #[test]
    fn no_timer_when_setting_off() {
        let mut fake = Fake::default();
        let t0 = Instant::now();
        let mut g = ClipboardGuard::new(&mut fake);
        g.copy("a", None, t0).unwrap();
        assert_eq!(g.tick(secs(t0, 3600)), None);
        drop(g);
        assert_eq!(fake.clears, 0);
    }

    #[test]
    fn failed_set_still_tracks_value() {
        let mut fake = Fake { fail_set: true, ..Default::default() };
        let mut g = ClipboardGuard::new(&mut fake);
        assert!(g.copy("a", None, Instant::now()).is_err());
        g.backend.content = Some("a".into()); // wie ctx.copy_text als Rückfall
        g.clear_now();
        drop(g);
        assert_eq!(fake.clears, 1);
    }
}
