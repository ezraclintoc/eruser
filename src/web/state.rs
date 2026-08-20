//! Shared state handed to every handler.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::broker::BrokerDatabase;
use crate::config::Config;
use crate::history::Store;
use crate::template::Engine;

use super::job::{JobManager, JobPersistence};
use super::security::RateLimiter;
use super::session::SessionStore;

/// Everything the handlers share.
///
/// Cheap to clone: axum hands each handler its own copy.
#[derive(Clone)]
pub struct AppState {
    /// The loaded config, or `None` before the setup wizard has run.
    ///
    /// Behind a lock because the wizard writes it while requests are being
    /// served. Go stored a `*config.Config` and mutated it in place from a
    /// handler while other handlers read it.
    pub config: Arc<RwLock<Option<Config>>>,
    pub config_path: PathBuf,
    pub brokers: Arc<BrokerDatabase>,
    pub store: Store,
    pub engine: Arc<Engine>,
    pub sessions: SessionStore,
    pub rate_limiter: RateLimiter,
    pub jobs: JobManager,
    pub job_persistence: JobPersistence,
    pub templates: Arc<minijinja::Environment<'static>>,
    pub port: u16,
    /// The user rows belong to. One user until authentication lands.
    pub user_id: i64,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The config holds an SMTP password; print only whether it exists.
        f.debug_struct("AppState")
            .field("configured", &self.is_configured())
            .field("config_path", &self.config_path)
            .field("brokers", &self.brokers.brokers.len())
            .field("port", &self.port)
            .finish()
    }
}

impl AppState {
    /// A snapshot of the config, if there is one.
    pub fn config(&self) -> Option<Config> {
        self.read_config().clone()
    }

    /// Whether there is a config complete enough to send with.
    pub fn is_configured(&self) -> bool {
        self.read_config()
            .as_ref()
            .is_some_and(|config| config.validate().is_ok())
    }

    /// Replace the config and write it to disk.
    pub fn save_config(&self, config: Config) -> Result<(), crate::config::Error> {
        config.save(&self.config_path)?;
        *self.write_config() = Some(config);
        Ok(())
    }

    fn read_config(&self) -> std::sync::RwLockReadGuard<'_, Option<Config>> {
        self.config.read().unwrap_or_else(|p| p.into_inner())
    }

    fn write_config(&self) -> std::sync::RwLockWriteGuard<'_, Option<Config>> {
        self.config.write().unwrap_or_else(|p| p.into_inner())
    }
}
