//! Email template rendering.
//!
//! Ported from `internal/template/template.go`. The three request templates
//! are embedded in the binary, converted from Go `text/template` syntax to
//! Jinja (minijinja), which is close enough that the wording is unchanged.

use std::collections::BTreeMap;

use chrono::{Datelike, Local};
use serde::Serialize;

use crate::broker::Broker;
use crate::config::Profile;

mod error;
pub use error::Error;

/// Template sources, embedded at compile time so the binary is self-contained.
const SOURCES: [(&str, &str); 3] = [
    ("gdpr", include_str!("../../templates/email/gdpr.txt")),
    ("ccpa", include_str!("../../templates/email/ccpa.txt")),
    ("generic", include_str!("../../templates/email/generic.txt")),
];

/// The default when the config does not name one.
pub const DEFAULT_TEMPLATE: &str = "generic";

/// Everything a request template can reference.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct EmailData {
    pub first_name: String,
    pub last_name: String,
    pub full_name: String,
    pub email: String,
    pub address: String,
    pub city: String,
    pub state: String,
    pub zip_code: String,
    pub country: String,
    pub phone: String,
    pub date_of_birth: String,

    pub broker_name: String,
    pub broker_email: String,
    pub broker_website: String,
    pub broker_opt_out: String,

    pub date: String,
    pub year: i32,
    pub month: String,
    pub template: String,
}

impl EmailData {
    fn new(profile: &Profile, broker: &Broker, template: &str) -> Self {
        let now = Local::now();
        Self {
            first_name: profile.first_name.clone(),
            last_name: profile.last_name.clone(),
            full_name: profile.full_name(),
            email: profile.email.clone(),
            address: profile.address.clone(),
            city: profile.city.clone(),
            state: profile.state.clone(),
            zip_code: profile.zip_code.clone(),
            country: profile.country.clone(),
            phone: profile.phone.clone(),
            date_of_birth: profile.date_of_birth.clone(),

            broker_name: broker.name.clone(),
            broker_email: broker.email.clone(),
            broker_website: broker.website.clone(),
            broker_opt_out: broker.opt_out_url.clone(),

            // "January 2, 2006" in Go's reference-time notation.
            date: now.format("%B %-d, %Y").to_string(),
            year: now.year(),
            month: now.format("%B").to_string(),
            template: template.to_string(),
        }
    }
}

/// A rendered email, ready to hand to a sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub subject: String,
    pub body: String,
}

/// Holds the parsed request templates.
pub struct Engine {
    env: minijinja::Environment<'static>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("templates", &self.available_templates())
            .finish()
    }
}

impl Engine {
    /// Parse the embedded templates. Fails only on a malformed template,
    /// which a test in this module catches before it can ship.
    pub fn new() -> Result<Self, Error> {
        let mut env = minijinja::Environment::new();
        // These are plain-text emails; HTML escaping would mangle
        // apostrophes and ampersands in names and addresses.
        env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
        // An unset placeholder means a template references a field that no
        // longer exists — surface it rather than silently sending a blank.
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);

        for (name, source) in SOURCES {
            env.add_template(name, source)
                .map_err(|source| Error::Parse {
                    template: name.to_string(),
                    source,
                })?;
        }
        Ok(Self { env })
    }

    /// Render `template_name` for one broker.
    pub fn render(
        &self,
        template_name: &str,
        profile: &Profile,
        broker: &Broker,
    ) -> Result<Email, Error> {
        let template = self
            .env
            .get_template(template_name)
            .map_err(|_| Error::Unknown(template_name.to_string()))?;

        let data = EmailData::new(profile, broker, template_name);
        let body = template.render(&data).map_err(|source| Error::Render {
            template: template_name.to_string(),
            source,
        })?;

        Ok(Email {
            subject: subject_for(template_name).to_string(),
            body,
        })
    }

    pub fn has_template(&self, name: &str) -> bool {
        self.env.get_template(name).is_ok()
    }

    /// Available template names, sorted, so menus and `--help` are stable.
    pub fn available_templates(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .env
            .templates()
            .map(|(name, _)| name.to_string())
            .collect();
        names.sort();
        names
    }

    /// One-line descriptions, for the CLI and the setup wizard.
    pub fn descriptions() -> BTreeMap<&'static str, &'static str> {
        BTreeMap::from([
            ("gdpr", "Invokes the EU GDPR Article 17 right to erasure"),
            (
                "ccpa",
                "Invokes California Consumer Privacy Act deletion rights",
            ),
            (
                "generic",
                "References several privacy laws; a reasonable default anywhere",
            ),
        ])
    }
}

/// Subject lines are fixed per template rather than templated, because
/// brokers route on them and an injected newline in a subject is a header
/// injection vector.
fn subject_for(template_name: &str) -> &'static str {
    match template_name {
        "gdpr" => "GDPR Data Erasure Request - Article 17 Right to Erasure",
        "ccpa" => "CCPA Data Deletion Request - Right to Delete Personal Information",
        _ => "Personal Data Removal Request",
    }
}

#[cfg(test)]
mod tests;
