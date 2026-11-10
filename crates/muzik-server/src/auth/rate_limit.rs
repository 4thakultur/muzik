//! In-memory sliding-window limiters for failed authentication attempts.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Counts failures per key in a sliding window.
pub struct LoginLimiter {
    window: Duration,
    max_failures: usize,
    failures: Mutex<HashMap<String, Vec<Instant>>>,
}

impl LoginLimiter {
    pub fn new(max_failures: usize, window: Duration) -> Self {
        Self {
            window,
            max_failures,
            failures: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_locked(&self, key: &str) -> bool {
        let Ok(mut map) = self.failures.lock() else {
            return false;
        };
        let now = Instant::now();
        match map.get_mut(key) {
            Some(v) => {
                v.retain(|t| now.duration_since(*t) < self.window);
                v.len() >= self.max_failures
            }
            None => false,
        }
    }

    pub fn record_failure(&self, key: &str) {
        if let Ok(mut map) = self.failures.lock() {
            let now = Instant::now();
            // Opportunistic cleanup to bound memory.
            if map.len() > 10_000 {
                map.retain(|_, v| v.iter().any(|t| now.duration_since(*t) < self.window));
            }
            let v = map.entry(key.to_owned()).or_default();
            v.push(now);
            // Never keep more timestamps than needed to decide.
            if v.len() > self.max_failures {
                let extra = v.len() - self.max_failures;
                v.drain(..extra);
            }
        }
    }

    pub fn reset(&self, key: &str) {
        if let Ok(mut map) = self.failures.lock() {
            map.remove(key);
        }
    }
}

/// Failed-authentication limits (passwords and second factors), in four tiers:
///
/// - **user + IP** (10 / 15 min): the normal brute-force lockout.
/// - **IP** (30 / 15 min): one client spraying many usernames.
/// - **user** (100 / 15 min): distributed guessing against one account.
/// - **second factor** (10 / hour, per user): wrong authenticator or
///   recovery codes.
///
/// The per-user tier is deliberately much higher than the user+IP one, so a
/// single attacker cannot lock a victim out of their own account: they hit
/// their own IP limits long before the account-wide one.
///
/// The second-factor tier can be strict because only someone who already
/// has the password (or a session) can produce such failures. Without it, an
/// attacker who knows the password could spread 100 code guesses per 15 min
/// over many addresses, roughly a 3% chance per day of hitting a valid code.
pub struct AuthLimiter {
    user_ip: LoginLimiter,
    ip: LoginLimiter,
    user: LoginLimiter,
    second_factor: LoginLimiter,
}

impl Default for AuthLimiter {
    fn default() -> Self {
        let window = Duration::from_secs(15 * 60);
        Self {
            user_ip: LoginLimiter::new(10, window),
            ip: LoginLimiter::new(30, window),
            user: LoginLimiter::new(100, window),
            second_factor: LoginLimiter::new(10, Duration::from_secs(3600)),
        }
    }
}

impl AuthLimiter {
    fn keys(username: &str, ip: &str) -> (String, String, String) {
        let u = username.to_ascii_lowercase();
        (format!("{u}|{ip}"), ip.to_owned(), u)
    }

    pub fn is_locked(&self, username: &str, ip: &str) -> bool {
        let (ui, i, u) = Self::keys(username, ip);
        self.user_ip.is_locked(&ui)
            || self.ip.is_locked(&i)
            || self.user.is_locked(&u)
            || self.second_factor.is_locked(&u)
    }

    pub fn record_failure(&self, username: &str, ip: &str) {
        let (ui, i, u) = Self::keys(username, ip);
        self.user_ip.record_failure(&ui);
        self.ip.record_failure(&i);
        self.user.record_failure(&u);
    }

    /// A wrong second factor: counts toward every tier plus the stricter
    /// per-account second-factor one.
    pub fn record_second_factor_failure(&self, username: &str, ip: &str) {
        self.record_failure(username, ip);
        let (_, _, u) = Self::keys(username, ip);
        self.second_factor.record_failure(&u);
    }

    /// Clears the user+IP counter after a fully successful sign-in.
    pub fn record_success(&self, username: &str, ip: &str) {
        let (ui, _, _) = Self::keys(username, ip);
        self.user_ip.reset(&ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_after_max_failures() {
        let l = LoginLimiter::new(3, Duration::from_secs(60));
        for _ in 0..2 {
            l.record_failure("u:alice");
        }
        assert!(!l.is_locked("u:alice"));
        l.record_failure("u:alice");
        assert!(l.is_locked("u:alice"));
        assert!(!l.is_locked("u:bob"));
        l.reset("u:alice");
        assert!(!l.is_locked("u:alice"));
    }

    #[test]
    fn one_ip_cannot_lock_out_a_user_everywhere() {
        let l = AuthLimiter::default();
        for _ in 0..10 {
            l.record_failure("Alice", "203.0.113.9");
        }
        assert!(l.is_locked("alice", "203.0.113.9"));
        // The real user, from their own address, can still sign in.
        assert!(!l.is_locked("alice", "198.51.100.4"));
    }

    #[test]
    fn wrong_codes_lock_the_account_from_any_address() {
        let l = AuthLimiter::default();
        for i in 0..10 {
            l.record_second_factor_failure("alice", &format!("203.0.113.{i}"));
        }
        // Ten addresses, one failure each: only the account-wide tier trips.
        assert!(l.is_locked("alice", "198.51.100.4"));
        assert!(!l.is_locked("bob", "203.0.113.1"));
    }

    #[test]
    fn ip_spraying_usernames_is_limited() {
        let l = AuthLimiter::default();
        for i in 0..30 {
            l.record_failure(&format!("user{i}"), "203.0.113.9");
        }
        assert!(l.is_locked("someone-else", "203.0.113.9"));
    }
}
