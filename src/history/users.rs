//! People with accounts on this instance, and how they sign in.
//!
//! New in the Rust version. The `users` table has existed since the first
//! migration with a NULL password hash, so nothing has to be rewritten to
//! turn authentication on — the rows were already there and every query was
//! already scoped by them.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;

use super::{Error, Store};

/// Short enough to be typable, long enough to be worth hashing.
pub const MINIMUM_PASSWORD_LENGTH: usize = 8;

/// Someone who can sign in.
///
/// Deliberately has no password field. The hash is read only by
/// [`Store::verify_password`], so there is nowhere for it to be logged,
/// serialized, or rendered into a page by accident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    /// Whether a password has been set. An account without one cannot be
    /// signed into.
    pub has_password: bool,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountError {
    #[error("a username is required")]
    MissingUsername,

    #[error("a username may only contain letters, numbers, dots, dashes and underscores")]
    InvalidUsername,

    #[error("that name is already taken")]
    UsernameTaken,

    #[error("a password of at least {MINIMUM_PASSWORD_LENGTH} characters is required")]
    PasswordTooShort,

    #[error("no such account")]
    NoSuchUser,

    #[error("that is not the right password")]
    WrongPassword,

    #[error("this account has no password set, so it cannot be signed into")]
    NoPasswordSet,
}

impl Store {
    /// Everyone with an account, oldest first.
    pub async fn users(&self) -> Result<Vec<User>, Error> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY id ASC")
            .fetch_all(self.pool())
            .await?;

        rows.into_iter().map(user_from_row).collect()
    }

    pub async fn user(&self, id: i64) -> Result<Option<User>, Error> {
        let row = sqlx::query("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool())
            .await?;

        row.map(user_from_row).transpose()
    }

    pub async fn user_by_name(&self, username: &str) -> Result<Option<User>, Error> {
        let row = sqlx::query("SELECT * FROM users WHERE username = ? COLLATE NOCASE")
            .bind(username.trim())
            .fetch_optional(self.pool())
            .await?;

        row.map(user_from_row).transpose()
    }

    /// Whether anyone can sign in yet.
    ///
    /// A fresh install, and one upgraded from the single-user version, both
    /// have a user row with no password. Until someone sets one, the
    /// interface offers to create the first account rather than a login
    /// form — otherwise an upgrade would lock the owner out of their own
    /// history.
    pub async fn has_any_password(&self) -> Result<bool, Error> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE password_hash IS NOT NULL")
                .fetch_one(self.pool())
                .await?;

        Ok(count > 0)
    }

    /// Create an account.
    pub async fn create_user(&self, username: &str, password: &str) -> Result<User, Error> {
        let username = normalize_username(username)?;
        check_password(password)?;

        if self.user_by_name(&username).await?.is_some() {
            return Err(AccountError::UsernameTaken.into());
        }

        let hash = hash_password(password)?;
        let id: i64 =
            sqlx::query("INSERT INTO users (username, password_hash) VALUES (?, ?) RETURNING id")
                .bind(&username)
                .bind(&hash)
                .fetch_one(self.pool())
                .await?
                .try_get("id")?;

        // Everything downstream assumes these rows exist.
        self.ensure_user_rows(id).await?;

        Ok(User {
            id,
            username,
            has_password: true,
            created_at: Some(Utc::now()),
        })
    }

    /// Give the existing password-less account a password and a name.
    ///
    /// This is how a single-user install becomes a signed-in one: the rows
    /// already belong to user 1, so claiming that account keeps the history
    /// rather than starting an empty second one beside it.
    pub async fn claim_first_user(&self, username: &str, password: &str) -> Result<User, Error> {
        if self.has_any_password().await? {
            // Someone has already set one; this is no longer a fresh
            // install, and letting it through would be a way to take over
            // an existing account.
            return Err(AccountError::UsernameTaken.into());
        }

        let username = normalize_username(username)?;
        check_password(password)?;

        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM users WHERE password_hash IS NULL ORDER BY id ASC")
                .fetch_optional(self.pool())
                .await?;

        let Some(id) = existing else {
            return self.create_user(&username, password).await;
        };

        let hash = hash_password(password)?;
        sqlx::query("UPDATE users SET username = ?, password_hash = ? WHERE id = ?")
            .bind(&username)
            .bind(&hash)
            .bind(id)
            .execute(self.pool())
            .await?;

        self.ensure_user_rows(id).await?;

        Ok(User {
            id,
            username,
            has_password: true,
            created_at: None,
        })
    }

    /// Check a password, returning who it belongs to.
    ///
    /// The same error comes back whether the name is unknown or the password
    /// is wrong, so this cannot be used to find out which accounts exist.
    pub async fn verify_password(&self, username: &str, password: &str) -> Result<User, Error> {
        let row = sqlx::query("SELECT * FROM users WHERE username = ? COLLATE NOCASE")
            .bind(username.trim())
            .fetch_optional(self.pool())
            .await?;

        let Some(row) = row else {
            // Hash anyway, so a missing account does not answer noticeably
            // faster than a wrong password.
            let _ = hash_password(password);
            return Err(AccountError::WrongPassword.into());
        };

        let stored: Option<String> = row.try_get("password_hash")?;
        let Some(stored) = stored else {
            return Err(AccountError::NoPasswordSet.into());
        };

        let parsed =
            PasswordHash::new(&stored).map_err(|_| Error::from(AccountError::WrongPassword))?;

        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .map_err(|_| Error::from(AccountError::WrongPassword))?;

        user_from_row(row)
    }

    pub async fn set_password(&self, user_id: i64, password: &str) -> Result<(), Error> {
        check_password(password)?;

        let hash = hash_password(password)?;
        let affected = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(&hash)
            .bind(user_id)
            .execute(self.pool())
            .await?
            .rows_affected();

        if affected == 0 {
            return Err(AccountError::NoSuchUser.into());
        }
        Ok(())
    }

    /// Change a password, checking the current one first.
    pub async fn change_password(
        &self,
        user_id: i64,
        current: &str,
        new: &str,
    ) -> Result<(), Error> {
        let Some(user) = self.user(user_id).await? else {
            return Err(AccountError::NoSuchUser.into());
        };

        self.verify_password(&user.username, current).await?;
        self.set_password(user_id, new).await
    }

    /// Remove an account and everything belonging to it.
    ///
    /// Refuses to remove the last account: an instance with no accounts
    /// cannot be signed into, and the only way back would be editing the
    /// database by hand.
    pub async fn delete_user(&self, user_id: i64) -> Result<bool, Error> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(self.pool())
            .await?;

        if count <= 1 {
            return Ok(false);
        }

        // Their history, replies, tasks, profile, settings, and sending
        // accounts all cascade from here.
        let affected = sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(user_id)
            .execute(self.pool())
            .await?
            .rows_affected();

        Ok(affected > 0)
    }

    /// Make sure the rows every other query assumes are present.
    async fn ensure_user_rows(&self, user_id: i64) -> Result<(), Error> {
        sqlx::query("INSERT OR IGNORE INTO user_profiles (user_id) VALUES (?)")
            .bind(user_id)
            .execute(self.pool())
            .await?;
        sqlx::query("INSERT OR IGNORE INTO user_settings (user_id) VALUES (?)")
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }
}

fn user_from_row(row: sqlx::sqlite::SqliteRow) -> Result<User, Error> {
    Ok(User {
        id: row.try_get("id")?,
        username: row.try_get("username")?,
        has_password: row.try_get::<Option<String>, _>("password_hash")?.is_some(),
        created_at: row.try_get("created_at")?,
    })
}

/// Trim and check a username.
///
/// Kept to a conservative set because the name appears in the interface and
/// is compared case-insensitively; allowing arbitrary text invites two
/// accounts that look identical.
fn normalize_username(username: &str) -> Result<String, AccountError> {
    let username = username.trim();

    if username.is_empty() {
        return Err(AccountError::MissingUsername);
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err(AccountError::InvalidUsername);
    }

    Ok(username.to_string())
}

fn check_password(password: &str) -> Result<(), AccountError> {
    if password.chars().count() < MINIMUM_PASSWORD_LENGTH {
        return Err(AccountError::PasswordTooShort);
    }
    Ok(())
}

/// Hash a password with Argon2id, which salts it for us.
fn hash_password(password: &str) -> Result<String, Error> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        // Hashing only fails on a parameter mistake, never on the input, so
        // there is nothing here to tell the user apart from "that did not
        // work".
        .map_err(|_| Error::from(AccountError::WrongPassword))
}

#[cfg(test)]
mod tests;
