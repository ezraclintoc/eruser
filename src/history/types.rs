//! Row types and the string-backed enums stored alongside them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Rows created before authentication exists all belong to this user.
pub const DEFAULT_USER_ID: i64 = 1;

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $fallback:ident {
            $($variant:ident => $text:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant,)+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = super::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($text => Ok(Self::$variant),)+
                    other => Err(super::Error::UnknownEnumValue {
                        kind: stringify!($name),
                        value: other.to_string(),
                    }),
                }
            }
        }

        impl $name {
            /// Parse a value read back from the database, falling back rather
            /// than failing. A row written by a newer version of eruser
            /// should not make an older one unable to list history.
            pub fn from_db(s: &str) -> Self {
                s.parse().unwrap_or(Self::$fallback)
            }
        }
    };
}

string_enum! {
    /// Delivery outcome of a single removal request.
    Status, Failed {
        Sent => "sent",
        Failed => "failed",
        Pending => "pending",
    }
}

string_enum! {
    /// Where a broker sits in the removal pipeline.
    PipelineStatus, EmailSent {
        EmailSent => "email_sent",
        AwaitingResponse => "awaiting_response",
        FormRequired => "form_required",
        FormFilled => "form_filled",
        AwaitingCaptcha => "awaiting_captcha",
        CaptchaSolved => "captcha_solved",
        AwaitingConfirmation => "awaiting_confirmation",
        Confirmed => "confirmed",
        Failed => "failed",
        Rejected => "rejected",
    }
}

string_enum! {
    /// What kind of human intervention a pending task needs.
    TaskType, Review {
        Captcha => "captcha",
        ManualForm => "manual_form",
        Review => "review",
        Confirm => "confirm",
    }
}

string_enum! {
    /// Lifecycle of a pending task.
    TaskStatus, Pending {
        Pending => "pending",
        Completed => "completed",
        Skipped => "skipped",
    }
}

string_enum! {
    /// How the classifier read a broker's reply.
    ResponseType, Unknown {
        FormRequired => "form_required",
        ConfirmationRequired => "confirmation_required",
        Success => "success",
        Rejected => "rejected",
        Bounced => "bounced",
        Pending => "pending",
        Unknown => "unknown",
    }
}

string_enum! {
    /// Where an opt-out form stands, derived from tasks and pipeline state.
    FormStatus, Pending {
        Pending => "pending",
        Filled => "filled",
        Captcha => "captcha",
        Failed => "failed",
        Skipped => "skipped",
    }
}

/// One removal request that was attempted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: i64,
    pub user_id: i64,
    pub sender_account_id: Option<i64>,
    pub broker_id: String,
    pub broker_name: String,
    pub email: String,
    pub template: String,
    pub status: Status,
    pub message_id: String,
    pub error: String,
    pub sent_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub pipeline_status: PipelineStatus,
}

/// The fields needed to record a new request; the rest are assigned on insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRecord {
    pub user_id: i64,
    /// Which sending account carried it, when one is known.
    ///
    /// This is what makes the daily count per account rather than per
    /// person, so a run can roll over when one mailbox is spent.
    pub sender_account_id: Option<i64>,
    pub broker_id: String,
    pub broker_name: String,
    pub email: String,
    pub template: String,
    pub status: Status,
    pub message_id: String,
    pub error: String,
    pub sent_at: Option<DateTime<Utc>>,
}

impl NewRecord {
    /// A successful send.
    pub fn sent(
        broker_id: &str,
        broker_name: &str,
        email: &str,
        template: &str,
        message_id: &str,
    ) -> Self {
        Self {
            user_id: DEFAULT_USER_ID,
            broker_id: broker_id.to_string(),
            broker_name: broker_name.to_string(),
            email: email.to_string(),
            template: template.to_string(),
            sender_account_id: None,
            status: Status::Sent,
            message_id: message_id.to_string(),
            error: String::new(),
            sent_at: Some(Utc::now()),
        }
    }

    /// A send that failed. `error` is shown to the user, so it must already
    /// be free of credentials.
    pub fn failed(
        broker_id: &str,
        broker_name: &str,
        email: &str,
        template: &str,
        error: &str,
    ) -> Self {
        Self {
            user_id: DEFAULT_USER_ID,
            broker_id: broker_id.to_string(),
            broker_name: broker_name.to_string(),
            email: email.to_string(),
            template: template.to_string(),
            sender_account_id: None,
            status: Status::Failed,
            message_id: String::new(),
            error: error.to_string(),
            sent_at: Some(Utc::now()),
        }
    }

    pub fn for_user(mut self, user_id: i64) -> Self {
        self.user_id = user_id;
        self
    }

    /// Attribute the send to a particular account.
    pub fn through_account(mut self, account_id: Option<i64>) -> Self {
        self.sender_account_id = account_id;
        self
    }
}

/// A classified reply from a broker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrokerResponse {
    pub id: i64,
    pub user_id: i64,
    pub broker_id: String,
    pub broker_name: String,
    pub response_type: ResponseType,
    pub email_from: String,
    pub email_subject: String,
    pub email_body: String,
    pub form_url: String,
    pub confirm_url: String,
    pub confidence: f64,
    pub needs_review: bool,
    pub received_at: Option<DateTime<Utc>>,
    pub processed_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
}

/// A reply to store, before it has an id.
#[derive(Debug, Clone, PartialEq)]
pub struct NewBrokerResponse {
    pub user_id: i64,
    pub broker_id: String,
    pub broker_name: String,
    pub response_type: ResponseType,
    pub email_from: String,
    pub email_subject: String,
    pub email_body: String,
    pub form_url: String,
    pub confirm_url: String,
    pub confidence: f64,
    pub needs_review: bool,
    pub received_at: Option<DateTime<Utc>>,
}

impl Default for NewBrokerResponse {
    fn default() -> Self {
        Self {
            user_id: DEFAULT_USER_ID,
            broker_id: String::new(),
            broker_name: String::new(),
            response_type: ResponseType::Unknown,
            email_from: String::new(),
            email_subject: String::new(),
            email_body: String::new(),
            form_url: String::new(),
            confirm_url: String::new(),
            confidence: 0.0,
            needs_review: false,
            received_at: None,
        }
    }
}

/// Work waiting on a human.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTask {
    pub id: i64,
    pub user_id: i64,
    pub broker_id: String,
    pub broker_name: String,
    pub task_type: TaskType,
    pub form_url: String,
    pub screenshot_path: String,
    pub browser_state: String,
    pub notes: String,
    pub status: TaskStatus,
    pub created_at: Option<DateTime<Utc>>,
    /// When the user first opened the helper page for this task.
    pub opened_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// A task to create, before it has an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPendingTask {
    pub user_id: i64,
    pub broker_id: String,
    pub broker_name: String,
    pub task_type: TaskType,
    pub form_url: String,
    pub screenshot_path: String,
    pub browser_state: String,
    pub notes: String,
}

impl Default for NewPendingTask {
    fn default() -> Self {
        Self {
            user_id: DEFAULT_USER_ID,
            broker_id: String::new(),
            broker_name: String::new(),
            task_type: TaskType::Review,
            form_url: String::new(),
            screenshot_path: String::new(),
            browser_state: String::new(),
            notes: String::new(),
        }
    }
}

/// Per-broker summary of what has been sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerStatus {
    pub broker_id: String,
    pub last_sent: Option<DateTime<Utc>>,
    pub status: Status,
    pub total_sent: i64,
}

/// A form detected in a broker reply, with where it currently stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormWithStatus {
    pub broker_id: String,
    pub broker_name: String,
    pub form_url: String,
    pub email_subject: String,
    pub detected_at: Option<DateTime<Utc>>,
    pub status: FormStatus,
    /// 0 when no pending task exists for this broker.
    pub task_id: i64,
    pub pipeline_status: PipelineStatus,
}

/// Aggregate counts for the dashboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub total: i64,
    pub sent: i64,
    pub failed: i64,
}

/// Counts of pending tasks by lifecycle state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStats {
    pub pending: i64,
    pub completed: i64,
    pub skipped: i64,
}

/// Counts of detected forms by state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormStats {
    pub pending: i64,
    pub filled: i64,
    pub captcha: i64,
    pub failed: i64,
    pub skipped: i64,
}

/// Filter for listing broker responses.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResponseFilter {
    pub response_type: Option<ResponseType>,
    pub needs_review: bool,
    pub limit: Option<i64>,
}

/// Filter for listing pending tasks.
#[derive(Debug, Clone, Copy, Default)]
pub struct TaskFilter {
    pub task_type: Option<TaskType>,
    pub status: Option<TaskStatus>,
}
