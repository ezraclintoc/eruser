#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unknown email provider {0:?} (only smtp is supported)")]
    UnknownProvider(String),

    #[error(transparent)]
    Invalid(#[from] ValidationError),

    #[error("SMTP authentication failed: check the username and app password")]
    Authentication,

    #[error("TLS error connecting to the mail server: check the host and port")]
    Tls,

    #[error("could not reach the mail server: check the host, port, and network")]
    Connection,

    #[error("the mail server rejected the message: {0}")]
    Rejected(String),

    #[error("SMTP transport is misconfigured: {0}")]
    Configuration(String),
}

/// Problems with the message itself, detected before any network traffic.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("address {0:?} contains characters that are not allowed in a header")]
    IllegalCharacters(String),

    #[error("address {0:?} is not a valid email address")]
    Malformed(String),

    #[error("invalid sender address")]
    Sender(#[source] Box<ValidationError>),

    #[error("invalid recipient address")]
    Recipient(#[source] Box<ValidationError>),

    #[error("subject contains a line break")]
    SubjectLineBreak,
}
