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
    /// Relay tickets one account may ask for per `relay_ticket_window`. The limit hangs on the
    /// account because an endpoint ID costs nothing to make.
    pub relay_tickets_per_account: u32,
    pub relay_ticket_window: Duration,
    /// Avatar uploads per account in `avatar_window`.
    pub avatar_uploads: u32,
    pub avatar_window: Duration,
    /// Feed reactions one account may set or take back per `feed_reaction_window`.
    pub feed_reactions: u32,
    pub feed_reaction_window: Duration,
    /// Messages one account may send per `messages_window`.
    pub messages: u32,
    pub messages_window: Duration,
    /// Edits, unsends and reactions one account may make per `message_changes_window`.
    pub message_changes: u32,
    pub message_changes_window: Duration,
    /// Typing signals one account may send over all conversations per `typing_window`.
    pub typing: u32,
    pub typing_window: Duration,
    /// Calls one account may start per `calls_window`.
    pub calls: u32,
    pub calls_window: Duration,
    /// Conversations one account may start with people who are not friends per
    /// `strangers_window`.
    pub strangers: u32,
    pub strangers_window: Duration,
    /// Reports one account may file per `report_window`, besides the daily limit in the settings.
    pub report_per_account: u32,
    pub report_window: Duration,
    /// Appeals one account may file per `appeal_window`.
    pub appeal_per_account: u32,
    pub appeal_window: Duration,
    /// Reads of public profiles on the web portal per address and `portal_public_window`.
    pub portal_public: u32,
    pub portal_public_window: Duration,
    /// Sessions the web portal starts for a client address per `auth_window`: every page load
    /// without a cookie starts one, and many players can share an address, so this is generous.
    pub portal_sessions_per_ip: u32,
    /// New join codes, and redemptions of join codes, one account may make per
    /// `join_code_window`.
    pub join_codes_new: u32,
    pub join_codes_redeemed: u32,
    pub join_code_window: Duration,
    /// Connections one account may start per `link_start_window`.
    pub link_starts: u32,
    pub link_start_window: Duration,
    /// Run reports one account may send per `run_report_window`.
    pub run_reports: u32,
    pub run_report_window: Duration,
    /// Trades of Discord SDK codes and refresh tokens one account may make per
    /// `discord_grant_window`. A player needs one about every seven days.
    pub discord_grants_per_account: u32,
    pub discord_grant_window: Duration,
    /// Listings one account may post per `lfg_window`, requests to join it may make, and reads
    /// of the listing board.
    pub lfg_posts: u32,
    pub lfg_asks: u32,
    pub lfg_reads: u32,
    pub lfg_window: Duration,
    /// Screenshots and clips one account may upload per `media_window`.
    pub media_uploads: u32,
    pub media_window: Duration,
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
            relay_tickets_per_account: 20,
            relay_ticket_window: Duration::from_secs(60 * 60),
            avatar_uploads: 5,
            avatar_window: Duration::from_secs(60 * 60),
            feed_reactions: 60,
            feed_reaction_window: Duration::from_secs(60),
            messages: 30,
            messages_window: Duration::from_secs(60),
            message_changes: 60,
            message_changes_window: Duration::from_secs(60),
            typing: 30,
            typing_window: Duration::from_secs(60),
            calls: 10,
            calls_window: Duration::from_secs(60),
            strangers: 10,
            strangers_window: Duration::from_secs(60 * 60),
            report_per_account: 5,
            report_window: Duration::from_secs(10 * 60),
            appeal_per_account: 10,
            appeal_window: Duration::from_secs(60 * 60),
            portal_public: 120,
            portal_public_window: Duration::from_secs(60),
            portal_sessions_per_ip: 300,
            join_codes_new: 10,
            join_codes_redeemed: 30,
            join_code_window: Duration::from_secs(60 * 60),
            link_starts: 10,
            link_start_window: Duration::from_secs(60 * 60),
            run_reports: 30,
            run_report_window: Duration::from_secs(60 * 60),
            discord_grants_per_account: 10,
            discord_grant_window: Duration::from_secs(60 * 60),
            lfg_posts: 6,
            lfg_asks: 30,
            lfg_reads: 60,
            lfg_window: Duration::from_secs(10 * 60),
            media_uploads: 20,
            media_window: Duration::from_secs(60 * 60),
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
        self.hit_at(key, limit, window, Instant::now())
    }

    /// Like [`RateLimiter::hit`], at a moment of the caller's choosing. A seam for tests.
    pub fn hit_at(&self, key: &str, limit: u32, window: Duration, now: Instant) -> Result<(), u64> {
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
    fn the_count_starts_over_when_the_window_ends_by_the_given_clock() {
        let limiter = RateLimiter::default();
        let window = Duration::from_secs(3600);
        let start = Instant::now();
        for _ in 0..20 {
            assert!(limiter.hit_at("a", 20, window, start).is_ok());
        }
        let wait = limiter.hit_at("a", 20, window, start).unwrap_err();
        assert_eq!(wait, 3600);
        let almost = start + Duration::from_secs(3599);
        assert_eq!(limiter.hit_at("a", 20, window, almost), Err(1));
        let later = start + window;
        assert!(limiter.hit_at("a", 20, window, later).is_ok());
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
