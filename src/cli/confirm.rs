//! `eruser confirm` — follow the confirmation links brokers sent.

use super::{Error, Paths};
use crate::automation::confirm::{Confirmation, Confirmer, Outcome};
use crate::history::{DEFAULT_USER_ID, PipelineStatus, ResponseFilter, ResponseType, Store};

#[derive(Debug, Default, clap::Args)]
pub struct Args {
    /// A single link to follow, instead of the ones already found
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,

    /// Only this broker's link
    #[arg(long, value_name = "ID")]
    pub broker: Option<String>,

    /// Show what would be followed, without following anything
    #[arg(long)]
    pub dry_run: bool,

    /// Follow links to domains that belong to no known broker
    ///
    /// These URLs come out of email. Leave this off unless you have looked
    /// at the link yourself.
    #[arg(long)]
    pub no_validate_domain: bool,
}

pub async fn run(paths: &Paths, args: Args) -> Result<(), Error> {
    let brokers = paths.load_brokers()?;
    let confirmer = Confirmer::new(&brokers.brokers)?;

    // A URL given on the command line is followed on its own; nothing is
    // read from or written to history for it.
    if let Some(url) = &args.url {
        if args.dry_run {
            println!("Would follow {url}");
            return Ok(());
        }

        let result = confirmer.confirm(url, !args.no_validate_domain).await?;
        println!("{}", format_one("", &result));
        return Ok(());
    }

    let store = Store::open(Store::default_path()).await?;
    let pending = pending_links(&store, args.broker.as_deref()).await?;

    if pending.is_empty() {
        store.close().await;
        println!("{}", NOTHING_TO_CONFIRM);
        return Ok(());
    }

    if args.dry_run {
        store.close().await;
        print!("{}", format_dry_run(&pending));
        return Ok(());
    }

    println!("Following {} confirmation links…", pending.len());
    println!();

    let mut confirmed = 0usize;
    let mut needs_a_person = 0usize;
    let mut failed = 0usize;

    for (broker_id, broker_name, url) in &pending {
        let result = confirmer.confirm(url, !args.no_validate_domain).await;

        let outcome = match result {
            Ok(confirmation) => {
                print!("{}", format_one(broker_name, &confirmation));
                confirmation.outcome
            }
            Err(error) => {
                println!("  {broker_name}: {error}");
                Outcome::Failed(error.to_string())
            }
        };

        if outcome.is_success() {
            confirmed += 1;
        } else if outcome.needs_a_person() {
            needs_a_person += 1;
        } else {
            failed += 1;
        }

        if let Some(stage) = stage_for(&outcome) {
            store
                .update_pipeline_status(DEFAULT_USER_ID, broker_id, stage)
                .await?;
        }
    }

    store.close().await;
    print!("{}", format_summary(confirmed, needs_a_person, failed));
    Ok(())
}

const NOTHING_TO_CONFIRM: &str = "No confirmation links are waiting.\n\n\
     Run `eruser monitor` first — links are found by reading the replies \
     brokers send.";

/// The confirmation links found in stored replies, still to be followed.
async fn pending_links(
    store: &Store,
    only_broker: Option<&str>,
) -> Result<Vec<(String, String, String)>, Error> {
    let responses = store
        .broker_responses(
            DEFAULT_USER_ID,
            ResponseFilter {
                response_type: Some(ResponseType::ConfirmationRequired),
                limit: Some(1000),
                ..Default::default()
            },
        )
        .await?;

    Ok(responses
        .into_iter()
        .filter(|response| !response.confirm_url.is_empty())
        .filter(|response| {
            only_broker.is_none_or(|wanted| response.broker_id.eq_ignore_ascii_case(wanted))
        })
        .map(|response| {
            (
                response.broker_id,
                response.broker_name,
                response.confirm_url,
            )
        })
        .collect())
}

/// Which pipeline stage an outcome moves the broker to.
///
/// A failure moves nothing: the link can be tried again later, and marking
/// the broker as failed would hide it from the next run.
pub fn stage_for(outcome: &Outcome) -> Option<PipelineStatus> {
    match outcome {
        Outcome::Confirmed | Outcome::AlreadyConfirmed => Some(PipelineStatus::Confirmed),
        Outcome::Expired | Outcome::Invalid => Some(PipelineStatus::Failed),
        // Still waiting on a person, which is where it already was.
        Outcome::Blocked(_) | Outcome::Unclear => None,
        Outcome::Failed(_) => None,
    }
}

/// One line per link followed.
fn format_one(broker_name: &str, result: &Confirmation) -> String {
    let marker = if result.outcome.is_success() {
        "ok  "
    } else if result.outcome.needs_a_person() {
        "look"
    } else {
        "FAIL"
    };

    let who = if broker_name.is_empty() {
        result.url.clone()
    } else {
        broker_name.to_string()
    };

    let mut line = format!("{marker}  {who}: {}\n", result.outcome.summary());

    // Where a person has to go and finish it, the link is worth repeating.
    if result.outcome.needs_a_person() && !broker_name.is_empty() {
        line.push_str(&format!("      {}\n", result.final_url));
    }

    line
}

fn format_dry_run(pending: &[(String, String, String)]) -> String {
    use std::fmt::Write;

    let mut out = format!("Would follow {} confirmation links:\n\n", pending.len());
    for (_, broker_name, url) in pending {
        let _ = writeln!(out, "  {broker_name}\n    {url}");
    }
    out
}

fn format_summary(confirmed: usize, needs_a_person: usize, failed: usize) -> String {
    use std::fmt::Write;

    let mut out = String::from("\n");
    let _ = writeln!(out, "{}", "-".repeat(40));
    let _ = write!(out, "{confirmed} confirmed");
    if needs_a_person > 0 {
        let _ = write!(out, ", {needs_a_person} need a look");
    }
    if failed > 0 {
        let _ = write!(out, ", {failed} failed");
    }
    let _ = writeln!(out, ".");

    if needs_a_person > 0 {
        let _ = writeln!(
            out,
            "The ones marked \"look\" are behind a challenge or said nothing useful."
        );
    }

    out
}

#[cfg(test)]
mod tests;
