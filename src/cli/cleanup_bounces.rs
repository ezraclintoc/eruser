//! `eruser cleanup-bounces` — retire broker addresses that no longer accept mail.
//!
//! Ported from `runCleanupBounces` in `cmd/eraser/main.go`. Across 764
//! community-maintained addresses some are always dead; every send to one
//! wastes a slot against the daily limit and comes back as a bounce.

use super::{Error, Paths};
use crate::broker::{Broker, BrokerDatabase};
use crate::history::{DEFAULT_USER_ID, Store};
use crate::inbox::{Monitor, parser};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Actually remove them. Without this, nothing is changed.
    #[arg(long)]
    pub remove: bool,

    /// How many days of mail to look through
    #[arg(long, default_value_t = 30)]
    pub days: i64,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            remove: false,
            days: 30,
        }
    }
}

/// One dead address, and what said so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounced {
    pub address: String,
    pub broker: Broker,
    pub subject: String,
    pub received_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A bounce that could not be acted on, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmatched {
    /// The bounce did not name which address failed.
    NoAddress { subject: String },
    /// It named one, but no broker here uses it.
    UnknownBroker { address: String },
}

pub async fn run(paths: &Paths, args: Args) -> Result<(), Error> {
    let store = Store::open(Store::default_path()).await?;
    let config = paths.settings_for(&store, DEFAULT_USER_ID).await?;
    store.close().await;

    config.validate_inbox()?;

    let broker_path = paths.broker_path();
    let mut brokers = paths.load_brokers()?;

    println!("Looking through the last {} days for bounces…", args.days);
    println!();

    let mut monitor = Monitor::new(config.inbox.clone(), &brokers.brokers);
    monitor.connect().await?;
    let emails = monitor.bounce_emails(args.days).await;
    monitor.disconnect().await;
    let emails = emails?;

    let (bounced, unmatched) = sort_bounces(&emails, &brokers);
    print!("{}", format_report(&bounced, &unmatched, args.remove));

    if !args.remove || bounced.is_empty() {
        return Ok(());
    }

    // Writing to the database only makes sense against a file. A binary
    // running on its embedded copy has nowhere to save the change.
    let Some(path) = broker_path else {
        return Err(Error::NoBrokerFile);
    };

    let mut removed = 0;
    for entry in &bounced {
        if brokers.remove_by_email(&entry.address).is_some() {
            removed += 1;
        }
    }

    brokers.save_with_backup(&path)?;

    println!();
    println!("Removed {removed} of them from {}.", path.display());
    println!("The previous file is beside it as {}.bak.", path.display());

    Ok(())
}

/// Work out which brokers the bounces are about.
///
/// Pure, so the matching is testable without a mailbox.
pub(super) fn sort_bounces(
    emails: &[crate::inbox::Email],
    brokers: &BrokerDatabase,
) -> (Vec<Bounced>, Vec<Unmatched>) {
    let mut bounced: Vec<Bounced> = Vec::new();
    let mut unmatched = Vec::new();

    for email in emails {
        let Some(address) = parser::extract_bounced_recipient(email) else {
            unmatched.push(Unmatched::NoAddress {
                subject: email.subject.clone(),
            });
            continue;
        };

        let Some(broker) = brokers.find_by_email(&address) else {
            unmatched.push(Unmatched::UnknownBroker { address });
            continue;
        };

        // One address can bounce many times; it is still one broker to
        // remove, and listing it repeatedly would overstate the damage.
        if bounced
            .iter()
            .any(|seen| seen.address.eq_ignore_ascii_case(&address))
        {
            continue;
        }

        bounced.push(Bounced {
            address,
            broker: broker.clone(),
            subject: email.subject.clone(),
            received_at: email.received_at,
        });
    }

    (bounced, unmatched)
}

/// Render the report. Pure, so the wording is testable.
pub(super) fn format_report(
    bounced: &[Bounced],
    unmatched: &[Unmatched],
    removing: bool,
) -> String {
    let mut out = String::new();

    for entry in bounced {
        let date = entry
            .received_at
            .map(|at| at.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "an unknown date".to_string());

        out.push_str(&format!(
            "{} — {} ({})\n  bounced on {date}: {}\n\n",
            entry.address,
            entry.broker.name,
            entry.broker.id,
            truncate(&entry.subject, 60),
        ));
    }

    for entry in unmatched {
        match entry {
            Unmatched::NoAddress { subject } => out.push_str(&format!(
                "A bounce that does not say which address failed: {}\n",
                truncate(subject, 60)
            )),
            Unmatched::UnknownBroker { address } => out.push_str(&format!(
                "{address} bounced, but no broker here uses that address.\n"
            )),
        }
    }

    if !unmatched.is_empty() {
        out.push('\n');
    }

    if bounced.is_empty() {
        out.push_str("No broker addresses look dead.\n");
        return out;
    }

    let count = bounced.len();
    let plural = if count == 1 { "" } else { "es" };
    if removing {
        out.push_str(&format!("Removing {count} address{plural}.\n"));
    } else {
        out.push_str(&format!(
            "{count} address{plural} look dead. Nothing has been changed — run this \
             again with --remove to take them out of the broker database.\n"
        ));
    }

    out
}

/// Shorten a subject line for a one-line summary.
fn truncate(text: &str, limit: usize) -> String {
    // Counted in characters, not bytes: Go's slice would split a multi-byte
    // character and produce mojibake.
    if text.chars().count() <= limit {
        return text.to_string();
    }

    let kept: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests;
