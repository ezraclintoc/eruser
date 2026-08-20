//! `eruser add-broker` — append an entry to the broker database.

use super::{Error, Paths, prompt};
use crate::broker::{Broker, BrokerDatabase};

pub fn run(paths: &Paths) -> Result<(), Error> {
    println!("Add a data broker");
    println!("-----------------");
    println!();

    let name = prompt::line("Name: ")?;
    let suggested_id = slugify(&name);
    let id = prompt::line_or(&format!("Id [{suggested_id}]: "), &suggested_id)?;

    let broker = Broker {
        id: slugify(&id),
        name,
        email: prompt::line("Privacy or removal email: ")?,
        website: prompt::line("Website: ")?,
        opt_out_url: prompt::line("Opt-out URL: ")?,
        region: prompt::line_or("Region — us, eu, or global [us]: ", "us")?,
        category: prompt::line("Category — people-search, marketing, background-check: ")?,
        notes: String::new(),
        requires_id: false,
        tags: Vec::new(),
    };

    // Only a file can be edited; the embedded database is read-only, so say
    // so rather than appearing to add an entry that vanishes.
    let Some(path) = paths.broker_path() else {
        println!();
        println!("There is no broker file to write to.");
        println!("Copy the database out of the repository first, then pass it:");
        println!("  eruser --brokers ./data/brokers.yaml add-broker");
        return Err(Error::Cancelled);
    };

    let mut db = if path.exists() {
        BrokerDatabase::load_from_file(&path)?
    } else {
        BrokerDatabase::default()
    };

    let name = broker.name.clone();
    db.add(broker)?;
    // Back up first: this file is the product, and a botched write loses 764
    // community-contributed entries.
    db.save_with_backup(&path)?;

    println!();
    println!("Added {name} to {}", path.display());
    Ok(())
}

/// Turn a display name into an id: lowercase, words joined by hyphens.
pub(super) fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut pending_hyphen = false;

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_hyphen && !slug.is_empty() {
                slug.push('-');
            }
            pending_hyphen = false;
            slug.push(ch.to_ascii_lowercase());
        } else {
            pending_hyphen = true;
        }
    }
    slug
}
