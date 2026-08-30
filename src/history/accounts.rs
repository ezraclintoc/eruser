//! Per-person settings, and the accounts they send through.
//!
//! New in the Rust version. Upstream had one config file describing one
//! person and one mailbox; these tables let an instance hold several people,
//! each with several sending accounts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use super::{Error, Store};
use crate::config::{Config, EmailConfig, InboxConfig, Options, Profile, SmtpConfig};

/// Who may send through an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountScope {
    /// Only the person who added it.
    Personal,
    /// Anyone on this instance.
    ///
    /// Meant for a household sharing one mailbox. Worth being deliberate
    /// about: it lets someone else put mail into the world over this
    /// address, and broker replies land in the owner's mailbox rather than
    /// the sender's.
    Family,
}

impl AccountScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Family => "family",
        }
    }

    /// Anything unrecognised reads as personal, which is the safer of the
    /// two: a value nobody understands should not widen who can send.
    pub fn from_db(raw: &str) -> Self {
        match raw {
            "family" => Self::Family,
            _ => Self::Personal,
        }
    }

    pub fn is_shared(self) -> bool {
        matches!(self, Self::Family)
    }
}

impl std::fmt::Display for AccountScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One account requests can be sent from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SenderAccount {
    pub id: i64,
    pub user_id: i64,
    /// A name the person recognises, e.g. "personal gmail".
    pub label: String,
    pub scope: AccountScope,
    /// `smtp`, `resend`, or `sendgrid`.
    pub provider: String,
    pub from_address: String,
    pub smtp: SmtpConfig,
    pub api_key: String,
    /// How many this account sends in a day before a run moves on.
    pub daily_limit: i64,
    pub enabled: bool,
    /// Lower goes first, so a free account is spent before a paid one.
    pub priority: i64,
    pub created_at: Option<DateTime<Utc>>,
}

impl SenderAccount {
    /// The email settings needed to build a sender for this account.
    pub fn email_config(&self) -> EmailConfig {
        EmailConfig {
            provider: self.provider.clone(),
            from: self.from_address.clone(),
            smtp: self.smtp.clone(),
            resend: crate::config::ApiKeyConfig {
                api_key: self.api_key.clone(),
            },
            sendgrid: crate::config::ApiKeyConfig {
                api_key: self.api_key.clone(),
            },
        }
    }

    /// What to call this account in the interface.
    pub fn display_name(&self) -> String {
        if self.label.is_empty() {
            self.from_address.clone()
        } else {
            format!("{} ({})", self.label, self.from_address)
        }
    }
}

/// A new account, before it has an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSenderAccount {
    pub user_id: i64,
    pub label: String,
    pub scope: AccountScope,
    pub provider: String,
    pub from_address: String,
    pub smtp: SmtpConfig,
    pub api_key: String,
    pub daily_limit: i64,
    pub enabled: bool,
    pub priority: i64,
}

/// Gmail stops accepting around 500 a day and counts failures toward it;
/// 250 leaves room for whatever else the account sends.
pub const DEFAULT_DAILY_LIMIT: i64 = 250;

impl Default for NewSenderAccount {
    fn default() -> Self {
        Self {
            user_id: super::DEFAULT_USER_ID,
            label: String::new(),
            // Not shared unless someone says so.
            scope: AccountScope::Personal,
            provider: "smtp".to_string(),
            from_address: String::new(),
            smtp: SmtpConfig::default(),
            api_key: String::new(),
            daily_limit: DEFAULT_DAILY_LIMIT,
            enabled: true,
            priority: 0,
        }
    }
}

/// An account and how much of its allowance is left today.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountCapacity {
    pub account: SenderAccount,
    /// Sent through this account since midnight, local time.
    pub sent_today: i64,
    /// What is left before the run moves on.
    pub remaining: i64,
}

impl AccountCapacity {
    pub fn is_available(&self) -> bool {
        self.account.enabled && self.remaining > 0
    }
}

impl Store {
    // ---------------------------------------------------------------
    // Profile
    // ---------------------------------------------------------------

    pub async fn profile(&self, user_id: i64) -> Result<Profile, Error> {
        let row = sqlx::query("SELECT * FROM user_profiles WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(self.pool())
            .await?;

        let Some(row) = row else {
            return Ok(Profile::default());
        };

        Ok(Profile {
            first_name: row.try_get("first_name")?,
            last_name: row.try_get("last_name")?,
            email: row.try_get("email")?,
            address: row.try_get("address")?,
            city: row.try_get("city")?,
            state: row.try_get("state")?,
            zip_code: row.try_get("zip_code")?,
            country: row.try_get("country")?,
            phone: row.try_get("phone")?,
            date_of_birth: row.try_get("date_of_birth")?,
        })
    }

    pub async fn save_profile(&self, user_id: i64, profile: &Profile) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO user_profiles
                 (user_id, first_name, last_name, email, address, city, state,
                  zip_code, country, phone, date_of_birth, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET
                 first_name = excluded.first_name,
                 last_name  = excluded.last_name,
                 email      = excluded.email,
                 address    = excluded.address,
                 city       = excluded.city,
                 state      = excluded.state,
                 zip_code   = excluded.zip_code,
                 country    = excluded.country,
                 phone      = excluded.phone,
                 date_of_birth = excluded.date_of_birth,
                 updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(&profile.first_name)
        .bind(&profile.last_name)
        .bind(&profile.email)
        .bind(&profile.address)
        .bind(&profile.city)
        .bind(&profile.state)
        .bind(&profile.zip_code)
        .bind(&profile.country)
        .bind(&profile.phone)
        .bind(&profile.date_of_birth)
        .bind(Utc::now())
        .execute(self.pool())
        .await?;

        Ok(())
    }

    // ---------------------------------------------------------------
    // Settings
    // ---------------------------------------------------------------

    pub async fn settings(&self, user_id: i64) -> Result<(Options, InboxConfig), Error> {
        let row = sqlx::query("SELECT * FROM user_settings WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(self.pool())
            .await?;

        let Some(row) = row else {
            return Ok((Options::default(), InboxConfig::default()));
        };

        let options = Options {
            template: row.try_get("template")?,
            dry_run: false,
            rate_limit_ms: row.try_get::<i64, _>("rate_limit_ms")?.max(0) as u64,
            regions: json_list(&row.try_get::<String, _>("regions")?),
            excluded_brokers: json_list(&row.try_get::<String, _>("excluded_brokers")?),
        };

        let mut inbox = InboxConfig {
            enabled: row.try_get::<i64, _>("inbox_enabled")? != 0,
            provider: row.try_get("inbox_provider")?,
            server: row.try_get("inbox_server")?,
            port: row
                .try_get::<i64, _>("inbox_port")?
                .clamp(0, i64::from(u16::MAX)) as u16,
            email: row.try_get("inbox_email")?,
            password: row.try_get("inbox_password")?,
            folder: row.try_get("inbox_folder")?,
            ..Default::default()
        };
        if inbox.folder.is_empty() {
            inbox.folder = "INBOX".to_string();
        }

        Ok((options, inbox))
    }

    pub async fn save_settings(
        &self,
        user_id: i64,
        options: &Options,
        inbox: &InboxConfig,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO user_settings
                 (user_id, template, rate_limit_ms, regions, excluded_brokers,
                  inbox_enabled, inbox_provider, inbox_server, inbox_port,
                  inbox_email, inbox_password, inbox_folder, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET
                 template         = excluded.template,
                 rate_limit_ms    = excluded.rate_limit_ms,
                 regions          = excluded.regions,
                 excluded_brokers = excluded.excluded_brokers,
                 inbox_enabled    = excluded.inbox_enabled,
                 inbox_provider   = excluded.inbox_provider,
                 inbox_server     = excluded.inbox_server,
                 inbox_port       = excluded.inbox_port,
                 inbox_email      = excluded.inbox_email,
                 inbox_password   = excluded.inbox_password,
                 inbox_folder     = excluded.inbox_folder,
                 updated_at       = excluded.updated_at",
        )
        .bind(user_id)
        .bind(&options.template)
        .bind(options.rate_limit_ms as i64)
        .bind(to_json_list(&options.regions))
        .bind(to_json_list(&options.excluded_brokers))
        .bind(inbox.enabled)
        .bind(&inbox.provider)
        .bind(&inbox.server)
        .bind(i64::from(inbox.port))
        .bind(&inbox.email)
        .bind(&inbox.password)
        .bind(&inbox.folder)
        .bind(Utc::now())
        .execute(self.pool())
        .await?;

        Ok(())
    }

    // ---------------------------------------------------------------
    // Sending accounts
    // ---------------------------------------------------------------

    pub async fn add_sender_account(&self, account: &NewSenderAccount) -> Result<i64, Error> {
        let id = sqlx::query(
            "INSERT INTO sender_accounts
                 (user_id, label, scope, provider, from_address, smtp_host, smtp_port,
                  smtp_username, smtp_password, smtp_use_tls, api_key,
                  daily_limit, enabled, priority)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id, from_address) DO UPDATE SET
                 label         = excluded.label,
                 scope         = excluded.scope,
                 provider      = excluded.provider,
                 smtp_host     = excluded.smtp_host,
                 smtp_port     = excluded.smtp_port,
                 smtp_username = excluded.smtp_username,
                 smtp_password = excluded.smtp_password,
                 smtp_use_tls  = excluded.smtp_use_tls,
                 api_key       = excluded.api_key,
                 daily_limit   = excluded.daily_limit,
                 enabled       = excluded.enabled,
                 priority      = excluded.priority
             RETURNING id",
        )
        .bind(account.user_id)
        .bind(&account.label)
        .bind(account.scope.as_str())
        .bind(&account.provider)
        .bind(&account.from_address)
        .bind(&account.smtp.host)
        .bind(i64::from(account.smtp.port))
        .bind(&account.smtp.username)
        .bind(&account.smtp.password)
        .bind(account.smtp.use_tls)
        .bind(&account.api_key)
        .bind(account.daily_limit)
        .bind(account.enabled)
        .bind(account.priority)
        .fetch_one(self.pool())
        .await?
        .try_get("id")?;

        Ok(id)
    }

    /// Every account a user may send through: their own, plus any that
    /// someone on this instance has shared with the household.
    ///
    /// Ordered the way a run would use them.
    pub async fn usable_sender_accounts(&self, user_id: i64) -> Result<Vec<SenderAccount>, Error> {
        let rows = sqlx::query(
            "SELECT * FROM sender_accounts
             WHERE user_id = ? OR scope = 'family'
             ORDER BY priority ASC, id ASC",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;

        rows.into_iter().map(account_from_row).collect()
    }

    /// Every account a user owns, in the order a run would use them.
    ///
    /// This is the list for managing accounts; [`Self::usable_sender_accounts`]
    /// is the list for sending, which also includes shared ones.
    pub async fn sender_accounts(&self, user_id: i64) -> Result<Vec<SenderAccount>, Error> {
        let rows = sqlx::query(
            "SELECT * FROM sender_accounts
             WHERE user_id = ?
             ORDER BY priority ASC, id ASC",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;

        rows.into_iter().map(account_from_row).collect()
    }

    pub async fn sender_account(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<SenderAccount>, Error> {
        let row = sqlx::query("SELECT * FROM sender_accounts WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(id)
            .fetch_optional(self.pool())
            .await?;

        row.map(account_from_row).transpose()
    }

    /// Remove an account. Returns false if there was no such account.
    ///
    /// History keeps pointing at the id, so what was already sent through it
    /// stays attributed rather than silently moving to another account.
    pub async fn delete_sender_account(&self, user_id: i64, id: i64) -> Result<bool, Error> {
        let affected = sqlx::query("DELETE FROM sender_accounts WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(id)
            .execute(self.pool())
            .await?
            .rows_affected();

        Ok(affected > 0)
    }

    pub async fn set_sender_account_enabled(
        &self,
        user_id: i64,
        id: i64,
        enabled: bool,
    ) -> Result<bool, Error> {
        let affected =
            sqlx::query("UPDATE sender_accounts SET enabled = ? WHERE user_id = ? AND id = ?")
                .bind(enabled)
                .bind(user_id)
                .bind(id)
                .execute(self.pool())
                .await?
                .rows_affected();

        Ok(affected > 0)
    }

    /// Every account with how much of today's allowance it has left.
    ///
    /// The count is per account rather than per person, which is the whole
    /// point: three accounts sharing one cap would never roll over.
    pub async fn account_capacity(&self, user_id: i64) -> Result<Vec<AccountCapacity>, Error> {
        let accounts = self.usable_sender_accounts(user_id).await?;
        if accounts.is_empty() {
            return Ok(Vec::new());
        }

        // Counted per account across everyone, not per person. A shared
        // mailbox has one allowance however many people send through it, and
        // counting per person would let a household blow through the
        // provider's cap and get the address rate limited.
        let since = start_of_today();
        let rows = sqlx::query(
            "SELECT sender_account_id, COUNT(*) AS sent
             FROM removal_requests
             WHERE sent_at >= ? AND sender_account_id IS NOT NULL
             GROUP BY sender_account_id",
        )
        .bind(since)
        .fetch_all(self.pool())
        .await?;

        let mut sent_by_account = std::collections::HashMap::new();
        for row in rows {
            let id: i64 = row.try_get("sender_account_id")?;
            let sent: i64 = row.try_get("sent")?;
            sent_by_account.insert(id, sent);
        }

        Ok(accounts
            .into_iter()
            .map(|account| {
                let sent_today = sent_by_account.get(&account.id).copied().unwrap_or(0);
                AccountCapacity {
                    remaining: (account.daily_limit - sent_today).max(0),
                    sent_today,
                    account,
                }
            })
            .collect())
    }

    /// What a user could send right now, across every account they have.
    pub async fn remaining_capacity_today(&self, user_id: i64) -> Result<i64, Error> {
        Ok(self
            .account_capacity(user_id)
            .await?
            .iter()
            .filter(|capacity| capacity.is_available())
            .map(|capacity| capacity.remaining)
            .sum())
    }

    // ---------------------------------------------------------------
    // Importing the config file
    // ---------------------------------------------------------------

    /// Move a single-user `config.yaml` into the database.
    ///
    /// This is how an existing install upgrades. It runs once: if the user
    /// already has a name recorded, the file is left alone, so editing the
    /// database does not get undone by a stale file on the next start.
    pub async fn import_config(&self, user_id: i64, config: &Config) -> Result<bool, Error> {
        let existing = self.profile(user_id).await?;
        if !existing.first_name.is_empty() || !existing.email.is_empty() {
            return Ok(false);
        }

        self.save_profile(user_id, &config.profile).await?;
        self.save_settings(user_id, &config.options, &config.inbox)
            .await?;

        // Only worth creating an account if the file actually described one.
        if !config.email.from.is_empty() && config.validate().is_ok() {
            self.add_sender_account(&NewSenderAccount {
                user_id,
                label: "imported".to_string(),
                provider: config.email.provider.clone(),
                from_address: config.email.from.clone(),
                smtp: config.email.smtp.clone(),
                api_key: match config.email.provider.as_str() {
                    "resend" => config.email.resend.api_key.clone(),
                    "sendgrid" => config.email.sendgrid.api_key.clone(),
                    _ => String::new(),
                },
                ..Default::default()
            })
            .await?;
        }

        tracing::info!(user_id, "imported config.yaml into the database");
        Ok(true)
    }

    /// Rebuild a `Config` from the database, for the code that still wants one.
    pub async fn config_for(&self, user_id: i64) -> Result<Config, Error> {
        let (options, inbox) = self.settings(user_id).await?;
        let accounts = self.usable_sender_accounts(user_id).await?;

        let email = accounts
            .iter()
            .find(|account| account.enabled)
            .map(SenderAccount::email_config)
            .unwrap_or_default();

        Ok(Config {
            profile: self.profile(user_id).await?,
            email,
            options,
            inbox,
            pipeline: crate::config::Pipeline::default(),
        })
    }
}

/// Midnight this morning, local time, as an instant.
fn start_of_today() -> DateTime<Utc> {
    use chrono::{Local, TimeZone};

    let now = Local::now();
    Local
        .with_ymd_and_hms(
            now.year_naive(),
            now.month_naive(),
            now.day_naive(),
            0,
            0,
            0,
        )
        .single()
        .map(|start| start.with_timezone(&Utc))
        // A DST change at local midnight can make that instant ambiguous;
        // counting from 24 hours ago is close enough to be useful.
        .unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24))
}

/// Chrono's Datelike under a name that does not collide with the columns.
trait LocalParts {
    fn year_naive(&self) -> i32;
    fn month_naive(&self) -> u32;
    fn day_naive(&self) -> u32;
}

impl LocalParts for chrono::DateTime<chrono::Local> {
    fn year_naive(&self) -> i32 {
        chrono::Datelike::year(self)
    }
    fn month_naive(&self) -> u32 {
        chrono::Datelike::month(self)
    }
    fn day_naive(&self) -> u32 {
        chrono::Datelike::day(self)
    }
}

fn account_from_row(row: sqlx::sqlite::SqliteRow) -> Result<SenderAccount, Error> {
    Ok(SenderAccount {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        label: row.try_get("label")?,
        scope: AccountScope::from_db(&row.try_get::<String, _>("scope")?),
        provider: row.try_get("provider")?,
        from_address: row.try_get("from_address")?,
        smtp: SmtpConfig {
            host: row.try_get("smtp_host")?,
            port: row
                .try_get::<i64, _>("smtp_port")?
                .clamp(0, i64::from(u16::MAX)) as u16,
            username: row.try_get("smtp_username")?,
            password: row.try_get("smtp_password")?,
            use_tls: row.try_get::<i64, _>("smtp_use_tls")? != 0,
        },
        api_key: row.try_get("api_key")?,
        daily_limit: row.try_get("daily_limit")?,
        enabled: row.try_get::<i64, _>("enabled")? != 0,
        priority: row.try_get("priority")?,
        created_at: row.try_get("created_at")?,
    })
}

/// A JSON array of strings, tolerating anything unreadable as empty.
fn json_list(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn to_json_list(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests;
