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

    /// Keep reading, rather than reading once and stopping
    #[arg(long, conflicts_with = "reclassify")]
    pub watch: bool,

    /// Minutes between reads when watching
    #[arg(long, default_value_t = DEFAULT_WATCH_MINUTES, requires = "watch")]
    pub interval: u64,

    /// After the scan, draft replies for the replies that deserve one.
    /// Needs `ai:` to be set up; without it, this is a no-op.
    #[arg(long)]
    pub draft: bool,
}

/// How long to wait between reads when watching.
///
/// Brokers answer over days, not seconds, so there is nothing to gain from
/// checking more often — and every check is a login the provider counts.
pub const DEFAULT_WATCH_MINUTES: u64 = 5;

impl Default for Args {
    fn default() -> Self {
        Self {
            days: scan::DEFAULT_DAYS,
            include_unmatched: false,
            reclassify: false,
            watch: false,
            interval: DEFAULT_WATCH_MINUTES,
            draft: false,
        }
    }
}

pub async fn run(paths: &Paths, args: Args) -> Result<(), Error> {
    let store = Store::open(Store::default_path()).await?;
    let config = paths
        .settings_for(&store, crate::history::DEFAULT_USER_ID)
        .await?;

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

    let result = if args.watch {
        watch(&mut monitor, &store, &options, args.interval).await
    } else {
        scan::scan(&mut monitor, &store, &options, |event| {
            print!("{}", format_progress(&event));
        })
        .await
        .map(|_| ())
    };

    store.close().await;
    result?;

    if args.draft {
        crate::cli::draft_replies::run_standalone(paths, true).await?;
    }

    Ok(())
}

/// Read the mailbox over and over until interrupted.
///
/// Go held an IMAP IDLE connection open and reacted to the server pushing an
/// update. That is prompter, but servers drop an idle connection after about
/// half an hour and upstream's loop did not reconnect, so watching quietly
/// stopped working. Reading again on a timer is duller and keeps working:
/// each pass connects, reads, and disconnects, so a dropped connection, a
/// laptop waking from sleep, or a provider restarting costs one cycle
/// instead of ending the watch.
async fn watch(
    monitor: &mut Monitor,
    store: &Store,
    options: &ScanOptions,
    interval_minutes: u64,
) -> Result<(), scan::Error> {
    let interval = std::time::Duration::from_secs(interval_minutes.max(1) * 60);
    println!("{}", format_watch_start(interval_minutes));

    loop {
        // A failed pass is reported and retried. The mailbox being briefly
        // unreachable is not a reason to abandon a watch that is meant to
        // run for days.
        match scan::scan(monitor, store, options, |event| {
            print!("{}", format_progress(&event));
        })
        .await
        {
            Ok(_) => {}
            Err(problem) => {
                // scan disconnects only on success. A pass that died part
                // way through leaves whatever is left of the connection
                // behind; drop it here so the retry starts clean rather
                // than stacking a new session on top of a dead one.
                monitor.disconnect().await;
                eprintln!("Could not read the mailbox: {problem}");
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = tokio::signal::ctrl_c() => {
                println!();
                println!("Stopped watching.");
                return Ok(());
            }
        }
    }
}

/// What to say when a watch starts. Pure, so the wording is testable.
fn format_watch_start(interval_minutes: u64) -> String {
    let minutes = interval_minutes.max(1);
    let every = if minutes == 1 {
        "every minute".to_string()
    } else {
        format!("every {minutes} minutes")
    };

    format!("Watching {every}. Press Ctrl-C to stop.\n")
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
