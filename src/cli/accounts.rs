//! `eruser accounts` — the mailboxes requests are sent from.
//!
//! New in the Rust version. Go read one set of SMTP settings out of the
//! config file and sent everything through it, which meant one provider's
//! daily cap was the whole tool's daily cap. Several accounts can be
//! registered here and a run rolls over to the next when one is spent.

use super::{Error, Paths, prompt};
use crate::history::{
    AccountCapacity, AccountScope, DEFAULT_DAILY_LIMIT, DEFAULT_USER_ID, NewSenderAccount, Store,
    User,
};

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Show every sending account and what it has left today
    List(WhoArgs),

    /// Register a mailbox to send from
    Add(AddArgs),

    /// Stop using an account without removing it
    Disable(PickArgs),

    /// Use an account again
    Enable(PickArgs),

    /// Remove an account
    Remove(PickArgs),
}

#[derive(Debug, Default, clap::Args)]
pub struct WhoArgs {
    /// Whose accounts to act on [default: the only account, if there is one]
    #[arg(long, value_name = "NAME")]
    pub user: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct AddArgs {
    #[command(flatten)]
    pub who: WhoArgs,

    /// The address to send from
    #[arg(long, value_name = "ADDRESS")]
    pub from: String,

    /// A name you will recognise, e.g. "personal gmail"
    #[arg(long)]
    pub label: Option<String>,

    /// How it sends: smtp, resend, or sendgrid
    #[arg(long, default_value = "smtp")]
    pub provider: String,

    /// SMTP server [default: guessed from the address]
    #[arg(long)]
    pub host: Option<String>,

    /// SMTP port
    #[arg(long, default_value_t = 465)]
    pub port: u16,

    /// SMTP username [default: the address]
    #[arg(long)]
    pub username: Option<String>,

    /// Let anyone on this instance send through this account
    ///
    /// For a household sharing one mailbox. Replies still land in this
    /// mailbox, not the sender's.
    #[arg(long)]
    pub family: bool,

    /// How many to send from this account in a day
    #[arg(long, default_value_t = DEFAULT_DAILY_LIMIT)]
    pub daily_limit: i64,

    /// Lower goes first, so a free account is spent before a paid one
    #[arg(long, default_value_t = 0)]
    pub priority: i64,
}

#[derive(Debug, clap::Args)]
pub struct PickArgs {
    #[command(flatten)]
    pub who: WhoArgs,

    /// The address of the account
    pub from: String,
}

pub async fn run(_paths: &Paths, command: Command) -> Result<(), Error> {
    let store = Store::open(Store::default_path()).await?;
    let result = dispatch(&store, command).await;
    store.close().await;
    result
}

async fn dispatch(store: &Store, command: Command) -> Result<(), Error> {
    match command {
        Command::List(who) => {
            let user_id = resolve(store, &who).await?;
            let capacity = store.account_capacity(user_id).await?;
            print!("{}", format_accounts(&capacity));
        }

        Command::Add(args) => {
            let user_id = resolve(store, &args.who).await?;
            let account = build(user_id, &args, |message| Ok(prompt::secret(message)?))?;
            store.add_sender_account(&account).await?;

            println!("Added {}.", args.from);
            if args.family {
                println!("Anyone on this instance can send through it.");
            }
        }

        Command::Disable(args) => {
            let (user_id, id) = pick(store, &args).await?;
            store.set_sender_account_enabled(user_id, id, false).await?;
            println!("{} will not be used until it is enabled again.", args.from);
        }

        Command::Enable(args) => {
            let (user_id, id) = pick(store, &args).await?;
            store.set_sender_account_enabled(user_id, id, true).await?;
            println!("{} is in use again.", args.from);
        }

        Command::Remove(args) => {
            let (user_id, id) = pick(store, &args).await?;
            store.delete_sender_account(user_id, id).await?;
            println!("Removed {}.", args.from);
        }
    }

    Ok(())
}

/// Which account these arguments are about.
///
/// Most installs have one person, so naming them every time would be noise.
/// With several, there is no sensible default and the command says so rather
/// than guessing.
async fn resolve(store: &Store, who: &WhoArgs) -> Result<i64, Error> {
    let users = store.users().await?;

    if let Some(name) = &who.user {
        return users
            .iter()
            .find(|user| user.username.eq_ignore_ascii_case(name.trim()))
            .map(|user| user.id)
            .ok_or_else(|| Error::History(crate::history::AccountError::NoSuchUser.into()));
    }

    match users.as_slice() {
        [] => Ok(DEFAULT_USER_ID),
        [only] => Ok(only.id),
        several => Err(Error::AmbiguousUser {
            names: names_of(several),
        }),
    }
}

fn names_of(users: &[User]) -> String {
    users
        .iter()
        .map(|user| user.username.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Find the account an address names.
async fn pick(store: &Store, args: &PickArgs) -> Result<(i64, i64), Error> {
    let user_id = resolve(store, &args.who).await?;
    let accounts = store.sender_accounts(user_id).await?;

    let account = accounts
        .into_iter()
        .find(|account| account.from_address.eq_ignore_ascii_case(args.from.trim()))
        .ok_or_else(|| Error::NoSuchSenderAccount {
            address: args.from.clone(),
        })?;

    Ok((user_id, account.id))
}

/// Turn the arguments into an account, asking for the secret only once it is
/// clear one is needed.
///
/// The secret arrives through a closure rather than a string so the tests can
/// build an account without a terminal to type into.
fn build(
    user_id: i64,
    args: &AddArgs,
    secret: impl FnOnce(&str) -> Result<String, Error>,
) -> Result<NewSenderAccount, Error> {
    let from = args.from.trim().to_string();
    let provider = args.provider.trim().to_lowercase();

    let mut account = NewSenderAccount {
        user_id,
        label: args.label.clone().unwrap_or_default(),
        scope: if args.family {
            AccountScope::Family
        } else {
            AccountScope::Personal
        },
        provider: provider.clone(),
        from_address: from.clone(),
        daily_limit: args.daily_limit,
        priority: args.priority,
        ..NewSenderAccount::default()
    };

    match provider.as_str() {
        "smtp" => {
            account.smtp = crate::config::SmtpConfig {
                host: args
                    .host
                    .clone()
                    .or_else(|| smtp_host_for(&from).map(str::to_string))
                    .ok_or_else(|| Error::MissingSmtpHost {
                        address: from.clone(),
                    })?,
                port: args.port,
                username: args.username.clone().unwrap_or_else(|| from.clone()),
                // Never taken from a flag: it would sit in the shell history.
                password: secret("App password: ")?,
                use_tls: true,
            };
        }
        "resend" | "sendgrid" => {
            account.api_key = secret("API key: ")?;
        }
        other => {
            return Err(Error::UnknownProvider {
                provider: other.to_string(),
            });
        }
    }

    Ok(account)
}

/// The SMTP server for the well-known providers, so the common case needs no
/// flag. Anything else has to say `--host`.
fn smtp_host_for(address: &str) -> Option<&'static str> {
    let domain = address.rsplit_once('@')?.1.to_lowercase();
    match domain.as_str() {
        "gmail.com" | "googlemail.com" => Some("smtp.gmail.com"),
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => Some("smtp-mail.outlook.com"),
        "yahoo.com" | "ymail.com" => Some("smtp.mail.yahoo.com"),
        "icloud.com" | "me.com" | "mac.com" => Some("smtp.mail.me.com"),
        "proton.me" | "protonmail.com" | "pm.me" => Some("smtp.protonmail.ch"),
        "fastmail.com" | "fastmail.fm" => Some("smtp.fastmail.com"),
        "zoho.com" => Some("smtp.zoho.com"),
        "aol.com" => Some("smtp.aol.com"),
        _ => None,
    }
}

/// Render the account list. Pure, so the wording is testable.
pub(super) fn format_accounts(capacity: &[AccountCapacity]) -> String {
    if capacity.is_empty() {
        return "No sending accounts yet. `eruser accounts add --from <address>` adds one.\n"
            .to_string();
    }

    let width = capacity
        .iter()
        .map(|entry| entry.account.from_address.chars().count())
        .max()
        .unwrap_or(7)
        .max(7);

    let mut out = String::new();
    for entry in capacity {
        let account = &entry.account;

        let mut notes = vec![format!("{} left today", entry.remaining)];
        if account.scope.is_shared() {
            notes.push("shared with the household".to_string());
        }
        if !account.enabled {
            notes.push("disabled".to_string());
        }
        if !account.label.is_empty() {
            notes.insert(0, account.label.clone());
        }

        out.push_str(&format!(
            "{:<width$}  {:<8}  {}\n",
            account.from_address,
            account.provider,
            notes.join(" · "),
            width = width
        ));
    }

    let total: i64 = capacity
        .iter()
        .filter(|entry| entry.is_available())
        .map(|entry| entry.remaining)
        .sum();
    out.push_str(&format!("\n{total} can be sent today in total.\n"));

    out
}

#[cfg(test)]
mod tests;
