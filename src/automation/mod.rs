//! Acting on broker websites.
//!
//! Ported from `internal/browser/`. Named for what it does rather than how:
//! following a confirmation link is plain HTTP, and only form filling needs
//! a real browser.

pub mod captcha;
pub mod confirm;

pub use captcha::{Captcha, CaptchaKind};
pub use confirm::{Confirmation, Confirmer, Outcome};
