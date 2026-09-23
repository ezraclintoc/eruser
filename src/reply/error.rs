//! Errors from the drafting sidecar.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the drafting model could not be reached: {0}")]
    DrafterUnavailable(String),

    #[error("the draft was refused: {0}")]
    DraftRefused(#[from] super::ValidationError),
}
