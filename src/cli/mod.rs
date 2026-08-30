//! Command-line interface.
//!
//! Ported from `cmd/eraser/main.go`, which held every command and all of
//! their logic in one 1,577-line file. Here `mod.rs` only describes the
//! interface; each command's implementation is its own module, and the parts
//! that produce output are pure functions so they can be tested.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::broker::BrokerDatabase;
use crate::config::{self, Config};

pub(crate) mod accounts;
pub(crate) mod add_broker;
pub(crate) mod confirm;
pub(crate) mod fill;
pub(crate) mod init;
pub(crate) mod list_brokers;
pub(crate) mod monitor;
mod prompt;
pub(crate) mod send;
pub(crate) mod serve;
pub(crate) mod status;
pub(crate) mod users;

pub use serve::ServeError;

/// Automated data broker removal requests.
#[derive(Debug, Parser)]
#[command(
    name = "eruser",
    version,
    about = "Send data removal requests to 750+ data brokers",
    long_about = "eruser sends data removal requests to data brokers on your behalf.

It supports GDPR, CCPA, and generic request templates, sends over SMTP, and
keeps a local record of what has been sent and what came back."
)]
pub struct Cli {
    /// Config file to use [default: ~/.eraser/config.yaml]
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Broker database to use [default: ./data/brokers.yaml, else the
    /// database built into this binary]
    #[arg(long, global = true, value_name = "PATH")]
    pub brokers: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a configuration file, answering a few questions
    Init,

    /// Send removal requests to data brokers
    Send(send::Args),

    /// List the data brokers in the database
    ListBrokers(list_brokers::Args),

    /// Show what has been sent, and how it went
    Status(status::Args),

    /// Add a broker to the database
    AddBroker,

    /// Read the mailbox and sort what brokers sent back
    Monitor(monitor::Args),

    /// Follow the confirmation links brokers sent
    Confirm(confirm::Args),

    /// Fill in the opt-out forms brokers asked for
    Fill(fill::Args),

    /// Start the local web interface
    Serve(serve::Args),

    /// Manage the accounts people sign in with
    #[command(subcommand)]
    Users(users::Command),

    /// Manage the mailboxes requests are sent from
    #[command(subcommand)]
    Accounts(accounts::Command),
}

impl Cli {
    pub async fn run(self) -> Result<(), Error> {
        let paths = Paths {
            config: self.config,
            brokers: self.brokers,
        };

        match self.command {
            Command::Init => init::run(&paths),
            Command::Send(args) => send::run(&paths, args).await,
            Command::ListBrokers(args) => list_brokers::run(&paths, args),
            Command::Status(args) => status::run(&paths, args).await,
            Command::AddBroker => add_broker::run(&paths),
            Command::Monitor(args) => monitor::run(&paths, args).await,
            Command::Confirm(args) => confirm::run(&paths, args).await,
            Command::Fill(args) => fill::run(&paths, args).await,
            Command::Serve(args) => serve::run(&paths, args).await,
            Command::Users(command) => users::run(&paths, command).await,
            Command::Accounts(command) => accounts::run(&paths, command).await,
        }
    }
}

/// Where the config and broker database live for this invocation.
#[derive(Debug, Default, Clone)]
pub struct Paths {
    pub config: Option<PathBuf>,
    pub brokers: Option<PathBuf>,
}

impl Paths {
    pub fn config_path(&self) -> PathBuf {
        self.config
            .clone()
            .unwrap_or_else(config::default_config_path)
    }

    /// The broker file to read, if there is one.
    ///
    /// An explicit `--brokers` always wins, even when it does not exist, so a
    /// typo is reported rather than silently falling back to a different
    /// database than the one asked for.
    pub fn broker_path(&self) -> Option<PathBuf> {
        if let Some(path) = &self.brokers {
            return Some(path.clone());
        }
        default_broker_file()
    }

    /// Load the broker database, falling back to the embedded copy.
    pub fn load_brokers(&self) -> Result<BrokerDatabase, Error> {
        match self.broker_path() {
            Some(path) => Ok(BrokerDatabase::load_from_file(path)?),
            None => Ok(BrokerDatabase::embedded()?),
        }
    }

    /// Load the config, refusing to continue if it is missing or unusable.
    pub fn load_config(&self) -> Result<Config, Error> {
        let path = self.config_path();
        if !path.exists() {
            return Err(Error::NoConfig { path });
        }
        Ok(Config::load(path)?)
    }

    /// The settings a command should act on.
    ///
    /// Settings live in the database now, one set per person, so that the
    /// web interface can have several people on one instance. A `config.yaml`
    /// left over from before is moved across the first time it is seen, and
    /// after that the file is only a starting point — editing the database
    /// is not undone by a stale file.
    pub async fn settings_for(
        &self,
        store: &crate::history::Store,
        user_id: i64,
    ) -> Result<Config, Error> {
        let path = self.config_path();
        if path.exists()
            && let Ok(config) = Config::load(&path)
        {
            store.import_config(user_id, &config).await?;
        }

        let config = store.config_for(user_id).await?;
        if config.profile.first_name.is_empty() && config.email.from.is_empty() {
            // Nothing in the database and nothing usable on disk: this
            // install has not been set up at all.
            return Err(Error::NoConfig { path });
        }

        Ok(config)
    }
}

/// `./data/brokers.yaml`, or the same path beside the executable.
fn default_broker_file() -> Option<PathBuf> {
    let local = Path::new("data/brokers.yaml");
    if local.is_file() {
        return Some(local.to_path_buf());
    }

    let beside_exe = std::env::current_exe()
        .ok()?
        .parent()?
        .join("data")
        .join("brokers.yaml");
    beside_exe.is_file().then_some(beside_exe)
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no config file at {path}\n\nRun `eruser init` to create one.")]
    NoConfig { path: PathBuf },

    #[error(transparent)]
    Config(#[from] config::Error),

    #[error("the config is not usable yet")]
    InvalidConfig(#[from] config::ValidationError),

    #[error(transparent)]
    Broker(#[from] crate::broker::Error),

    #[error(transparent)]
    Template(#[from] crate::template::Error),

    #[error(transparent)]
    Email(#[from] crate::email::Error),

    #[error(transparent)]
    History(#[from] crate::history::Error),

    #[error(transparent)]
    Serve(#[from] ServeError),

    #[error(transparent)]
    Inbox(#[from] crate::inbox::scan::Error),

    #[error(transparent)]
    Confirm(#[from] crate::automation::confirm::Error),

    #[error(transparent)]
    Browser(#[from] crate::automation::browser::Error),

    #[error("could not read from the terminal")]
    Input(#[from] std::io::Error),

    #[error(
        "every request failed ({count} of them)\n\nThis usually means the email settings are wrong. Check `eruser status` for\nthe reason, then fix the config or run `eruser init` again."
    )]
    AllSendsFailed { count: usize },

    #[error(
        "every sending account has used its allowance for today\n\nAdd another account, or run this again tomorrow. `eruser accounts` shows\nwhat each one has left."
    )]
    NoCapacityToday,

    #[error("cancelled")]
    Cancelled,

    #[error("the two passwords are not the same")]
    PasswordsDiffer,

    #[error(
        "that is the only account left\n\nAn instance with no accounts cannot be signed into. Add another account\nfirst if you meant to replace this one."
    )]
    LastAccount,

    #[error(
        "there are several accounts here, so this needs to say which one\n\nPass --user with one of: {names}"
    )]
    AmbiguousUser { names: String },

    #[error("no sending account here uses {address}\n\n`eruser accounts` lists them.")]
    NoSuchSenderAccount { address: String },

    #[error(
        "there is no SMTP server known for {address}\n\nPass --host with your provider's server, e.g. --host smtp.example.com."
    )]
    MissingSmtpHost { address: String },

    #[error("{provider} is not a way of sending\n\nUse smtp, resend, or sendgrid.")]
    UnknownProvider { provider: String },
}

#[cfg(test)]
mod tests;
