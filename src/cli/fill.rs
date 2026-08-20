//! `eruser fill` — drive a browser through the opt-out forms brokers ask for.

use std::path::PathBuf;

use super::{Error, Paths};
use crate::automation::browser::{self, Browser, BrowserOptions, FormOutcome};
use crate::history::{
    DEFAULT_USER_ID, FormStatus, NewPendingTask, PipelineStatus, Store, TaskType,
};

#[derive(Debug, Default, clap::Args)]
pub struct Args {
    /// A single form to fill, instead of the ones already found
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,

    /// Only this broker's form
    #[arg(long, value_name = "ID")]
    pub broker: Option<String>,

    /// Show which forms would be filled, without opening a browser
    #[arg(long)]
    pub dry_run: bool,

    /// Show the browser window instead of running it hidden
    #[arg(long)]
    pub show_browser: bool,

    /// Press submit after filling
    ///
    /// Off by default: a form that submits the wrong thing cannot be
    /// un-submitted. Without this, eruser fills the boxes, saves a picture,
    /// and leaves the sending to you.
    #[arg(long)]
    pub submit: bool,

    /// Where to save screenshots [default: ~/.eraser/screenshots]
    #[arg(long, value_name = "DIR")]
    pub screenshots: Option<PathBuf>,

    /// Do not save screenshots
    #[arg(long, conflicts_with = "screenshots")]
    pub no_screenshots: bool,
}

pub async fn run(paths: &Paths, args: Args) -> Result<(), Error> {
    let config = paths.load_config()?;
    let store = Store::open(Store::default_path()).await?;

    let forms = match &args.url {
        // A URL on the command line stands in for a broker's form.
        Some(url) => vec![("".to_string(), url.clone(), url.clone())],
        None => pending_forms(&store, args.broker.as_deref()).await?,
    };

    if forms.is_empty() {
        store.close().await;
        println!("{NOTHING_TO_FILL}");
        return Ok(());
    }

    if args.dry_run {
        store.close().await;
        print!("{}", format_dry_run(&forms));
        return Ok(());
    }

    let options = BrowserOptions {
        headless: !args.show_browser,
        screenshot_dir: screenshot_dir(&args),
        submit: args.submit,
        ..Default::default()
    };

    if args.submit {
        println!("Submitting each form after filling it.");
    } else {
        println!("Filling each form and saving a picture. Nothing will be submitted.");
    }
    println!();

    let browser = Browser::launch(config.profile.clone(), options).await?;

    let mut filled = 0usize;
    let mut needs_a_person = 0usize;

    for (broker_id, broker_name, url) in &forms {
        match browser.fill_form(url, broker_id).await {
            Ok(outcome) => {
                print!("{}", format_one(broker_name, &outcome));

                if outcome.needs_a_person() {
                    needs_a_person += 1;
                    record_task(&store, broker_id, broker_name, url, &outcome).await?;
                } else {
                    filled += 1;
                }

                if !broker_id.is_empty() {
                    store
                        .update_pipeline_status(DEFAULT_USER_ID, broker_id, stage_for(&outcome))
                        .await?;
                }
            }
            Err(error) => {
                println!("FAIL  {broker_name}: {error}");
                needs_a_person += 1;
            }
        }
    }

    browser.close().await;
    store.close().await;

    print!("{}", format_summary(filled, needs_a_person, args.submit));
    Ok(())
}

const NOTHING_TO_FILL: &str = "No opt-out forms are waiting.\n\n\
     Run `eruser monitor` first — forms are found by reading the replies \
     brokers send.";

/// The forms found in stored replies that nobody has dealt with yet.
async fn pending_forms(
    store: &Store,
    only_broker: Option<&str>,
) -> Result<Vec<(String, String, String)>, Error> {
    Ok(store
        .forms_with_status(DEFAULT_USER_ID)
        .await?
        .into_iter()
        .filter(|form| form.status == FormStatus::Pending)
        .filter(|form| only_broker.is_none_or(|wanted| form.broker_id.eq_ignore_ascii_case(wanted)))
        .map(|form| (form.broker_id, form.broker_name, form.form_url))
        .collect())
}

/// Where an outcome leaves the broker.
pub fn stage_for(outcome: &FormOutcome) -> PipelineStatus {
    if outcome
        .captcha
        .as_ref()
        .is_some_and(crate::automation::Captcha::blocks_automation)
    {
        return PipelineStatus::AwaitingCaptcha;
    }
    if outcome.submitted {
        return PipelineStatus::FormFilled;
    }
    // Filled but not sent is still waiting on a person to press the button.
    PipelineStatus::FormRequired
}

/// Record a form that a person has to finish.
///
/// The screenshot goes with it, so the task page can show what state the
/// form was left in rather than only naming the broker.
async fn record_task(
    store: &Store,
    broker_id: &str,
    broker_name: &str,
    url: &str,
    outcome: &FormOutcome,
) -> Result<(), Error> {
    if broker_id.is_empty() {
        return Ok(());
    }

    let task_type = if outcome.captcha.is_some() {
        TaskType::Captcha
    } else {
        TaskType::ManualForm
    };

    store
        .add_task(&NewPendingTask {
            user_id: DEFAULT_USER_ID,
            broker_id: broker_id.to_string(),
            broker_name: broker_name.to_string(),
            task_type,
            form_url: url.to_string(),
            screenshot_path: outcome
                .screenshot
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            browser_state: String::new(),
            notes: outcome.summary(),
        })
        .await?;

    Ok(())
}

/// Where screenshots go for this run.
fn screenshot_dir(args: &Args) -> Option<PathBuf> {
    if args.no_screenshots {
        return None;
    }
    Some(
        args.screenshots
            .clone()
            .unwrap_or_else(browser::default_screenshot_dir),
    )
}

fn format_one(broker_name: &str, outcome: &FormOutcome) -> String {
    let marker = if outcome.needs_a_person() {
        "look"
    } else {
        "ok  "
    };
    let who = if broker_name.is_empty() {
        &outcome.url
    } else {
        broker_name
    };

    let mut line = format!("{marker}  {who}: {}\n", outcome.summary());
    if let Some(path) = &outcome.screenshot {
        line.push_str(&format!("      {}\n", path.display()));
    }
    line
}

fn format_dry_run(forms: &[(String, String, String)]) -> String {
    use std::fmt::Write;

    let mut out = format!("Would fill {} forms:\n\n", forms.len());
    for (_, broker_name, url) in forms {
        let _ = writeln!(out, "  {broker_name}\n    {url}");
    }
    out
}

fn format_summary(filled: usize, needs_a_person: usize, submitted: bool) -> String {
    use std::fmt::Write;

    let mut out = String::from("\n");
    let _ = writeln!(out, "{}", "-".repeat(40));

    let verb = if submitted { "submitted" } else { "filled" };
    let _ = write!(out, "{filled} {verb}");
    if needs_a_person > 0 {
        let _ = write!(out, ", {needs_a_person} need you");
    }
    let _ = writeln!(out, ".");

    if !submitted && filled > 0 {
        let _ = writeln!(
            out,
            "Nothing was sent. Check the pictures, then submit each form yourself,"
        );
        let _ = writeln!(out, "or run again with --submit.");
    }
    if needs_a_person > 0 {
        let _ = writeln!(
            out,
            "The ones marked \"look\" are listed under Tasks in `eruser serve`."
        );
    }

    out
}

#[cfg(test)]
mod tests;
