//! `eruser serve` — start the local web interface.
//!
//! The web UI itself is not ported yet; this command exists so the interface
//! is stable and `--help` tells the truth about what is available.

use super::{Error, Paths};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Port to listen on
    #[arg(long, short, default_value_t = 8080)]
    pub port: u16,

    /// Address to bind. Leave this at localhost unless you understand the
    /// consequences: the interface has no authentication yet, so anything
    /// that can reach it can read your profile and send mail as you.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            port: 8080,
            host: "127.0.0.1".to_string(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error(
        "the web interface has not been ported yet\n\n\
         Until it lands, the command line does the same work:\n  \
         eruser init            set up your profile and email\n  \
         eruser send --dry-run  preview the requests\n  \
         eruser send            send them\n  \
         eruser status          see how it went"
    )]
    NotImplemented,
}

pub async fn run(_paths: &Paths, _args: Args) -> Result<(), Error> {
    Err(ServeError::NotImplemented.into())
}
