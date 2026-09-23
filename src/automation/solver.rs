//! Attempting a challenge before handing the page to a person.
//!
//! Detecting a CAPTCHA has always been honest here: eruser says what is in
//! the way and leaves the page alone. The next step up is *trying* — letting
//! a solver take a shot, and checking the page afterwards to see whether the
//! challenge actually went away.
//!
//! The rule this module is built around: **a solver is an optimisation, never
//! a gate.** Every path that is not a verified success lands in exactly the
//! place the tool has always landed — the page untouched, a screenshot taken,
//! a captcha task queued for a person. A solver that rots, lies, or cannot be
//! reached costs nothing but a log line.
//!
//! No solving models ship with eruser. The solver is a sidecar process the
//! user runs themselves — anything that speaks the small JSON contract below,
//! such as a self-hosted hCaptcha challenge solver or an OCR service. That
//! keeps the solving models (and their licences) outside this repository, and
//! keeps a privacy tool from shipping models nobody audited.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::captcha::Captcha;

/// How long to wait after a solver claims success before re-reading the
/// page. Challenge widgets take a moment to hand control back.
const SETTLE_DELAY: Duration = Duration::from_millis(1500);

/// What came of one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolveOutcome {
    /// The solver says it cleared the challenge. The caller still re-checks
    /// the page — claiming success and succeeding are different things.
    Solved,
    /// The solver was reached, looked, and could not.
    Failed(String),
    /// The solver could not be reached, or answered nonsense.
    Unavailable(String),
}

impl SolveOutcome {
    /// The human-facing line for the progress output and the task notes.
    pub fn detail(&self) -> String {
        match self {
            Self::Solved => "the solver cleared it".to_string(),
            Self::Failed(detail) => detail.clone(),
            Self::Unavailable(detail) => format!("the solver could not be reached: {detail}"),
        }
    }
}

/// Something that has a go at the challenge standing in front of the form.
///
/// Kept object-safe via `async_trait`, on the same pattern as
/// [`crate::email::Sender`], so the CLI and the web runner can share one.
#[async_trait::async_trait]
pub trait CaptchaSolver: Send + Sync {
    /// Try to clear whatever challenge is on the page. Implementations
    /// should assume the page is live in a browser they cannot see; they
    /// get the challenge eruser detected and the URL it was detected on.
    async fn solve(&self, challenge: &Captcha, page_url: &str) -> SolveOutcome;

    /// Short name, for logs and progress output.
    fn name(&self) -> &'static str;
}

/// The solver a local service provides.
///
/// The contract is deliberately tiny, so any solving backend can speak it:
///
/// ```text
/// POST <url>
/// {"kind": "hcaptcha", "page_url": "https://…"}
/// → 200 {"status": "solved"}
/// → 200 {"status": "failed", "detail": "why"}
/// ```
///
/// Anything else — a non-200, unparseable body, or a timeout — reads as
/// [`SolveOutcome::Unavailable`], which is reported and never retried: a
/// broken sidecar should not slow a run down.
pub struct SidecarSolver {
    endpoint: String,
    token: Option<String>,
    client: reqwest::Client,
}

/// What eruser asks the sidecar.
#[derive(Serialize)]
struct SolveRequest<'a> {
    kind: &'a str,
    page_url: &'a str,
}

/// What the sidecar is expected to answer.
#[derive(Deserialize)]
struct SolveAnswer {
    status: String,
    #[serde(default)]
    detail: String,
}

impl SidecarSolver {
    pub fn new(endpoint: impl Into<String>, token: Option<String>, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();

        Self {
            endpoint: endpoint.into(),
            token,
            client,
        }
    }
}

/// Build the solver the settings describe. `None` means every challenge is
/// left for a person, which is the default and always the fallback.
///
/// A solver that is switched on but half-configured is demoted to a warning
/// rather than an error, on the same reasoning as the sender pool: one stale
/// piece of setup should not stop the rest of the run.
pub fn from_config(config: &crate::config::CaptchaSolverConfig) -> Option<Arc<dyn CaptchaSolver>> {
    if !config.enabled {
        return None;
    }

    if config.url.trim().is_empty() {
        tracing::warn!(
            "captcha_solver is enabled but no url is set; challenges will be left for a person"
        );
        return None;
    }

    let token = (!config.token.trim().is_empty()).then(|| config.token.clone());
    let solver = SidecarSolver::new(
        config.url.trim().to_string(),
        token,
        Duration::from_secs(config.timeout_sec.max(1)),
    );
    Some(Arc::new(solver))
}

#[async_trait::async_trait]
impl CaptchaSolver for SidecarSolver {
    async fn solve(&self, challenge: &Captcha, page_url: &str) -> SolveOutcome {
        let request = SolveRequest {
            kind: challenge.kind.as_str(),
            page_url,
        };

        let mut send = self.client.post(&self.endpoint).json(&request);
        if let Some(token) = &self.token {
            send = send.bearer_auth(token);
        }

        let answer = match send.send().await {
            Ok(answer) => answer,
            Err(error) => return SolveOutcome::Unavailable(error.to_string()),
        };

        let status = answer.status().as_u16();
        let body = answer.text().await.unwrap_or_default();
        parse_answer(status, &body)
    }

    fn name(&self) -> &'static str {
        "sidecar"
    }
}

/// Turn an HTTP answer into an outcome. Split out from the request so the
/// contract is testable without a service on the other end.
fn parse_answer(status: u16, body: &str) -> SolveOutcome {
    if !(200..300).contains(&status) {
        return SolveOutcome::Unavailable(format!("the solver answered HTTP {status}"));
    }

    match serde_json::from_str::<SolveAnswer>(body) {
        Ok(answer) if answer.status == "solved" => SolveOutcome::Solved,
        Ok(answer) if answer.status == "failed" => {
            let detail = if answer.detail.trim().is_empty() {
                "the solver could not clear the challenge".to_string()
            } else {
                answer.detail
            };
            SolveOutcome::Failed(detail)
        }
        Ok(answer) => SolveOutcome::Failed(format!(
            "the solver answered with a status eruser does not understand: {:?}",
            answer.status
        )),
        Err(_) => {
            SolveOutcome::Failed("the solver's answer was not the JSON eruser expects".to_string())
        }
    }
}

/// Re-check a page after a solver claims success, and say whether the
/// challenge is genuinely gone.
///
/// This is the trust-but-verify half. A solver that has rotted can click
/// through a widget it used to beat and still report victory; the only
/// evidence worth acting on is the page itself. Returns the challenge to
/// record when it is still there — freshly detected, so the notes describe
/// the page as it is now.
pub async fn verify(
    page: &chromiumoxide::Page,
    solver: &dyn CaptchaSolver,
    _challenge: &Captcha,
) -> Result<Option<Captcha>, super::browser::Error> {
    tokio::time::sleep(SETTLE_DELAY).await;

    let html = page
        .content()
        .await
        .map_err(|error| super::browser::Error::ReadPage {
            message: error.to_string(),
        })?;

    let still = super::captcha::detect_in_html(&html);
    if let Some(remaining) = &still
        && remaining.blocks_automation()
    {
        tracing::info!(
            solver = solver.name(),
            challenge = remaining.kind.as_str(),
            "the solver claimed success but the challenge is still on the page"
        );
    }
    Ok(still.filter(Captcha::blocks_automation))
}

/// Whether a fresh read of a page still shows a challenge that blocks the
/// fill. The whole of the verify decision, minus the page fetch, so the
/// bar a solver has to clear is testable without a browser.
pub fn still_blocked_after(html: &str) -> bool {
    super::captcha::detect_in_html(html).is_some_and(|found| found.blocks_automation())
}

#[cfg(test)]
mod tests;
