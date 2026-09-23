//! `eruser draft-replies` — write the replies brokers asked for.
//!
//! Reads the replies already in history — no mailbox, no network beyond the
//! local model — and drafts answers for the ones that deserve one. Every
//! draft lands on the task list; nothing is sent here. Auto-send belongs to
//! the send path, and only ever for the whitelisted reply types.

use super::{Error, Paths};
use crate::history::Store;
use crate::reply::pipeline::{self, DraftRunSummary};

#[derive(Debug, Default, clap::Args)]
pub struct Args {
    /// Draft for one broker only
    #[arg(long, value_name = "ID")]
    pub broker: Option<String>,
}

pub async fn run(paths: &Paths, _args: Args) -> Result<(), Error> {
    run_standalone(paths, false).await
}

/// The drafting pass, shared by this command and `monitor --draft`.
///
/// `quiet` trims the banner when it runs as the tail of a monitor scan.
pub async fn run_standalone(paths: &Paths, quiet: bool) -> Result<(), Error> {
    let store = Store::open(Store::default_path()).await?;
    let result = run_with(&store, paths, quiet).await;
    store.close().await;
    result
}

async fn run_with(store: &Store, paths: &Paths, quiet: bool) -> Result<(), Error> {
    let pipeline = paths.pipeline_settings();
    let Some(drafter) = crate::reply::from_config(&pipeline.ai) else {
        if !quiet {
            println!(
                "Reply drafting is not set up.\n\nAdd an `ai:` section to your config with \
                 `enabled: true`, an `endpoint` and a `model`:\n\n  \
                 ai:\n    \
                 enabled: true\n    \
                 endpoint: http://localhost:11434/v1\n    \
                 model: qwen3:4b\n"
            );
        }
        return Ok(());
    };

    let config = paths
        .settings_for(store, crate::history::DEFAULT_USER_ID)
        .await?;

    if !quiet {
        println!("Drafting replies with {}…", pipeline.ai.model);
        println!();
    }

    let summary = pipeline::draft_replies(
        store,
        drafter.as_ref(),
        &config.profile,
        crate::history::DEFAULT_USER_ID,
        pipeline.ai.max_drafts_per_run,
    )
    .await
    .map_err(Error::Draft)?;

    print!("{}", format_summary(&summary, quiet));
    Ok(())
}

/// Render the run's outcome. Pure, so the wording is testable.
fn format_summary(summary: &DraftRunSummary, quiet: bool) -> String {
    let mut out = String::new();
    let replies = |count: usize| {
        if count == 1 { "reply" } else { "replies" }
    };

    if summary.drafted == 0 {
        if summary.already_answered > 0 {
            let count = summary.already_answered;
            out.push_str(&format!(
                "{count} {} already have a draft waiting.\n",
                replies(count)
            ));
        } else if !quiet {
            out.push_str("No replies look like they need an answer.\n");
        }
        return out;
    }

    let drafted = summary.drafted;
    out.push_str(&format!(
        "Drafted {drafted} {}. Nothing has been sent — they are waiting on the \
         task list for you to read and send.\n",
        replies(drafted)
    ));
    if summary.refused > 0 {
        let refused = summary.refused;
        let was = if refused == 1 {
            "1 draft was"
        } else {
            "{refused} drafts were"
        };
        let was = was.replace("{refused}", &refused.to_string());
        out.push_str(&format!(
            "{was} refused by the safety checks and kept nothing.\n"
        ));
    }

    out
}

#[cfg(test)]
mod tests;
