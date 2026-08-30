//! What each email provider needs to be talked to.
//!
//! Go hardcoded Gmail in three places — the init prompt, the setup wizard,
//! and the config defaults — so any other provider meant editing the config
//! file by hand. The table lives here instead, and the places that used to
//! assume Gmail ask it.

pub const GMAIL_SMTP_HOST: &str = "smtp.gmail.com";

/// Implicit TLS. Every provider below accepts it, and unlike STARTTLS on 587
/// there is no plaintext moment to strip.
pub const DEFAULT_SMTP_PORT: u16 = 465;

/// The SMTP server for a well-known address, so the common case needs no
/// setting.
///
/// A domain nobody here recognises gets no guess: inventing a hostname would
/// only turn a clear question into a connection failure later.
pub fn smtp_host_for(address: &str) -> Option<&'static str> {
    let domain = address.rsplit_once('@')?.1.trim().to_lowercase();

    match domain.as_str() {
        "gmail.com" | "googlemail.com" => Some(GMAIL_SMTP_HOST),
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => Some("smtp-mail.outlook.com"),
        "yahoo.com" | "ymail.com" => Some("smtp.mail.yahoo.com"),
        "icloud.com" | "me.com" | "mac.com" => Some("smtp.mail.me.com"),
        "proton.me" | "protonmail.com" | "pm.me" => Some("smtp.protonmail.ch"),
        "fastmail.com" | "fastmail.fm" => Some("smtp.fastmail.com"),
        "zoho.com" => Some("smtp.zoho.com"),
        "aol.com" => Some("smtp.aol.com"),
        _ => None,
    }
}

/// Whether an address belongs to a provider that needs an app password
/// rather than the account's own password.
///
/// Worth saying out loud in the interface: typing the normal password is the
/// single most common reason a first send fails.
pub fn needs_app_password(address: &str) -> bool {
    matches!(
        smtp_host_for(address),
        Some(GMAIL_SMTP_HOST | "smtp.mail.yahoo.com" | "smtp.mail.me.com")
    )
}

#[cfg(test)]
mod tests;
