//! Shared state handed to every handler.

use std::path::PathBuf;
use std::sync::Arc;

use crate::broker::BrokerDatabase;
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
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The config holds an SMTP password; print only whether it exists.
        f.debug_struct("AppState")
            .field("config_path", &self.config_path)
            .field("brokers", &self.brokers.brokers.len())
            .field("port", &self.port)
            .finish()
    }
}

impl AppState {
    /// The machine-scoped pipeline settings, read from config.yaml at call
    /// time. These stay file-scoped rather than living in the database:
    /// they describe what this installation may do on its own, not what one
    /// person has configured.
    pub fn pipeline(&self) -> crate::config::Pipeline {
        let Ok((config, _)) = crate::config::Config::load_lenient(&self.config_path) else {
            return crate::config::Pipeline::default();
        };
        config.pipeline
    }

    /// The auto-send whitelist: the machine's setting, or nothing when the
    /// file does not say. Never merged with anything per-person — a
    /// household shares one instance, but not necessarily one appetite for
    /// automatic mail.
    pub fn auto_send_whitelist(&self, _config: &crate::config::Config) -> Vec<String> {
        self.pipeline().ai.auto_send
    }
}
