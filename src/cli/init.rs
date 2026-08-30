//! `eruser init` — write a config file, answering a few questions.

use super::{Error, Paths, prompt};
use crate::config::{Config, SmtpConfig};

/// Gmail's submission endpoint, the overwhelmingly common case.
const GMAIL_SMTP_HOST: &str = "smtp.gmail.com";
const GMAIL_SMTP_PORT: u16 = 465;

pub fn run(paths: &Paths) -> Result<(), Error> {
    let path = paths.config_path();

    if path.exists() {
        // Overwriting silently would throw away a working setup, including a
        // password the user may no longer have a copy of.
        let answer = prompt::line(&format!(
            "A config already exists at {}.\nOverwrite it? [y/N]: ",
            path.display()
        ))?;
        if !matches!(answer.to_lowercase().as_str(), "y" | "yes") {
            println!("Left the existing config alone.");
            return Err(Error::Cancelled);
        }
        println!();
    }

    println!("eruser setup");
    println!("============");
    println!();
    println!("Your details go into the removal requests — brokers need them to");
    println!("find your records. Everything stays on this machine.");
    println!();

    let mut config = Config::default();

    config.profile.first_name = prompt::line("First name: ")?;
    config.profile.last_name = prompt::line("Last name: ")?;
    config.profile.email = prompt::line("Email address: ")?;

    println!();
    println!("The rest is optional, but each field you fill in makes it easier");
    println!("for a broker to match your record — and harder for them to claim");
    println!("they could not find you.");
    println!();

    config.profile.address = prompt::line("Street address: ")?;
    config.profile.city = prompt::line("City: ")?;
    config.profile.state = prompt::line("State or province: ")?;
    config.profile.zip_code = prompt::line("ZIP or postal code: ")?;
    config.profile.country = prompt::line("Country: ")?;
    config.profile.phone = prompt::line("Phone number: ")?;
    config.profile.date_of_birth = prompt::line("Date of birth (YYYY-MM-DD): ")?;

    println!();
    println!("Sending email");
    println!("-------------");
    println!();
    println!("Two ways to do this:");
    println!();
    println!("  1. Your own mailbox over SMTP. Free, and the requests come from");
    println!("     your real address — but Gmail needs two-factor authentication");
    println!("     turned on and an app password generated.");
    println!("  2. A sending service — Resend or SendGrid. One API key, nothing");
    println!("     else to set up. Both have free tiers large enough for a full");
    println!("     run, and both need you to verify the address you send from.");
    println!();

    let choice = prompt::line_or("Which? [1]: ", "1")?;

    config.email.from = prompt::line_or(
        &format!("Address to send from [{}]: ", config.profile.email),
        &config.profile.email,
    )?;

    match choice.trim() {
        "2" | "resend" | "sendgrid" => {
            let provider = prompt::line_or("resend or sendgrid [resend]: ", "resend")?;
            let provider = provider.trim().to_lowercase();

            let signup = match provider.as_str() {
                "sendgrid" => "https://app.sendgrid.com/settings/api_keys",
                _ => "https://resend.com/api-keys",
            };
            println!();
            println!("Create a key at {signup}");
            println!();

            let key = prompt::secret("API key")?;
            if provider == "sendgrid" {
                config.email.provider = "sendgrid".to_string();
                config.email.sendgrid.api_key = key;
            } else {
                config.email.provider = "resend".to_string();
                config.email.resend.api_key = key;
            }
        }
        _ => {
            println!();
            println!("For Gmail you need an app password, not your normal one:");
            println!("  https://myaccount.google.com/apppasswords");
            println!();

            let username = prompt::line_or(
                &format!("Mailbox to sign in as [{}]: ", config.email.from),
                &config.email.from,
            )?;
            let password = prompt::secret("App password")?;
            let host =
                prompt::line_or(&format!("SMTP host [{GMAIL_SMTP_HOST}]: "), GMAIL_SMTP_HOST)?;
            let port = prompt::line_or(
                &format!("SMTP port [{GMAIL_SMTP_PORT}]: "),
                &GMAIL_SMTP_PORT.to_string(),
            )?
            .parse()
            .unwrap_or(GMAIL_SMTP_PORT);

            config.email.provider = "smtp".to_string();
            config.email.smtp = SmtpConfig {
                host,
                port,
                username,
                password,
                use_tls: true,
            };
        }
    }

    println!();
    let template = prompt::line_or(
        "Request template — gdpr, ccpa, or generic [generic]: ",
        crate::template::DEFAULT_TEMPLATE,
    )?;
    config.options.template = template;
    config.apply_defaults();

    config.save(&path)?;

    println!();
    println!("Saved to {}", path.display());
    if let Err(problem) = config.validate() {
        println!();
        println!("Heads up: {problem}");
        println!("Edit the file before sending, or run `eruser init` again.");
        return Ok(());
    }

    println!();
    println!("Next:");
    println!("  eruser send --dry-run   see what would go out, without sending");
    println!("  eruser send             send the requests");
    println!("  eruser serve            do all of this in a browser instead");

    Ok(())
}
