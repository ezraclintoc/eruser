//! `eruser users` — the accounts people sign in with.
//!
//! New in the Rust version; the Go tool had no accounts. This exists mostly
//! so a forgotten password is recoverable: the web interface cannot offer a
//! reset by email, because the only mailbox it knows about is the one it
//! sends removal requests from, and anyone who can read that mailbox could
//! then take the account.

use super::{Error, Paths, prompt};
use crate::history::{AccountError, MINIMUM_PASSWORD_LENGTH, Store, User};

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Show everyone with an account here
    List,

    /// Create an account
    Add(NameArg),

    /// Change an account's password
    SetPassword(NameArg),

    /// Remove an account and everything belonging to it
    Remove(NameArg),
}

#[derive(Debug, clap::Args)]
pub struct NameArg {
    /// The account's name
    pub username: String,
}

pub async fn run(_paths: &Paths, command: Command) -> Result<(), Error> {
    let store = Store::open(Store::default_path()).await?;
    let result = dispatch(&store, command).await;
    store.close().await;
    result
}

async fn dispatch(store: &Store, command: Command) -> Result<(), Error> {
    match command {
        Command::List => {
            let users = store.users().await?;
            print!("{}", format_users(&users));
        }

        Command::Add(arg) => {
            let password = read_new_password()?;
            let user = store.create_user(&arg.username, &password).await?;
            println!("Created {}.", user.username);
        }

        Command::SetPassword(arg) => {
            let user = find(store, &arg.username).await?;
            let password = read_new_password()?;
            store.set_password(user.id, &password).await?;
            println!("The password for {} has been changed.", user.username);
        }

        Command::Remove(arg) => {
            let user = find(store, &arg.username).await?;

            // Their history goes with them, and there is no undo.
            let confirm = prompt::line(&format!(
                "Remove {} and everything they have sent? Type the name to confirm: ",
                user.username
            ))?;
            if confirm.trim() != user.username {
                return Err(Error::Cancelled);
            }

            if !store.delete_user(user.id).await? {
                return Err(Error::LastAccount);
            }
            println!("Removed {}.", user.username);
        }
    }

    Ok(())
}

async fn find(store: &Store, username: &str) -> Result<User, Error> {
    store
        .user_by_name(username)
        .await?
        .ok_or_else(|| Error::History(AccountError::NoSuchUser.into()))
}

/// Ask twice, so a typo does not become the password.
fn read_new_password() -> Result<String, Error> {
    let password = prompt::secret("New password: ")?;
    if password.chars().count() < MINIMUM_PASSWORD_LENGTH {
        return Err(Error::History(AccountError::PasswordTooShort.into()));
    }

    let again = prompt::secret("Again: ")?;
    if password != again {
        return Err(Error::PasswordsDiffer);
    }

    Ok(password)
}

/// Render the account list. Pure, so the wording is testable.
pub(super) fn format_users(users: &[User]) -> String {
    if users.is_empty() {
        return "No accounts yet. `eruser users add <name>` makes the first one.\n".to_string();
    }

    let width = users
        .iter()
        .map(|user| user.username.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);

    let mut out = String::new();
    for user in users {
        let created = user
            .created_at
            .map(|at| at.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "—".to_string());

        // An account with no password is one the web interface offers to
        // claim, so it is worth pointing out rather than showing as normal.
        let state = if user.has_password {
            String::new()
        } else {
            "  (no password set — unclaimed)".to_string()
        };

        out.push_str(&format!(
            "{:<width$}  added {created}{state}\n",
            user.username,
            width = width
        ));
    }

    out
}

#[cfg(test)]
mod tests;
