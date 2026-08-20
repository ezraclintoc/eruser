use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read broker file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write broker file {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse broker file {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_norway::Error,
    },

    #[error("failed to serialize broker database")]
    Serialize(#[source] serde_norway::Error),

    #[error("broker with ID {0:?} already exists")]
    DuplicateId(String),
}
