//! Errors from the decision model.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the decision model could not be reached: {0}")]
    Unreachable(String),

    /// Reached, answered, and the answer could not be used — a non-2xx, a
    /// body that is not the expected JSON, or a choice that was never on
    /// the list. Kept apart from `Unreachable` because the two mean
    /// different things to a person reading the log.
    #[error("the decision model's answer was unusable: {0}")]
    Unusable(String),
}
