//! Server-side sessions for the setup wizard.
//!
//! Ported from `internal/web/session.go`. The wizard collects an SMTP
//! password before it has anywhere to save it, so that value lives here and
//! the browser only ever holds an opaque id.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::RngExt;

use crate::config::{EmailConfig, Profile};

/// How long a wizard session survives without activity.
pub const DEFAULT_TTL: Duration = Duration::from_secs(30 * 60);

/// The name of the cookie holding the session id.
pub const COOKIE_NAME: &str = "eruser_session";

/// Wizard state held on the server.
#[derive(Debug, Clone, Default)]
pub struct Session {
    pub step: String,
    pub profile: Profile,
    pub email: EmailConfig,
    pub created_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// A set of live sessions, expiring on read.
///
/// Go ran a background goroutine to sweep expired entries every minute. Here
/// expiry is enforced on access and a sweep runs opportunistically on
/// creation, so there is no task to leak if the server shuts down.
#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<Mutex<HashMap<String, Session>>>,
    ttl: chrono::Duration,
}

impl std::fmt::Debug for SessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the contents: sessions hold an SMTP password.
        f.debug_struct("SessionStore")
            .field("count", &self.count())
            .finish()
    }
}

impl SessionStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl: chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::minutes(30)),
        }
    }

    /// Start a session and return its id.
    pub fn create(&self) -> String {
        let id = generate_id();
        let now = Utc::now();

        let mut sessions = self.lock();
        // Opportunistic sweep, so an abandoned wizard does not accumulate.
        sessions.retain(|_, session| !is_expired(session, now));
        sessions.insert(
            id.clone(),
            Session {
                created_at: Some(now),
                expires_at: Some(now + self.ttl),
                ..Default::default()
            },
        );
        id
    }

    /// Fetch a session, dropping it if it has expired.
    pub fn get(&self, id: &str) -> Option<Session> {
        if id.is_empty() {
            return None;
        }
        let now = Utc::now();
        let mut sessions = self.lock();

        match sessions.get(id) {
            Some(session) if is_expired(session, now) => {
                sessions.remove(id);
                None
            }
            Some(session) => Some(session.clone()),
            None => None,
        }
    }

    /// Mutate a session in place and extend its expiry.
    ///
    /// Returns false if there was no live session, so a caller can start a
    /// new wizard rather than writing into nothing.
    pub fn update(&self, id: &str, apply: impl FnOnce(&mut Session)) -> bool {
        let now = Utc::now();
        let mut sessions = self.lock();

        let Some(session) = sessions.get_mut(id) else {
            return false;
        };
        if is_expired(session, now) {
            sessions.remove(id);
            return false;
        }

        apply(session);
        session.expires_at = Some(now + self.ttl);
        true
    }

    pub fn delete(&self, id: &str) {
        self.lock().remove(id);
    }

    pub fn count(&self) -> usize {
        self.lock().len()
    }

    /// A poisoned lock only means some other thread panicked while holding
    /// it; the session map itself is still consistent, and refusing to serve
    /// the wizard forever afterwards would be worse.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn is_expired(session: &Session, now: DateTime<Utc>) -> bool {
    session.expires_at.is_some_and(|expiry| now > expiry)
}

/// 256 bits of randomness, hex encoded.
fn generate_id() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_session_can_be_read_back() {
        let store = SessionStore::new(DEFAULT_TTL);
        let id = store.create();

        let session = store.get(&id).expect("the session should exist");
        assert_eq!(session.step, "");
        assert!(session.created_at.is_some());
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn session_ids_are_long_and_unique() {
        let store = SessionStore::new(DEFAULT_TTL);
        let a = store.create();
        let b = store.create();

        assert_ne!(a, b);
        assert_eq!(a.len(), 64, "256 bits, hex encoded");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn an_unknown_or_empty_id_finds_nothing() {
        let store = SessionStore::new(DEFAULT_TTL);
        assert!(store.get("").is_none());
        assert!(store.get("deadbeef").is_none());
    }

    #[test]
    fn updates_are_visible_to_the_next_read() {
        let store = SessionStore::new(DEFAULT_TTL);
        let id = store.create();

        assert!(store.update(&id, |session| {
            session.step = "profile".into();
            session.profile.first_name = "Jane".into();
        }));

        let session = store.get(&id).unwrap();
        assert_eq!(session.step, "profile");
        assert_eq!(session.profile.first_name, "Jane");
    }

    #[test]
    fn updating_an_unknown_session_reports_false() {
        let store = SessionStore::new(DEFAULT_TTL);
        assert!(!store.update("deadbeef", |_| {}));
    }

    #[test]
    fn an_expired_session_is_gone_and_forgotten() {
        let store = SessionStore::new(Duration::ZERO);
        let id = store.create();

        assert!(store.get(&id).is_none());
        assert_eq!(
            store.count(),
            0,
            "reading an expired session should drop it"
        );
    }

    #[test]
    fn an_expired_session_cannot_be_updated() {
        let store = SessionStore::new(Duration::ZERO);
        let id = store.create();
        assert!(!store.update(&id, |session| session.step = "email".into()));
    }

    #[test]
    fn creating_a_session_sweeps_expired_ones() {
        let store = SessionStore::new(Duration::ZERO);
        store.create();
        store.create();
        // Each create sweeps first, so only the newest survives.
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn a_deleted_session_is_gone() {
        let store = SessionStore::new(DEFAULT_TTL);
        let id = store.create();
        store.delete(&id);
        assert!(store.get(&id).is_none());
    }

    /// Sessions hold an SMTP password until the wizard finishes.
    #[test]
    fn debug_output_does_not_print_session_contents() {
        let store = SessionStore::new(DEFAULT_TTL);
        let id = store.create();
        store.update(&id, |session| {
            session.email.smtp.password = "hunter2".into();
        });

        let debug = format!("{store:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
    }

    #[test]
    fn a_poisoned_lock_does_not_take_the_store_down() {
        let store = SessionStore::new(DEFAULT_TTL);
        let id = store.create();

        let poisoner = store.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock();
            panic!("poison the mutex");
        })
        .join();

        assert!(store.get(&id).is_some(), "the store should still work");
    }
}
