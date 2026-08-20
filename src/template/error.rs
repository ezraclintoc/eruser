#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to parse template {template:?}")]
    Parse {
        template: String,
        #[source]
        source: minijinja::Error,
    },

    #[error("failed to render template {template:?}")]
    Render {
        template: String,
        #[source]
        source: minijinja::Error,
    },

    #[error("unknown template {0:?} (expected one of: ccpa, gdpr, generic)")]
    Unknown(String),
}
