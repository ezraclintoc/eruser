//! The local web interface.
//!
//! Ported from `internal/web/`. axum and tower replace chi and its
//! middleware; the routes are the same ones the templates link to.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use axum::Router;
use axum::routing::{delete, get, post};

pub mod assets;
pub mod error;
pub mod handlers;
pub mod job;
pub mod security;
pub mod session;
pub mod state;
pub mod templates;
pub mod views;

use crate::broker::BrokerDatabase;
use crate::config::Config;
use crate::history::{DEFAULT_USER_ID, Store};
use crate::template::Engine;

use job::{JobManager, JobPersistence};
use security::RateLimiter;
use session::SessionStore;
use state::AppState;

/// How long a request may take before the server gives up on it.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The running web interface.
pub struct Server {
    state: AppState,
    address: SocketAddr,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to load the page templates")]
    Templates(#[source] minijinja::Error),

    #[error("{address} is not available — another program may be using port {port}")]
    Bind {
        address: SocketAddr,
        port: u16,
        #[source]
        source: std::io::Error,
    },

    #[error("the web server stopped unexpectedly")]
    Serve(#[source] std::io::Error),

    #[error("{0:?} is not an address this machine can listen on")]
    BadHost(String),
}

impl Server {
    /// Assemble the server. Does not bind a port yet.
    pub fn new(
        host: &str,
        port: u16,
        config: Option<Config>,
        config_path: PathBuf,
        brokers: BrokerDatabase,
        store: Store,
        engine: Engine,
    ) -> Result<Self, Error> {
        let ip: IpAddr = host.parse().map_err(|_| Error::BadHost(host.to_string()))?;

        let data_dir = config_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));

        let state = AppState {
            config: Arc::new(RwLock::new(config)),
            config_path,
            brokers: Arc::new(brokers),
            store,
            engine: Arc::new(engine),
            sessions: SessionStore::new(session::DEFAULT_TTL),
            rate_limiter: RateLimiter::default(),
            jobs: JobManager::new(),
            job_persistence: JobPersistence::new(data_dir),
            templates: Arc::new(templates::build().map_err(Error::Templates)?),
            port,
            user_id: DEFAULT_USER_ID,
        };

        Ok(Self {
            state,
            address: SocketAddr::new(ip, port),
        })
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Bind the port and serve until `shutdown` resolves.
    pub async fn serve(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), Error> {
        let listener = tokio::net::TcpListener::bind(self.address)
            .await
            .map_err(|source| Error::Bind {
                address: self.address,
                port: self.address.port(),
                source,
            })?;

        // Report the address actually bound: port 0 means "pick one", and
        // tests need to know which.
        let bound = listener.local_addr().unwrap_or(self.address);
        tracing::info!(%bound, "web interface listening");

        axum::serve(listener, router(self.state))
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(Error::Serve)
    }
}

/// Every route the interface serves.
pub fn router(state: AppState) -> Router {
    let pages = Router::new()
        .route("/", get(handlers::pages::dashboard))
        .route("/brokers", get(handlers::pages::brokers))
        .route("/history", get(handlers::pages::history))
        .route("/settings", get(handlers::pages::settings))
        .route(
            "/settings/inbox",
            post(handlers::pages::save_inbox_settings),
        )
        .route("/pipeline", get(handlers::pages::pipeline))
        .route("/tasks", get(handlers::pages::tasks))
        .route("/tasks/{task_id}", get(handlers::pages::task_detail))
        .route("/tasks/{task_id}/helper", get(handlers::pages::task_helper))
        .route(
            "/tasks/{task_id}/complete",
            post(handlers::pages::complete_task),
        )
        .route("/tasks/{task_id}/skip", post(handlers::pages::skip_task))
        .route("/forms", get(handlers::pages::forms))
        .route(
            "/forms/{broker_id}/complete",
            post(handlers::pages::complete_form),
        )
        .route("/forms/{broker_id}/skip", post(handlers::pages::skip_form));

    let setup = Router::new()
        .route("/", get(handlers::setup::index))
        .route("/welcome", get(handlers::setup::welcome))
        .route(
            "/profile",
            get(handlers::setup::show_profile).post(handlers::setup::save_profile),
        )
        .route(
            "/email",
            get(handlers::setup::show_email).post(handlers::setup::save_email),
        )
        .route("/test", get(handlers::setup::show_test))
        .route("/test/send", post(handlers::setup::send_test))
        .route("/complete", get(handlers::setup::complete));

    let api = Router::new()
        .route("/stats", get(handlers::api::stats))
        .route("/brokers", get(handlers::api::brokers))
        .route("/history", get(handlers::api::history))
        .route("/history/failed", delete(handlers::api::delete_failed))
        .route("/send/{broker_id}", post(handlers::api::send_one))
        .route("/send-all", post(handlers::api::send_all))
        .route("/job/active", get(handlers::api::active_job))
        .route("/job/{job_id}/status", get(handlers::api::job_status))
        .route("/job/{job_id}/cancel", post(handlers::api::cancel_job))
        .route("/pipeline/stats", get(handlers::api::pipeline_stats))
        .route("/pipeline/responses", get(handlers::api::responses))
        .route("/pipeline/tasks", get(handlers::api::tasks))
        .route("/inbox/scan", post(handlers::api::inbox_scan))
        .route("/inbox/rescan", post(handlers::api::inbox_rescan))
        .route("/inbox/reclassify", post(handlers::api::inbox_reclassify));

    Router::new()
        .merge(pages)
        .nest("/setup", setup)
        .nest("/api", api)
        // Static files skip CSRF and rate limiting: they change no state, and
        // one page load asks for several of them at once.
        .route("/static/{*path}", get(assets::serve))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::csrf_protect,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::rate_limit,
        ))
        .layer(axum::middleware::from_fn(security::security_headers))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .fallback(handlers::not_found)
        .with_state(state)
}

#[cfg(test)]
mod tests;
