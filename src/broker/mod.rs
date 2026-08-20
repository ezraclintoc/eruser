//! Broker database: loading, filtering, and mutation.
//!
//! Ported from `internal/broker/broker.go`. The Go version returned
//! `*Broker` pointers into the backing slice and signalled "not found" with
//! `nil`; here lookups return `Option<&Broker>` and removals return the owned
//! `Broker` that was taken out.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod error;
pub use error::Error;

/// A data broker: a company that collects and sells personal information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Broker {
    /// Unique lowercase hyphenated identifier, e.g. `data-axle`.
    pub id: String,
    pub name: String,
    /// Privacy / removal contact address. Required.
    pub email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub website: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub opt_out_url: String,
    /// `us`, `eu`, or `global`.
    pub region: String,
    /// `people-search`, `marketing`, `background-check`, ...
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Whether the broker demands identity verification.
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_id: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Broker {
    /// Blank out `website` and `opt_out_url` unless they are http(s) URLs.
    ///
    /// The database is community-edited, and these fields end up in rendered
    /// emails and in `href` attributes in the web UI, so anything that is not
    /// plainly an http(s) URL is dropped rather than passed through.
    fn sanitize(&mut self) {
        if !is_valid_url(&self.opt_out_url) {
            self.opt_out_url.clear();
        }
        if !is_valid_url(&self.website) {
            self.website.clear();
        }
    }
}

/// An empty string is allowed (the field is optional); anything else must
/// parse as a URL with an http or https scheme.
fn is_valid_url(raw: &str) -> bool {
    if raw.is_empty() {
        return true;
    }
    match raw.split_once("://") {
        Some((scheme, rest)) => {
            !rest.is_empty() && matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
        }
        None => false,
    }
}

/// The broker database, as stored in `data/brokers.yaml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrokerDatabase {
    #[serde(default)]
    pub brokers: Vec<Broker>,
}

/// The database shipped with eruser, embedded at compile time.
///
/// Having it in the binary means a copied-out `eruser` still works with no
/// data directory beside it; the Go version silently loaded zero brokers.
const EMBEDDED: &str = include_str!("../../data/brokers.yaml");

impl BrokerDatabase {
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let data = std::fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut db: BrokerDatabase =
            serde_norway::from_str(&data).map_err(|source| Error::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        for broker in &mut db.brokers {
            broker.sanitize();
        }
        Ok(db)
    }

    /// Parse the database embedded in the binary.
    pub fn embedded() -> Result<Self, Error> {
        let mut db: BrokerDatabase =
            serde_norway::from_str(EMBEDDED).map_err(|source| Error::Parse {
                path: PathBuf::from("<embedded>"),
                source,
            })?;
        for broker in &mut db.brokers {
            broker.sanitize();
        }
        Ok(db)
    }

    /// Load and concatenate every `.yaml` / `.yml` file in a directory.
    pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<Self, Error> {
        let dir = dir.as_ref();
        let entries = std::fs::read_dir(dir).map_err(|source| Error::Read {
            path: dir.to_path_buf(),
            source,
        })?;

        // Directory order is unspecified across platforms; sort so that a
        // given directory always produces the same database.
        let mut paths: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("yaml") | Some("yml")
                    )
            })
            .collect();
        paths.sort();

        let mut db = BrokerDatabase::default();
        for path in paths {
            db.brokers.extend(Self::load_from_file(path)?.brokers);
        }
        Ok(db)
    }

    /// Brokers matching `regions`, minus anything named in `excluded`.
    ///
    /// An empty `regions` list means "no region filter". Brokers whose region
    /// is `global` always pass the region filter, matching upstream.
    /// `excluded` entries are matched case-insensitively against both the
    /// broker id and the broker name.
    pub fn filter(&self, regions: &[String], excluded: &[String]) -> Vec<&Broker> {
        let region_set = to_set(regions);
        let excluded_set = to_set(excluded);

        self.brokers
            .iter()
            .filter(|b| {
                if excluded_set.contains(&b.id.to_lowercase())
                    || excluded_set.contains(&b.name.to_lowercase())
                {
                    return false;
                }
                if region_set.is_empty() {
                    return true;
                }
                let region = b.region.to_lowercase();
                region_set.contains(&region) || region == "global"
            })
            .collect()
    }

    pub fn find_by_id(&self, id: &str) -> Option<&Broker> {
        let id = id.to_lowercase();
        self.brokers.iter().find(|b| b.id.to_lowercase() == id)
    }

    pub fn find_by_email(&self, email: &str) -> Option<&Broker> {
        let email = email.to_lowercase();
        self.brokers
            .iter()
            .find(|b| b.email.to_lowercase() == email)
    }

    /// Remove a broker by id, returning it if it was present.
    pub fn remove_by_id(&mut self, id: &str) -> Option<Broker> {
        let id = id.to_lowercase();
        let index = self
            .brokers
            .iter()
            .position(|b| b.id.to_lowercase() == id)?;
        Some(self.brokers.remove(index))
    }

    /// Remove a broker by contact email, returning it if it was present.
    pub fn remove_by_email(&mut self, email: &str) -> Option<Broker> {
        let email = email.to_lowercase();
        let index = self
            .brokers
            .iter()
            .position(|b| b.email.to_lowercase() == email)?;
        Some(self.brokers.remove(index))
    }

    /// Append a broker, rejecting a duplicate id.
    pub fn add(&mut self, broker: Broker) -> Result<(), Error> {
        if self.find_by_id(&broker.id).is_some() {
            return Err(Error::DuplicateId(broker.id));
        }
        self.brokers.push(broker);
        Ok(())
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        let data = serde_norway::to_string(self).map_err(Error::Serialize)?;
        std::fs::write(path, data).map_err(|source| Error::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Save, first copying any existing file to `<path>.bak`.
    pub fn save_with_backup(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        if path.exists() {
            let backup = path.with_extension(match path.extension().and_then(|e| e.to_str()) {
                Some(ext) => format!("{ext}.bak"),
                None => "bak".to_string(),
            });
            std::fs::copy(path, &backup).map_err(|source| Error::Write {
                path: backup,
                source,
            })?;
        }
        self.save(path)
    }
}

fn to_set(items: &[String]) -> std::collections::HashSet<String> {
    items.iter().map(|s| s.to_lowercase()).collect()
}

#[cfg(test)]
mod tests;
