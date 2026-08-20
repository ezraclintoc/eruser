use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read config file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config file {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_norway::Error,
    },

    #[error("failed to serialize config")]
    Serialize(#[source] serde_norway::Error),

    #[error(
        "config file {path} has insecure permissions {mode:04o}; it holds an email password, run: chmod 600 {path}"
    )]
    InsecurePermissions { path: PathBuf, mode: u32 },

    #[error(transparent)]
    Invalid(#[from] ValidationError),
}

/// Everything that can be missing or wrong in an otherwise well-formed config.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("profile: first_name and last_name are required")]
    MissingName,
    #[error("profile: email is required")]
    MissingProfileEmail,
    #[error("email: provider is required")]
    MissingProvider,
    #[error("email: from address is required")]
    MissingFrom,
    #[error("email: unknown provider {0:?} (only smtp is supported)")]
    UnknownProvider(String),
    #[error("email.smtp: host is required")]
    MissingSmtpHost,
    #[error("email.smtp: port is required")]
    MissingSmtpPort,
    #[error("inbox: monitoring is not enabled in config")]
    InboxDisabled,
    #[error("inbox: email address is required")]
    MissingInboxEmail,
    #[error("inbox: password (app password) is required")]
    MissingInboxPassword,
    #[error("inbox: IMAP server is required")]
    MissingInboxServer,
    #[error("inbox: IMAP port is required")]
    MissingInboxPort,
}
