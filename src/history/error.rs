use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to create the directory for the history database at {path}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to open the history database at {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: sqlx::Error,
    },

    #[error("failed to migrate the history database")]
    Migrate(#[source] sqlx::migrate::MigrateError),

    #[error("database query failed")]
    Query(#[from] sqlx::Error),

    #[error("unknown {kind} value {value:?} in the database")]
    UnknownEnumValue { kind: &'static str, value: String },
}
