//! `eruser status` — what has been sent, and how it went.

use super::{Error, Paths};
use crate::history::{DEFAULT_USER_ID, Record, Stats, Status, Store};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// How many recent requests to show
    #[arg(long, default_value_t = 20)]
    pub limit: i64,

    /// Only show requests that failed
    #[arg(long)]
    pub failed: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            limit: 20,
            failed: false,
        }
    }
}

pub async fn run(_paths: &Paths, args: Args) -> Result<(), Error> {
    let store = Store::open(Store::default_path()).await?;

    let all_time = store.stats(DEFAULT_USER_ID).await?;
    let this_month = store.monthly_stats(DEFAULT_USER_ID).await?;
    let mut recent = store.recent_requests(DEFAULT_USER_ID, args.limit).await?;
    if args.failed {
        recent.retain(|record| record.status == Status::Failed);
    }

    print!(
        "{}",
        format_status(all_time, this_month, &recent, args.limit)
    );
    store.close().await;
    Ok(())
}

/// Render the report. Pure, so the wording is testable.
pub(super) fn format_status(
    all_time: Stats,
    this_month: Stats,
    recent: &[Record],
    limit: i64,
) -> String {
    use std::fmt::Write;

    let mut out = String::new();

    if all_time.total == 0 {
        let _ = writeln!(out, "Nothing sent yet.");
        let _ = writeln!(out);
        let _ = writeln!(out, "Run `eruser send --dry-run` to see what would go out.");
        return out;
    }

    let _ = writeln!(out, "Removal requests");
    let _ = writeln!(out, "{}", "-".repeat(40));
    let _ = writeln!(
        out,
        "  all time     {} sent, {} failed",
        all_time.sent, all_time.failed
    );
    let _ = writeln!(
        out,
        "  this month   {} sent, {} failed",
        this_month.sent, this_month.failed
    );

    if recent.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "No requests match.");
        return out;
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "Most recent {}", recent.len().min(limit as usize));
    let _ = writeln!(out, "{}", "-".repeat(40));

    for record in recent {
        let when = record
            .sent_at
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "unknown time".to_string());

        let marker = match record.status {
            Status::Sent => "ok  ",
            Status::Failed => "FAIL",
            Status::Pending => "wait",
        };

        let _ = writeln!(
            out,
            "{marker}  {when}  {}  ({})",
            record.broker_name, record.template
        );
        if !record.error.is_empty() {
            let _ = writeln!(out, "        {}", record.error);
        }
    }

    out
}
