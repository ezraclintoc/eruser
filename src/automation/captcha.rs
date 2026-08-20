//! Spotting a CAPTCHA.
//!
//! Ported from `internal/browser/captcha.go`. Only the HTML-based detection
//! is here; the live-page detectors belong with the browser driver.
//!
//! Detecting one is not solving one. What this buys is an honest answer: when
//! a broker's opt-out page is behind a challenge, eruser can say so and hand
//! the page to a person instead of silently failing.

use serde::{Deserialize, Serialize};

/// Which challenge is in the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptchaKind {
    RecaptchaV2,
    /// Invisible; usually scores the visitor without asking anything.
    RecaptchaV3,
    HCaptcha,
    Turnstile,
    FunCaptcha,
    ImageCaptcha,
    TextCaptcha,
    CloudflareChallenge,
    Unknown,
}

impl CaptchaKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RecaptchaV2 => "recaptcha_v2",
            Self::RecaptchaV3 => "recaptcha_v3",
            Self::HCaptcha => "hcaptcha",
            Self::Turnstile => "cloudflare_turnstile",
            Self::FunCaptcha => "funcaptcha",
            Self::ImageCaptcha => "image_captcha",
            Self::TextCaptcha => "text_captcha",
            Self::CloudflareChallenge => "cloudflare_challenge",
            Self::Unknown => "unknown",
        }
    }

    /// What the person actually has to do about it.
    pub const fn instructions(self) -> &'static str {
        match self {
            Self::RecaptchaV2 => "Google reCAPTCHA — tick the box, and solve the pictures if asked",
            Self::RecaptchaV3 => "Google reCAPTCHA v3 — invisible; it often passes on its own",
            Self::HCaptcha => "hCaptcha — pick the images that match the description",
            Self::Turnstile => "Cloudflare Turnstile — usually passes by itself after a moment",
            Self::FunCaptcha => "FunCaptcha — an interactive puzzle to work through",
            Self::ImageCaptcha => "Image CAPTCHA — type the characters in the picture",
            Self::TextCaptcha => "Text CAPTCHA — enter the verification code",
            Self::CloudflareChallenge => "Cloudflare check — wait for it, or complete the prompt",
            Self::Unknown => {
                "Something is asking you to prove you are human — open the page and look"
            }
        }
    }

    /// Whether this actually stops an automated fill.
    ///
    /// reCAPTCHA v3 scores the visitor in the background rather than asking
    /// anything, so a page carrying one is often still fillable.
    pub const fn blocks_automation(self) -> bool {
        !matches!(self, Self::RecaptchaV3)
    }
}

impl std::fmt::Display for CaptchaKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What was found on a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Captcha {
    pub kind: CaptchaKind,
    /// 0.0 to 1.0.
    pub confidence: f64,
    /// The text that gave it away, for anyone debugging a false positive.
    pub matched: String,
}

impl Captcha {
    /// Whether this finding should stop an automated fill.
    pub fn blocks_automation(&self) -> bool {
        self.kind.blocks_automation()
    }

    pub fn instructions(&self) -> &'static str {
        self.kind.instructions()
    }
}

/// Markers that name a specific challenge. Order matters: the specific ones
/// are checked before the generic keyword sweep.
const SIGNATURES: &[(CaptchaKind, &[&str])] = &[
    (
        CaptchaKind::HCaptcha,
        &["hcaptcha", "h-captcha", "hcaptcha.com"],
    ),
    (
        CaptchaKind::Turnstile,
        &["cf-turnstile", "challenges.cloudflare.com", "turnstile"],
    ),
    (
        CaptchaKind::FunCaptcha,
        &["funcaptcha", "arkoselabs", "arkose-labs"],
    ),
    (
        CaptchaKind::CloudflareChallenge,
        &["cf-challenge", "cf_chl_", "checking your browser", "ray id"],
    ),
];

/// Generic wording, which says something is there but not what.
const GENERIC_MARKERS: &[&str] = &[
    "captcha",
    "verification code",
    "security code",
    "prove you are human",
    "prove you're human",
    "i am not a robot",
    "i'm not a robot",
];

/// Confidence when a named provider's own markup is present.
const NAMED: f64 = 0.85;
/// Confidence when only generic wording matched.
const GENERIC: f64 = 0.60;

/// Look for a challenge in a page's HTML.
///
/// Go checked reCAPTCHA first and reported every hit as v2, so a page using
/// the invisible v3 was reported as a blocking challenge and sent to a person
/// for nothing. The two are told apart here.
pub fn detect_in_html(html: &str) -> Option<Captcha> {
    let html = html.to_lowercase();

    // reCAPTCHA first, because v2 and v3 share a name and have to be
    // separated before anything else claims them.
    if html.contains("recaptcha") || html.contains("g-recaptcha") {
        let is_v3 = html.contains("recaptcha/api.js?render=")
            || html.contains("grecaptcha.execute")
            || html.contains("recaptcha_v3")
            // v2 renders a visible widget; v3 never does.
            || (html.contains("recaptcha") && !html.contains("g-recaptcha"));

        return Some(Captcha {
            kind: if is_v3 {
                CaptchaKind::RecaptchaV3
            } else {
                CaptchaKind::RecaptchaV2
            },
            confidence: NAMED,
            matched: "recaptcha".to_string(),
        });
    }

    for (kind, markers) in SIGNATURES {
        if let Some(marker) = markers.iter().find(|marker| html.contains(**marker)) {
            return Some(Captcha {
                kind: *kind,
                confidence: NAMED,
                matched: (*marker).to_string(),
            });
        }
    }

    // An <img> whose source or name says captcha is the old-fashioned kind.
    if html.contains("captcha") && (html.contains("<img") || html.contains("captcha_image")) {
        return Some(Captcha {
            kind: CaptchaKind::ImageCaptcha,
            confidence: GENERIC,
            matched: "captcha image".to_string(),
        });
    }

    GENERIC_MARKERS
        .iter()
        .find(|marker| html.contains(**marker))
        .map(|marker| Captcha {
            kind: CaptchaKind::Unknown,
            confidence: GENERIC,
            matched: (*marker).to_string(),
        })
}

#[cfg(test)]
mod tests;
