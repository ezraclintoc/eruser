//! Errors that reach the browser.
//!
//! Go's handlers called `http.Error` with the raw error text, which put
//! database and SMTP internals on screen. Here a `WebError` decides what the
//! visitor sees and the detail goes to the log instead.

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("page not found")]
    NotFound,

    #[error("{0}")]
    BadRequest(String),

    #[error("this form has expired or came from somewhere else — reload the page and try again")]
    InvalidCsrf,

    #[error("too many requests — slow down for a moment")]
    RateLimited,

    #[error("eruser is not set up yet")]
    NotConfigured,

    #[error("a send is already running")]
    JobAlreadyRunning,

    #[error(transparent)]
    History(#[from] crate::history::Error),

    #[error(transparent)]
    Config(#[from] crate::config::Error),

    #[error(transparent)]
    Email(#[from] crate::email::Error),

    #[error(transparent)]
    Template(#[from] crate::template::Error),

    #[error("failed to render the page")]
    Render(#[from] minijinja::Error),

    #[error("failed to save the pending job")]
    Job(#[from] super::job::Error),

    #[error("could not read the mailbox")]
    Inbox(#[from] crate::inbox::scan::Error),
}

impl WebError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::InvalidCsrf => StatusCode::FORBIDDEN,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::NotConfigured => StatusCode::CONFLICT,
            Self::JobAlreadyRunning => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// What the visitor is told.
    ///
    /// Errors the user caused explain themselves. Everything else says
    /// something went wrong and nothing more: the detail is in the log, and
    /// an SMTP or SQLite error on screen helps nobody and can leak a host,
    /// a path, or a username.
    pub fn user_message(&self) -> String {
        match self {
            Self::NotFound
            | Self::BadRequest(_)
            | Self::InvalidCsrf
            | Self::RateLimited
            | Self::NotConfigured
            | Self::JobAlreadyRunning => self.to_string(),
            _ => "Something went wrong. Check the terminal running eruser for details.".to_string(),
        }
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let status = self.status();

        if status.is_server_error() {
            tracing::error!(error = %crate::send::error_chain(&self), "request failed");
        }

        let body = format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <title>{status}</title></head><body style=\"font-family:system-ui;padding:2rem\">\
             <h1>{status}</h1><p>{}</p><p><a href=\"/\">Back to the dashboard</a></p>\
             </body></html>",
            html_escape(&self.user_message())
        );

        (status, Html(body)).into_response()
    }
}

/// Escape the five characters that matter in HTML text and attributes.
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_facing_errors_explain_themselves() {
        assert!(WebError::InvalidCsrf.user_message().contains("reload"));
        assert!(WebError::RateLimited.user_message().contains("slow down"));
        assert_eq!(
            WebError::BadRequest("pick a template first".into()).user_message(),
            "pick a template first"
        );
    }

    /// A database or SMTP error on screen helps nobody and can name a host,
    /// a path, or a username.
    #[test]
    fn internal_errors_do_not_reach_the_browser() {
        let error = WebError::Email(crate::email::Error::Authentication);
        let message = error.user_message();

        assert!(!message.contains("authentication"), "{message}");
        assert!(!message.contains("password"), "{message}");
        assert!(message.contains("Something went wrong"));
    }

    #[test]
    fn statuses_match_the_kind_of_failure() {
        assert_eq!(WebError::NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(WebError::InvalidCsrf.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            WebError::RateLimited.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(WebError::JobAlreadyRunning.status(), StatusCode::CONFLICT);
        assert!(
            WebError::Email(crate::email::Error::Connection)
                .status()
                .is_server_error()
        );
    }

    #[test]
    fn escaping_covers_the_characters_that_break_out_of_markup() {
        assert_eq!(
            html_escape(r#"<script>alert("x" & 'y')</script>"#),
            "&lt;script&gt;alert(&quot;x&quot; &amp; &#x27;y&#x27;)&lt;/script&gt;"
        );
    }

    /// The error page interpolates the message, so a message carrying markup
    /// must not become markup.
    #[test]
    fn an_error_message_cannot_inject_markup_into_the_error_page() {
        let error = WebError::BadRequest("<img src=x onerror=alert(1)>".into());
        let body = html_escape(&error.user_message());
        assert!(!body.contains("<img"));
        assert!(body.contains("&lt;img"));
    }
}
