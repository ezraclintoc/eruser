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

/// What to say when the whole pipeline is switched off.
const NOT_SET_UP: &str = "Reply drafting is not set up.\n\n\
Add an `ai:` section to your config with `enabled: true`, and say who writes the \
reply — the shipped wording, or a model you run:\n\n  \
ai:\n  \
enabled: true\n  \
wording: canned          # use the replies eruser ships\n\n  \
or\n\n  \
ai:\n  \
enabled: true\n  \
endpoint: http://localhost:11434/v1\n  \
model: qwen3:4b\n";

/// What to say when the wording is meant to be generated but nothing can
/// generate it.
const NO_DRAFTER: &str = "Drafting is enabled with `wording: generated`, but no \
drafting model is configured.\n\nEither set `wording: canned` to use the replies \
eruser ships, or give the `ai:` section an `endpoint` and a `model`:\n\n  \
ai:\n  \
endpoint: http://localhost:11434/v1\n  \
model: qwen3:4b\n";

async fn run_with(store: &Store, paths: &Paths, quiet: bool) -> Result<(), Error> {
    let pipeline = paths.pipeline_settings();

    // One master switch: the reply pipeline does nothing at all until it is
    // switched on. After that, `wording` decides who writes — and the
    // pre-written replies need no model anywhere.
    if !pipeline.ai.enabled {
        if !quiet {
            print!("{NOT_SET_UP}");
        }
        return Ok(());
    }

    let drafter = crate::reply::from_config(&pipeline.ai);
    if pipeline.ai.wording == crate::config::Wording::Generated && drafter.is_none() {
        if !quiet {
            print!("{NO_DRAFTER}");
        }
        return Ok(());
    }

    let decider = crate::decision::from_config(&pipeline.decider);
    let settings = pipeline::Drafting::from_config(
        &pipeline.ai,
        &pipeline.decider,
        drafter.as_deref(),
        decider.as_deref(),
    );

    let config = paths
        .settings_for(store, crate::history::DEFAULT_USER_ID)
        .await?;

    if !quiet {
        println!("{}", banner(&pipeline.ai, drafter.is_some()));
        println!();
    }

    let summary = pipeline::draft_replies(
        store,
        &settings,
        &config.profile,
        crate::history::DEFAULT_USER_ID,
        pipeline.ai.max_drafts_per_run,
    )
    .await
    .map_err(Error::Draft)?;

    print!("{}", format_summary(&summary, quiet));
    Ok(())
}

/// The opening line: what is producing the wording this run.
fn banner(ai: &crate::config::AiConfig, has_drafter: bool) -> String {
    match ai.wording {
        crate::config::Wording::Canned => {
            "Drafting replies from the replies eruser ships…".to_string()
        }
        crate::config::Wording::Generated if has_drafter => {
            format!("Drafting replies with {}…", ai.model)
        }
        crate::config::Wording::Generated => "Drafting replies…".to_string(),
    }
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
