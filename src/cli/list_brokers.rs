//! `eruser list-brokers` — show what is in the broker database.

use super::{Error, Paths};
use crate::broker::Broker;

#[derive(Debug, Default, clap::Args)]
pub struct Args {
    /// Only brokers in this region: us, eu, or global
    #[arg(long)]
    pub region: Option<String>,

    /// Only brokers in this category, e.g. people-search
    #[arg(long)]
    pub category: Option<String>,

    /// Only brokers whose name, id, or address contains this text
    #[arg(long, value_name = "TEXT")]
    pub search: Option<String>,

    /// One id per line, for piping into other commands
    #[arg(long)]
    pub ids_only: bool,
}

pub fn run(paths: &Paths, args: Args) -> Result<(), Error> {
    let db = paths.load_brokers()?;
    let matched: Vec<&Broker> = db.brokers.iter().filter(|b| matches(b, &args)).collect();

    print!("{}", format_brokers(&matched, db.brokers.len(), &args));
    Ok(())
}

/// Whether a broker passes the filters given on the command line.
pub(super) fn matches(broker: &Broker, args: &Args) -> bool {
    if let Some(region) = &args.region
        && !broker.region.eq_ignore_ascii_case(region)
    {
        return false;
    }
    if let Some(category) = &args.category
        && !broker.category.eq_ignore_ascii_case(category)
    {
        return false;
    }
    if let Some(search) = &args.search {
        let needle = search.to_lowercase();
        let haystack = format!(
            "{} {} {}",
            broker.name.to_lowercase(),
            broker.id.to_lowercase(),
            broker.email.to_lowercase()
        );
        if !haystack.contains(&needle) {
            return false;
        }
    }
    true
}

/// Render the listing.
///
/// Kept separate from `run` so the formatting is testable without a terminal.
pub(super) fn format_brokers(brokers: &[&Broker], total: usize, args: &Args) -> String {
    use std::fmt::Write;

    if args.ids_only {
        let mut out = String::new();
        for broker in brokers {
            let _ = writeln!(out, "{}", broker.id);
        }
        return out;
    }

    let mut out = String::new();

    if brokers.is_empty() {
        let _ = writeln!(out, "No brokers matched. The database has {total}.");
        return out;
    }

    if brokers.len() == total {
        let _ = writeln!(out, "{total} data brokers");
    } else {
        let _ = writeln!(out, "{} of {total} data brokers", brokers.len());
    }
    let _ = writeln!(out, "{}", "-".repeat(40));

    for broker in brokers {
        let _ = writeln!(out);
        let _ = writeln!(out, "{}  [{}]", broker.name, broker.id);
        let _ = writeln!(out, "  email    {}", broker.email);
        if !broker.website.is_empty() {
            let _ = writeln!(out, "  site     {}", broker.website);
        }
        if !broker.opt_out_url.is_empty() {
            let _ = writeln!(out, "  opt out  {}", broker.opt_out_url);
        }
        let _ = writeln!(out, "  region   {}", broker.region);
        if !broker.category.is_empty() {
            let _ = writeln!(out, "  category {}", broker.category);
        }
        if broker.requires_id {
            let _ = writeln!(out, "  note     asks for ID verification");
        }
        if !broker.notes.is_empty() {
            let _ = writeln!(out, "  note     {}", broker.notes);
        }
    }

    out
}
