//! User configuration: profile, email transport, options, inbox, pipeline.
//!
//! Ported from `internal/config/config.go`. The on-disk YAML schema is
//! unchanged, so an existing `~/.eraser/config.yaml` loads as-is.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod error;
pub use error::{Error, ValidationError};

pub const DEFAULT_RATE_LIMIT_MS: u64 = 2000;
pub const DEFAULT_TEMPLATE: &str = "generic";
pub const DEFAULT_BROWSER_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub profile: Profile,
    #[serde(default)]
    pub email: EmailConfig,
    #[serde(default)]
    pub options: Options,
    #[serde(default)]
    pub inbox: InboxConfig,
    #[serde(default)]
    pub pipeline: Pipeline,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub address: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub city: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub zip_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub country: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub phone: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub date_of_birth: String,
}

impl Profile {
    pub fn full_name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailConfig {
    /// `smtp`, `resend`, or `sendgrid`.
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub smtp: SmtpConfig,
    #[serde(default, skip_serializing_if = "ApiKeyConfig::is_empty")]
    pub resend: ApiKeyConfig,
    #[serde(default, skip_serializing_if = "ApiKeyConfig::is_empty")]
    pub sendgrid: ApiKeyConfig,
}

/// Credentials for a provider that takes a single API key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeyConfig {
    #[serde(default)]
    pub api_key: String,
}

impl ApiKeyConfig {
    pub fn is_empty(&self) -> bool {
        self.api_key.is_empty()
    }
}

/// The providers eruser can send through.
pub const EMAIL_PROVIDERS: &[&str] = &["smtp", "resend", "sendgrid"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmtpConfig {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub use_tls: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Options {
    #[serde(default = "default_template")]
    pub template: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default = "default_rate_limit_ms")]
    pub rate_limit_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_brokers: Vec<String>,
}

fn default_template() -> String {
    DEFAULT_TEMPLATE.to_string()
}

fn default_rate_limit_ms() -> u64 {
    DEFAULT_RATE_LIMIT_MS
}

impl Default for Options {
    fn default() -> Self {
        Self {
            template: default_template(),
            dry_run: false,
            rate_limit_ms: DEFAULT_RATE_LIMIT_MS,
            regions: Vec::new(),
            excluded_brokers: Vec::new(),
        }
    }
}

/// IMAP settings for monitoring broker responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxConfig {
    #[serde(default)]
    pub enabled: bool,
    /// `gmail`, `outlook`, or `imap`.
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub email: String,
    /// App password. Never the account's main password.
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_folder")]
    pub folder: String,
    #[serde(default)]
    pub auto_archive: bool,
    #[serde(default = "default_archive_folder")]
    pub archive_folder: String,
}

fn default_folder() -> String {
    "INBOX".to_string()
}

fn default_archive_folder() -> String {
    "Eraser".to_string()
}

impl Default for InboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: String::new(),
            server: String::new(),
            port: 0,
            email: String::new(),
            password: String::new(),
            folder: default_folder(),
            auto_archive: false,
            archive_folder: default_archive_folder(),
        }
    }
}

/// Settings for the automation pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pipeline {
    /// Auto-click confirmation links found in broker replies.
    #[serde(default)]
    pub auto_confirm: bool,
    /// Drive a browser to fill opt-out forms.
    #[serde(default)]
    pub auto_fill_forms: bool,
    #[serde(default = "default_true")]
    pub browser_headless: bool,
    #[serde(default = "default_browser_timeout")]
    pub browser_timeout_sec: u64,
}

fn default_true() -> bool {
    true
}

fn default_browser_timeout() -> u64 {
    DEFAULT_BROWSER_TIMEOUT_SECS
}

impl Default for Pipeline {
    fn default() -> Self {
        Self {
            auto_confirm: false,
            auto_fill_forms: false,
            browser_headless: true,
            browser_timeout_sec: DEFAULT_BROWSER_TIMEOUT_SECS,
        }
    }
}

/// `~/.eraser/config.yaml`, or `config.yaml` in the working directory if the
/// home directory cannot be determined.
pub fn default_config_path() -> PathBuf {
    match home_dir() {
        Some(home) => home.join(".eraser").join("config.yaml"),
        None => PathBuf::from("config.yaml"),
    }
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

impl Config {
    /// Read and parse a config file, filling in defaults.
    ///
    /// Returns [`Error::InsecurePermissions`] when the file is group- or
    /// world-accessible. The Go original only printed a warning; the file
    /// holds an email password and a home address, so here it is fatal by
    /// default. Callers that want the lenient behaviour can use
    /// [`Config::load_lenient`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        check_permissions(path)?;
        Self::parse_file(path)
    }

    /// Like [`Config::load`], but downgrades an insecure-permissions failure
    /// to a returned warning instead of an error.
    pub fn load_lenient(path: impl AsRef<Path>) -> Result<(Self, Option<Error>), Error> {
        let path = path.as_ref();
        let warning = check_permissions(path).err();
        Ok((Self::parse_file(path)?, warning))
    }

    fn parse_file(path: &Path) -> Result<Self, Error> {
        let data = std::fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut cfg: Config = serde_norway::from_str(&data).map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        cfg.apply_defaults();
        Ok(cfg)
    }

    /// Fill in values that are meaningful to omit from the file.
    ///
    /// Most defaults come from serde, but the IMAP server/port pair is
    /// derived from the provider name and so cannot be a field default.
    pub fn apply_defaults(&mut self) {
        if self.options.template.is_empty() {
            self.options.template = DEFAULT_TEMPLATE.to_string();
        }
        if self.options.rate_limit_ms == 0 {
            self.options.rate_limit_ms = DEFAULT_RATE_LIMIT_MS;
        }
        if self.inbox.folder.is_empty() {
            self.inbox.folder = default_folder();
        }
        if self.inbox.archive_folder.is_empty() {
            self.inbox.archive_folder = default_archive_folder();
        }
        if self.inbox.server.is_empty() {
            match self.inbox.provider.as_str() {
                "gmail" => {
                    self.inbox.server = "imap.gmail.com".to_string();
                    self.inbox.port = 993;
                }
                "outlook" => {
                    self.inbox.server = "outlook.office365.com".to_string();
                    self.inbox.port = 993;
                }
                _ => {}
            }
        }
        if self.pipeline.browser_timeout_sec == 0 {
            self.pipeline.browser_timeout_sec = DEFAULT_BROWSER_TIMEOUT_SECS;
        }
    }

    /// Serialize to `path`, creating parent directories, owner-only.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| Error::Write {
                path: parent.to_path_buf(),
                source,
            })?;
            restrict_dir_permissions(parent)?;
        }
        let data = serde_norway::to_string(self).map_err(Error::Serialize)?;
        write_owner_only(path, &data)
    }

    /// Check that the config is complete enough to send with.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.profile.first_name.is_empty() || self.profile.last_name.is_empty() {
            return Err(ValidationError::MissingName);
        }
        if self.profile.email.is_empty() {
            return Err(ValidationError::MissingProfileEmail);
        }
        if self.email.provider.is_empty() {
            return Err(ValidationError::MissingProvider);
        }
        if self.email.from.is_empty() {
            return Err(ValidationError::MissingFrom);
        }
        // Each provider needs different things, and a key for one must not
        // excuse missing settings for another.
        match self.email.provider.as_str() {
            "smtp" => {
                if self.email.smtp.host.is_empty() {
                    return Err(ValidationError::MissingSmtpHost);
                }
                if self.email.smtp.port == 0 {
                    return Err(ValidationError::MissingSmtpPort);
                }
            }
            "resend" if self.email.resend.api_key.is_empty() => {
                return Err(ValidationError::MissingApiKey("resend"));
            }
            "sendgrid" if self.email.sendgrid.api_key.is_empty() => {
                return Err(ValidationError::MissingApiKey("sendgrid"));
            }
            "resend" | "sendgrid" => {}
            other => return Err(ValidationError::UnknownProvider(other.to_string())),
        }
        Ok(())
    }

    /// Check that inbox monitoring is configured. Only called by the commands
    /// that actually connect to IMAP.
    pub fn validate_inbox(&self) -> Result<(), ValidationError> {
        self.inbox.validate()
    }
}

impl InboxConfig {
    /// Check that this is complete enough to connect with.
    ///
    /// Lives on the settings rather than on `Config` so the monitor, which
    /// only ever holds this half, can check its own inputs.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !self.enabled {
            return Err(ValidationError::InboxDisabled);
        }
        if self.email.is_empty() {
            return Err(ValidationError::MissingInboxEmail);
        }
        if self.password.is_empty() {
            return Err(ValidationError::MissingInboxPassword);
        }
        if self.server.is_empty() {
            return Err(ValidationError::MissingInboxServer);
        }
        if self.port == 0 {
            return Err(ValidationError::MissingInboxPort);
        }
        Ok(())
    }
}

#[cfg(unix)]
fn check_permissions(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;

    let meta = std::fs::metadata(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(Error::InsecurePermissions {
            path: path.to_path_buf(),
            mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_permissions(_path: &Path) -> Result<(), Error> {
    Ok(())
}

#[cfg(unix)]
fn write_owner_only(path: &Path, data: &str) -> Result<(), Error> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    // Create with 0600 from the start rather than chmod-ing afterwards, so
    // the secrets are never briefly readable by other users.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| Error::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(data.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|source| Error::Write {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, data: &str) -> Result<(), Error> {
    std::fs::write(path, data).map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn restrict_dir_permissions(dir: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|source| {
        Error::Write {
            path: dir.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn restrict_dir_permissions(_dir: &Path) -> Result<(), Error> {
    Ok(())
}

#[cfg(test)]
mod tests;
