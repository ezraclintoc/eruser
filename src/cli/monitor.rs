//! `eruser monitor` — read the mailbox and sort what brokers sent back.

use super::{Error, Paths};
use crate::history::Store;
use crate::inbox::classifier::ResponseType;
use crate::inbox::scan::{self, Progress, ScanOptions, ScanSummary};
use crate::inbox::{Monitor, monitor};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// How many days of mail to read
    #[arg(long, default_value_t = scan::DEFAULT_DAYS)]
    pub days: i64,

    /// Also read mail that matched no known broker
    #[arg(long)]
    pub include_unmatched: bool,

    /// Re-read stored replies with the current patterns instead of
    /// fetching new mail
    #[arg(long, conflicts_with_all = ["days", "include_unmatched"])]
    pub reclassify: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            days: scan::DEFAULT_DAYS,
            include_unmatched: false,
            reclassify: false,
        }
    }
}

pub async fn run(paths: &Paths, args: Args) -> Result<(), Error> {
    let config = paths.load_config()?;
    let store = Store::open(Store::default_path()).await?;

    if args.reclassify {
        let changed = scan::reclassify_stored(&store, crate::history::DEFAULT_USER_ID).await?;
        store.close().await;

        println!("{}", format_reclassify(changed));
        return Ok(());
    }

    config.validate_inbox().map_err(|problem| {
        // The setting lives in the config file and the web UI, so say where.
        eprintln!("Inbox monitoring is not set up: {problem}");
        eprintln!();
        eprintln!("Add an `inbox:` section to your config, or turn it on from");
        eprintln!("Settings in the web interface (`eruser serve`).");
        problem
    })?;

    let brokers = paths.load_brokers()?;
    let mut monitor = Monitor::new(config.inbox.clone(), &brokers.brokers);

    let options = ScanOptions {
        days: args.days,
        user_id: crate::history::DEFAULT_USER_ID,
        include_unmatched: args.include_unmatched,
    };

    println!(
        "Reading {} for the last {} days…",
        config.inbox.email, args.days
    );
    println!();

    let result = scan::scan(&mut monitor, &store, &options, |event| {
        print!("{}", format_progress(&event));
    })
    .await;

    store.close().await;
    result?;

    Ok(())
}

/// Render one step of a scan.
fn format_progress(event: &Progress) -> String {
    match event {
        Progress::Connected => String::new(),

        Progress::Fetched { count } => match count {
            0 => "No mail in that period.\n".to_string(),
            1 => "1 message to look at.\n\n".to_string(),
            many => format!("{many} messages to look at.\n\n"),
        },

        Progress::Classified {
            index,
            total,
            broker_name,
            response_type,
            confidence,
        } => {
            let width = total.to_string().len();
            format!(
                "[{index:>width$}/{total}] {broker_name}: {} ({:.0}% sure)\n",
                label(*response_type),
                confidence * 100.0
            )
        }

        Progress::Finished(summary) => format_summary(summary),
    }
}

fn format_summary(summary: &ScanSummary) -> String {
    use std::fmt::Write;

    if summary.fetched == 0 {
        return String::new();
    }

    let mut out = String::from("\n");
    let _ = writeln!(out, "{}", "-".repeat(40));
    let _ = writeln!(
        out,
        "{} of {} messages were from known brokers.",
        summary.matched, summary.fetched
    );

    let counts = &summary.by_type;
    let _ = writeln!(out);
    if counts.success > 0 {
        let _ = writeln!(out, "  {:>4}  removed", counts.success);
    }
    if counts.form_required > 0 {
        let _ = writeln!(out, "  {:>4}  need a form filled in", counts.form_required);
    }
    if counts.confirmation_required > 0 {
        let _ = writeln!(
            out,
            "  {:>4}  need a link clicked",
            counts.confirmation_required
        );
    }
    if counts.pending > 0 {
        let _ = writeln!(out, "  {:>4}  acknowledged, still working", counts.pending);
    }
    if counts.rejected > 0 {
        let _ = writeln!(out, "  {:>4}  refused, or hold nothing", counts.rejected);
    }
    if counts.bounced > 0 {
        let _ = writeln!(
            out,
            "  {:>4}  bounced — the address may be dead",
            counts.bounced
        );
    }
    if counts.unknown > 0 {
        let _ = writeln!(out, "  {:>4}  could not be read", counts.unknown);
    }

    if counts.needs_review > 0 {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{} need a look. See them under Tasks in `eruser serve`.",
            counts.needs_review
        );
    }
    if counts.form_required + counts.confirmation_required > 0 {
        let _ = writeln!(
            out,
            "{} are waiting on you. `eruser serve` shows what each one needs.",
            counts.form_required + counts.confirmation_required
        );
    }

    out
}

fn format_reclassify(changed: usize) -> String {
    match changed {
        0 => "Re-read every stored reply. Nothing changed.".to_string(),
        1 => "Re-read every stored reply. 1 was filed differently.".to_string(),
        many => format!("Re-read every stored reply. {many} were filed differently."),
    }
}

/// Short label for a reply type, for a terminal line.
fn label(response_type: ResponseType) -> &'static str {
    match response_type {
        ResponseType::Success => "removed",
        ResponseType::FormRequired => "wants a form filled in",
        ResponseType::ConfirmationRequired => "wants a link clicked",
        ResponseType::Rejected => "refused",
        ResponseType::Pending => "acknowledged",
        ResponseType::Bounced => "BOUNCED",
        ResponseType::Unknown => "unclear, needs a look",
    }
}

/// Surface the monitor's errors through the CLI error type.
impl From<monitor::Error> for Error {
    fn from(value: monitor::Error) -> Self {
        Error::Inbox(scan::Error::Monitor(value))
    }
}

#[cfg(test)]
mod tests;
