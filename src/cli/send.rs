//! `eruser send` — send removal requests to the broker database.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::{Error, Paths};
use crate::history::{DEFAULT_USER_ID, Store};
use crate::send::{Outcome, Progress, SendJob, SendOptions, Summary, sender_for};
use crate::template::Engine;

#[derive(Debug, Default, clap::Args)]
pub struct Args {
    /// Show what would be sent, without sending or recording anything
    #[arg(long)]
    pub dry_run: bool,

    /// Override the template: gdpr, ccpa, or generic
    #[arg(long)]
    pub template: Option<String>,

    /// Only brokers in these regions
    #[arg(long, value_delimiter = ',')]
    pub region: Vec<String>,

    /// Only these brokers, by id
    // The id must differ from the global --brokers flag, which clap keys by
    // field name and would otherwise collide with this one.
    #[arg(long = "broker", id = "broker_id", value_name = "ID")]
    pub brokers: Vec<String>,

    /// Stop after this many sends, to stay under a provider's daily limit
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,

    /// Milliseconds to wait between sends
    #[arg(long, value_name = "MS")]
    pub rate_limit_ms: Option<u64>,
}

pub async fn run(paths: &Paths, args: Args) -> Result<(), Error> {
    let config = paths.load_config()?;
    config.validate()?;

    let db = paths.load_brokers()?;
    let template = args
        .template
        .clone()
        .unwrap_or_else(|| config.options.template.clone());

    let engine = Engine::new()?;
    if !engine.has_template(&template) {
        return Err(crate::template::Error::Unknown(template).into());
    }

    // --region overrides the config; an explicit flag should win over a file.
    let regions = if args.region.is_empty() {
        config.options.regions.clone()
    } else {
        args.region.clone()
    };

    let mut brokers: Vec<_> = db
        .filter(&regions, &config.options.excluded_brokers)
        .into_iter()
        .cloned()
        .collect();

    if !args.brokers.is_empty() {
        let wanted: std::collections::HashSet<String> =
            args.brokers.iter().map(|id| id.to_lowercase()).collect();
        // Report ids that match nothing rather than quietly sending fewer
        // requests than the user asked for.
        let found: std::collections::HashSet<String> =
            brokers.iter().map(|b| b.id.to_lowercase()).collect();
        for id in wanted.difference(&found) {
            eprintln!("warning: no broker with id {id:?}");
        }
        brokers.retain(|b| wanted.contains(&b.id.to_lowercase()));
    }

    if brokers.is_empty() {
        println!("No brokers matched. Nothing to send.");
        return Ok(());
    }

    let store = if args.dry_run {
        None
    } else {
        Some(Store::open(Store::default_path()).await?)
    };

    let options = SendOptions {
        template,
        from: config.email.from.clone(),
        rate_limit: Duration::from_millis(
            args.rate_limit_ms.unwrap_or(config.options.rate_limit_ms),
        ),
        daily_limit: args.limit,
        user_id: DEFAULT_USER_ID,
    };

    let job = SendJob {
        brokers,
        profile: config.profile.clone(),
        engine: Arc::new(engine),
        sender: sender_for(&config.email, args.dry_run)?,
        store: store.clone(),
        options,
    };

    // Ctrl-C stops after the request in flight rather than killing the
    // process mid-send, so history stays accurate.
    let cancel = CancellationToken::new();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\nStopping after the current request…");
            signal_cancel.cancel();
        }
    });

    if args.dry_run {
        println!("Dry run — nothing will be sent.");
        println!();
    }

    let summary = job
        .run(&cancel, |event| {
            print!("{}", format_progress(&event, args.dry_run))
        })
        .await;

    if let Some(store) = store {
        store.close().await;
    }

    // Some failures are normal across 764 community-maintained addresses, so
    // a partial failure still exits zero and stays usable from a script.
    // Every request failing means something is wrong with the setup itself.
    if summary.sent == 0 && summary.failed > 0 {
        return Err(Error::AllSendsFailed {
            count: summary.failed,
        });
    }
    Ok(())
}

/// Render one progress event.
///
/// Separated from the run loop so the wording is testable.
pub(super) fn format_progress(event: &Progress, dry_run: bool) -> String {
    match event {
        Progress::Started { total } => format!("Sending to {total} brokers\n\n"),

        Progress::Broker {
            index,
            total,
            broker_name,
            broker_email,
            outcome,
            ..
        } => {
            let width = total.to_string().len();
            let counter = format!("[{index:>width$}/{total}]");
            match outcome {
                Outcome::Sent { .. } if dry_run => {
                    format!("{counter} would send to {broker_name} <{broker_email}>\n")
                }
                Outcome::Sent { .. } => format!("{counter} sent to {broker_name}\n"),
                Outcome::Failed { error } => {
                    format!("{counter} FAILED {broker_name}: {error}\n")
                }
                Outcome::SkippedOverLimit => {
                    format!("{counter} skipped {broker_name} — daily limit reached\n")
                }
            }
        }

        Progress::Finished(summary) => format_summary(summary, dry_run),
    }
}

pub(super) fn format_summary(summary: &Summary, dry_run: bool) -> String {
    use std::fmt::Write;

    let mut out = String::from("\n");
    let _ = writeln!(out, "{}", "-".repeat(40));

    if dry_run {
        let _ = writeln!(out, "Dry run: {} brokers would be contacted.", summary.sent);
    } else {
        let _ = write!(out, "{} sent", summary.sent);
        if summary.failed > 0 {
            let _ = write!(out, ", {} failed", summary.failed);
        }
        if summary.skipped > 0 {
            let _ = write!(out, ", {} skipped", summary.skipped);
        }
        let _ = writeln!(out, ".");
    }

    if summary.cancelled {
        let _ = writeln!(out, "Stopped early. Run `eruser send` again to continue.");
    }
    if summary.failed > 0 && !dry_run {
        let _ = writeln!(out, "See `eruser status` for what went wrong.");
    }

    out
}
