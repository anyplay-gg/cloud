//! Rate limits: counters in fixed windows, kept in memory.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How many requests of each kind are allowed, and in which window. The defaults are the table in
/// the API specification.
#[derive(Clone, Debug)]
pub struct Rates {
    pub email_per_address: u32,
    pub email_per_ip: u32,
    pub code_per_ip: u32,
    /// The window of the three limits above.
    pub auth_window: Duration,
    pub refresh_per_device: u32,
    pub refresh_window: Duration,
    pub account: u32,
    pub account_window: Duration,
    pub admin_per_ip: u32,
}

impl Default for Rates {
    fn default() -> Self {
        Self {
            email_per_address: 3,
            email_per_ip: 10,
            code_per_ip: 20,
            auth_window: Duration::from_secs(15 * 60),
            refresh_per_device: 30,
            refresh_window: Duration::from_secs(60 * 60),
            account: 120,
            account_window: Duration::from_secs(60),
            admin_per_ip: 60,
        }
    }
}

struct Window {
    started: Instant,
    length: Duration,
    count: u32,
}

/// Counts events per key. A key's first event opens a window; when it ends the count starts over.
#[derive(Default)]
pub struct RateLimiter {
    windows: Mutex<HashMap<String, Window>>,
}

impl RateLimiter {
    /// Counts one event. Over the limit it answers with the seconds to wait.
    pub fn hit(&self, key: &str, limit: u32, window: Duration) -> Result<(), u64> {
        let now = Instant::now();
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let entry = windows.entry(key.to_owned()).or_insert(Window {
            started: now,
            length: window,
            count: 0,
        });
        if now.duration_since(entry.started) >= entry.length {
            entry.started = now;
            entry.length = window;
            entry.count = 0;
        }
        if entry.count >= limit {
            let left = entry
                .length
                .saturating_sub(now.duration_since(entry.started));
            return Err(left.as_secs() + u64::from(left.subsec_nanos() > 0));
        }
        entry.count += 1;
        Ok(())
    }

    /// Forgets windows that have ended.
    pub fn sweep(&self) {
        let now = Instant::now();
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        windows.retain(|_, w| now.duration_since(w.started) < w.length);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.windows.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_holds_within_the_window_and_names_the_wait() {
        let limiter = RateLimiter::default();
        let window = Duration::from_secs(60);
        for _ in 0..3 {
            assert!(limiter.hit("a", 3, window).is_ok());
        }
        let wait = limiter.hit("a", 3, window).unwrap_err();
        assert!((1..=60).contains(&wait));
        assert!(limiter.hit("b", 3, window).is_ok(), "keys are separate");
    }

    #[test]
    fn a_new_window_starts_when_the_old_one_ends() {
        let limiter = RateLimiter::default();
        let window = Duration::from_millis(30);
        assert!(limiter.hit("a", 1, window).is_ok());
        assert!(limiter.hit("a", 1, window).is_err());
        std::thread::sleep(Duration::from_millis(50));
        assert!(limiter.hit("a", 1, window).is_ok());
        limiter.sweep();
        std::thread::sleep(Duration::from_millis(50));
        limiter.sweep();
        assert_eq!(limiter.len(), 0);
    }
}
