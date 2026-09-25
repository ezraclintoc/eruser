//! Errors from the drafting sidecar.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the drafting model could not be reached: {0}")]
    DrafterUnavailable(String),

    #[error("the draft was refused: {0}")]
    DraftRefused(#[from] super::ValidationError),

    /// A pre-written reply that could not be prepared at all — the library
    /// has nothing for the reply type, or a candidate does not render. Not a
    /// refusal: nothing was written to refuse.
    #[error("the reply's wording could not be prepared: {0}")]
    Wording(String),
}
