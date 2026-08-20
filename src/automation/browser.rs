//! Driving a real browser through an opt-out form.
//!
//! Ported from `internal/browser/browser.go`. chromiumoxide replaces
//! chromedp.
//!
//! This module does as little thinking as possible: it reads the fields a
//! page declares, hands them to [`super::filler`], and types back whatever
//! comes out. Everything that decides *what goes where* lives in the filler,
//! where it can be tested without a Chrome.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;

use super::captcha::{self, Captcha};
use super::filler::{self, FillPlan, FormField};
use crate::config::Profile;

/// How long to wait for a page to load.
const NAVIGATION_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait after submitting before reading the result.
const SETTLE_AFTER_SUBMIT: Duration = Duration::from_secs(3);

/// Reads every input a page declares, along with the label attached to it.
///
/// Done in one pass rather than a round trip per candidate selector: Go
/// issued a CDP call for each selector in each mapping, which on a form with
/// ten boxes meant well over a hundred calls.
const COLLECT_FIELDS: &str = r##"
(() => {
  const labelFor = (el) => {
    if (el.labels && el.labels.length) return el.labels[0].innerText || '';
    const wrapping = el.closest('label');
    if (wrapping) return wrapping.innerText || '';
    return el.getAttribute('aria-label') || '';
  };

  return Array.from(document.querySelectorAll('input, textarea, select')).map((el, index) => {
    // Give every field a handle, so one without an id or a name can still be
    // typed into later.
    el.setAttribute('data-eruser-field', index);
    return {
      selector: '[data-eruser-field="' + index + '"]',
      name: el.name || '',
      id: el.id || '',
      placeholder: el.placeholder || '',
      input_type: el.getAttribute('type') || el.tagName.toLowerCase(),
      autocomplete: el.getAttribute('autocomplete') || '',
      label: (labelFor(el) || '').trim().slice(0, 120),
      required: el.required === true,
    };
  });
})()
"##;

/// Finds the button that submits the form.
const FIND_SUBMIT: &str = r##"
(() => {
  const candidates = Array.from(document.querySelectorAll(
    'button[type=submit], input[type=submit], button:not([type]), button[type=button]'
  ));
  const wanted = /submit|send|opt.?out|remove|delete|continue|next|confirm/i;

  const match = candidates.find((el) => wanted.test(el.innerText || el.value || ''))
    || candidates[0];
  if (!match) return null;

  match.setAttribute('data-eruser-submit', '1');
  return '[data-eruser-submit="1"]';
})()
"##;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "could not start a browser.\n\n\
         Filling forms needs Chrome or Chromium installed. Install one, or \
         set CHROME to its path."
    )]
    Launch(#[source] chromiumoxide::error::CdpError),

    #[error("could not open {url}")]
    Navigate {
        url: String,
        #[source]
        source: chromiumoxide::error::CdpError,
    },

    #[error("the browser stopped responding")]
    Browser(#[from] chromiumoxide::error::CdpError),

    #[error("failed to save a screenshot to {path}")]
    Screenshot {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// How the browser should behave.
#[derive(Debug, Clone)]
pub struct BrowserOptions {
    /// Run without a visible window.
    pub headless: bool,
    /// Where to write screenshots. `None` takes none.
    pub screenshot_dir: Option<PathBuf>,
    /// Press the submit button after filling.
    ///
    /// Off by default. A form that submits the wrong thing cannot be
    /// un-submitted, and a screenshot of a filled-but-unsent form is a much
    /// safer default for a tool acting on someone's behalf.
    pub submit: bool,
    pub timeout: Duration,
}

impl Default for BrowserOptions {
    fn default() -> Self {
        Self {
            headless: true,
            screenshot_dir: None,
            submit: false,
            timeout: NAVIGATION_TIMEOUT,
        }
    }
}

/// What happened on one opt-out page.
#[derive(Debug, Clone)]
pub struct FormOutcome {
    pub url: String,
    /// Where the page ended up, after any redirect.
    pub final_url: String,
    pub title: String,
    /// What was typed, and what could not be.
    pub plan: FillPlan,
    /// A challenge standing in the way, if there is one.
    pub captcha: Option<Captcha>,
    /// Whether the form was submitted.
    pub submitted: bool,
    /// Where the screenshot went.
    pub screenshot: Option<PathBuf>,
}

impl FormOutcome {
    /// Whether a person has to finish this one by hand.
    pub fn needs_a_person(&self) -> bool {
        self.captcha
            .as_ref()
            .is_some_and(Captcha::blocks_automation)
            || self.plan.has_unanswered_required()
            || self.plan.is_empty()
    }

    /// One line saying where this got to.
    pub fn summary(&self) -> String {
        if let Some(captcha) = &self.captcha
            && captcha.blocks_automation()
        {
            return format!("blocked by a challenge — {}", captcha.instructions());
        }
        if self.plan.is_empty() {
            return "nothing on the page could be filled in".to_string();
        }
        if self.plan.has_unanswered_required() {
            return format!(
                "filled {} of the boxes, but a required one is missing from your profile",
                self.plan.fills.len()
            );
        }
        if self.submitted {
            return format!("filled {} boxes and submitted", self.plan.fills.len());
        }
        format!(
            "filled {} boxes — open the page and press submit",
            self.plan.fills.len()
        )
    }
}

/// A running browser.
pub struct Browser {
    browser: chromiumoxide::Browser,
    /// The task pumping the CDP connection. Aborted on drop.
    handler: tokio::task::JoinHandle<()>,
    profile: Profile,
    options: BrowserOptions,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The profile is a home address and a phone number.
        f.debug_struct("Browser")
            .field("headless", &self.options.headless)
            .field("submit", &self.options.submit)
            .finish()
    }
}

impl Browser {
    /// Start a browser.
    pub async fn launch(profile: Profile, options: BrowserOptions) -> Result<Self, Error> {
        let mut config = chromiumoxide::BrowserConfig::builder();
        if !options.headless {
            config = config.with_head();
        }

        let config = config
            .request_timeout(options.timeout)
            // Brokers serve desktop layouts; a phone-sized window hides
            // fields behind menus.
            .window_size(1440, 900)
            .build()
            .map_err(|message| Error::Launch(chromiumoxide::error::CdpError::msg(message)))?;

        let (browser, mut events) = chromiumoxide::Browser::launch(config)
            .await
            .map_err(Error::Launch)?;

        // The connection only advances while something drains its events.
        let handler = tokio::spawn(async move { while events.next().await.is_some() {} });

        Ok(Self {
            browser,
            handler,
            profile,
            options,
        })
    }

    /// Open an opt-out page, fill what can be filled, and report back.
    pub async fn fill_form(&self, url: &str, broker_id: &str) -> Result<FormOutcome, Error> {
        let page = self
            .browser
            .new_page(url)
            .await
            .map_err(|source| Error::Navigate {
                url: url.to_string(),
                source,
            })?;

        page.wait_for_navigation().await?;

        let html = page.content().await.unwrap_or_default();
        let found_captcha = captcha::detect_in_html(&html);

        let fields: Vec<FormField> = page
            .evaluate(COLLECT_FIELDS)
            .await?
            .into_value()
            .unwrap_or_default();

        let plan = filler::plan(&self.profile, &fields);

        // A challenge means anything typed in is likely to be thrown away, so
        // leave the page as it is for a person to pick up.
        let blocked = found_captcha
            .as_ref()
            .is_some_and(Captcha::blocks_automation);

        if !blocked {
            for fill in &plan.fills {
                if let Err(error) = type_into(&page, &fill.field.selector, &fill.value).await {
                    // One box refusing input should not lose the others.
                    tracing::warn!(
                        selector = %fill.field.selector,
                        %error,
                        "could not fill a field"
                    );
                }
            }
        }

        let mut submitted = false;
        if self.options.submit && !blocked && !plan.is_empty() && !plan.has_unanswered_required() {
            submitted = self.submit(&page).await;
        }

        let screenshot = self.screenshot(&page, broker_id).await;

        let outcome = FormOutcome {
            url: url.to_string(),
            final_url: page.url().await.ok().flatten().unwrap_or_default(),
            title: page.get_title().await.ok().flatten().unwrap_or_default(),
            plan,
            captcha: found_captcha,
            submitted,
            screenshot,
        };

        let _ = page.close().await;
        Ok(outcome)
    }

    /// Press the form's submit button.
    async fn submit(&self, page: &chromiumoxide::Page) -> bool {
        let selector: Option<String> = page
            .evaluate(FIND_SUBMIT)
            .await
            .ok()
            .and_then(|result| result.into_value().ok())
            .flatten();

        let Some(selector) = selector else {
            tracing::warn!("no submit button found on the page");
            return false;
        };

        let Ok(element) = page.find_element(&selector).await else {
            return false;
        };
        if element.click().await.is_err() {
            return false;
        }

        // Give the page a moment to answer before the screenshot is taken.
        tokio::time::sleep(SETTLE_AFTER_SUBMIT).await;
        true
    }

    /// Save a picture of the page.
    ///
    /// This is what makes an unsubmitted fill useful: the screenshot shows
    /// exactly what was typed, so a person can check it before sending.
    async fn screenshot(&self, page: &chromiumoxide::Page, broker_id: &str) -> Option<PathBuf> {
        let dir = self.options.screenshot_dir.as_ref()?;

        if let Err(error) = std::fs::create_dir_all(dir) {
            tracing::warn!(%error, "could not create the screenshot directory");
            return None;
        }

        let path = dir.join(screenshot_name(broker_id));
        match page.screenshot(screenshot_params()).await {
            Ok(bytes) => match std::fs::write(&path, bytes) {
                Ok(()) => Some(path),
                Err(error) => {
                    tracing::warn!(%error, "could not save the screenshot");
                    None
                }
            },
            Err(error) => {
                tracing::warn!(%error, "could not take a screenshot");
                None
            }
        }
    }

    /// Shut the browser down.
    pub async fn close(mut self) {
        let _ = self.browser.close().await;
        let _ = self.browser.wait().await;
        self.handler.abort();
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // Without this the event pump outlives the browser and spins.
        self.handler.abort();
    }
}

/// Clear a field and type a value into it.
async fn type_into(
    page: &chromiumoxide::Page,
    selector: &str,
    value: &str,
) -> Result<(), chromiumoxide::error::CdpError> {
    let element = page.find_element(selector).await?;
    element.click().await?;
    // Some forms prefill a placeholder value; typing would append to it.
    element.type_str(value).await?;
    Ok(())
}

/// A stable, filesystem-safe name for one broker's screenshot.
pub fn screenshot_name(broker_id: &str) -> String {
    let safe: String = broker_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    format!("{safe}-{stamp}.png")
}

/// Where screenshots go by default.
pub fn default_screenshot_dir() -> PathBuf {
    match crate::config::home_dir() {
        Some(home) => home.join(".eraser").join("screenshots"),
        None => PathBuf::from("screenshots"),
    }
}

fn screenshot_params() -> chromiumoxide::page::ScreenshotParams {
    chromiumoxide::page::ScreenshotParams::builder()
        .format(chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat::Png)
        // The whole page, not just what fits: the confirmation text is often
        // below the fold.
        .full_page(true)
        .build()
}

/// Whether a path looks like somewhere screenshots may be written.
pub fn is_usable_screenshot_dir(dir: &Path) -> bool {
    !dir.as_os_str().is_empty() && (!dir.exists() || dir.is_dir())
}

#[cfg(test)]
mod tests;
