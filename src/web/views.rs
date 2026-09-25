//! View models: the shapes handed to templates.
//!
//! Go passed `map[string]interface{}` literals, so a typo in a key was a
//! blank spot on the page and nothing else. These are structs, and the field
//! names are what the templates read.

use serde::Serialize;

use crate::broker::Broker;
use crate::history::{self, BrokerStatus, PipelineStatus, Record, ResponseType};

/// Headline counts for the dashboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Stats {
    pub total_brokers: usize,
    pub sent: i64,
    pub failed: i64,
    /// Brokers not yet contacted.
    pub pending: i64,
}

impl Stats {
    pub fn new(total_brokers: usize, history: history::Stats) -> Self {
        // Saturating: history can outnumber the database if brokers were
        // removed from brokers.yaml after being contacted.
        let pending = (total_brokers as i64)
            .saturating_sub(history.sent)
            .saturating_sub(history.failed)
            .max(0);

        Self {
            total_brokers,
            sent: history.sent,
            failed: history.failed,
            pending,
        }
    }
}

/// Where one broker stands, as shown in the broker table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrokerWithStatus {
    // Flattened so templates read `item.name` rather than `item.broker.name`,
    // matching Go's embedded struct.
    #[serde(flatten)]
    pub broker: Broker,
    /// `never`, `sent`, or `failed`.
    pub status: &'static str,
    /// Preformatted, or empty when never contacted.
    pub last_sent: String,
    pub total_sent: i64,
}

impl BrokerWithStatus {
    pub fn new(broker: Broker, status: Option<&BrokerStatus>) -> Self {
        let Some(status) = status else {
            return Self {
                broker,
                status: "never",
                last_sent: String::new(),
                total_sent: 0,
            };
        };

        Self {
            broker,
            status: match status.status {
                history::Status::Sent => "sent",
                history::Status::Failed => "failed",
                history::Status::Pending => "pending",
            },
            last_sent: status
                .last_sent
                .map(|time| {
                    time.with_timezone(&chrono::Local)
                        .format("%b %-d, %Y")
                        .to_string()
                })
                .unwrap_or_default(),
            total_sent: status.total_sent,
        }
    }

    /// Whether this row passes the filters from the query string.
    pub fn matches(&self, filters: &BrokerFilters) -> bool {
        if !filters.search.is_empty() {
            let needle = filters.search.to_lowercase();
            let name = self.broker.name.to_lowercase();
            let email = self.broker.email.to_lowercase();
            if !name.contains(&needle) && !email.contains(&needle) {
                return false;
            }
        }
        if !filters.category.is_empty()
            && !self.broker.category.eq_ignore_ascii_case(&filters.category)
        {
            return false;
        }
        if !filters.region.is_empty() && !self.broker.region.eq_ignore_ascii_case(&filters.region) {
            return false;
        }

        match filters.status.to_lowercase().as_str() {
            "" => true,
            // "pending" on this page means "not yet contacted".
            "pending" => self.status == "never",
            wanted => self.status == wanted,
        }
    }
}

/// The filters the broker page accepts, straight off the query string.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct BrokerFilters {
    pub search: String,
    pub category: String,
    pub region: String,
    pub status: String,
}

impl BrokerFilters {
    /// Trim surrounding space, so " US " and "us" filter the same way.
    pub fn normalized(mut self) -> Self {
        self.search = self.search.trim().to_string();
        self.category = self.category.trim().to_string();
        self.region = self.region.trim().to_string();
        self.status = self.status.trim().to_string();
        self
    }
}

/// Counts per pipeline stage, plus the derived "needs you" total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PipelineStats {
    pub email_sent: i64,
    pub awaiting_response: i64,
    pub form_required: i64,
    pub form_filled: i64,
    pub awaiting_captcha: i64,
    pub captcha_solved: i64,
    pub awaiting_confirmation: i64,
    pub confirmed: i64,
    pub rejected: i64,
    pub failed: i64,
    /// Everything waiting on a person: pending forms, open tasks, and
    /// replies the classifier was unsure about.
    pub pending_tasks: i64,
    pub needs_review: i64,
}

impl PipelineStats {
    pub fn new(
        stages: &std::collections::HashMap<PipelineStatus, i64>,
        open_tasks: i64,
        pending_forms: i64,
        needs_review: i64,
    ) -> Self {
        let stage = |status: PipelineStatus| stages.get(&status).copied().unwrap_or(0);

        Self {
            email_sent: stage(PipelineStatus::EmailSent),
            awaiting_response: stage(PipelineStatus::AwaitingResponse),
            form_required: stage(PipelineStatus::FormRequired),
            form_filled: stage(PipelineStatus::FormFilled),
            awaiting_captcha: stage(PipelineStatus::AwaitingCaptcha),
            captcha_solved: stage(PipelineStatus::CaptchaSolved),
            awaiting_confirmation: stage(PipelineStatus::AwaitingConfirmation),
            confirmed: stage(PipelineStatus::Confirmed),
            rejected: stage(PipelineStatus::Rejected),
            failed: stage(PipelineStatus::Failed),
            pending_tasks: pending_forms + open_tasks + needs_review,
            needs_review,
        }
    }
}

/// A history row.
///
/// Timestamps are serialized as RFC 3339 and formatted by the `datetime`
/// filter at render time, so the template decides how they read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoryRow {
    pub id: i64,
    pub broker_id: String,
    pub broker_name: String,
    pub email: String,
    pub template: String,
    pub status: &'static str,
    pub error: String,
    pub sent_at: Option<String>,
    pub pipeline_status: &'static str,
}

impl From<Record> for HistoryRow {
    fn from(record: Record) -> Self {
        Self {
            id: record.id,
            broker_id: record.broker_id,
            broker_name: record.broker_name,
            email: record.email,
            template: record.template,
            status: match record.status {
                history::Status::Sent => "sent",
                history::Status::Failed => "failed",
                history::Status::Pending => "pending",
            },
            error: record.error,
            sent_at: record.sent_at.map(|time| time.to_rfc3339()),
            pipeline_status: record.pipeline_status.as_str(),
        }
    }
}

/// Distinct non-empty values of one broker field, sorted for a stable menu.
///
/// Go returned them in database order, so the filter dropdowns reshuffled
/// whenever `brokers.yaml` was edited.
pub fn unique_values(brokers: &[Broker], field: impl Fn(&Broker) -> &str) -> Vec<String> {
    let mut values: Vec<String> = brokers
        .iter()
        .map(&field)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    values.sort();
    values.dedup();
    values
}

// -------------------------------------------------------------------
// The run wizard
// -------------------------------------------------------------------

/// Extra scoping the run wizard accepts beyond the broker-page filters.
///
/// The mockup's "not written to in a while" needs a number of days, and the
/// letter choice needs to travel with the run so a retry sends the same
/// thing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RunFilters {
    #[serde(flatten)]
    pub base: BrokerFilters,
    /// Send only to brokers last contacted more than this many days ago.
    /// Zero (the default) means no staleness filter.
    pub stale_days: i64,
    /// `generic`, `ccpa`, `gdpr`, or `auto` for the per-broker best fit.
    pub template: String,
}

impl RunFilters {
    /// The template this run actually names, resolved from `auto`.
    ///
    /// A concrete name is stored as the run's template, so history and the
    /// resume file record what really went out; `auto` only exists at the
    /// moment of choosing.
    pub fn resolved_template(&self, broker: &Broker) -> String {
        match self.template.as_str() {
            "auto" | "" => crate::template::best_fit(broker).to_string(),
            other => other.to_string(),
        }
    }

    /// Whether a broker passes the staleness filter.
    ///
    /// Never-contacted brokers always pass: the point of the filter is to
    /// skip brokers already dealt with recently, not to skip ones nobody has
    /// written to yet.
    pub fn matches_staleness(&self, status: Option<&history::BrokerStatus>) -> bool {
        if self.stale_days <= 0 {
            return true;
        }
        let Some(status) = status else {
            return true;
        };
        let Some(last) = status.last_sent else {
            return true;
        };
        let age = chrono::Utc::now().signed_duration_since(last);
        age.num_days() >= self.stale_days
    }
}

/// One row of the run wizard's broker list, as the page reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunBrokerRow {
    pub id: String,
    pub name: String,
    pub region: String,
    /// The letter the run will actually send this broker.
    pub template: String,
    pub status: &'static str,
}

/// The letter split a run would produce, for the wizard's confirm step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TemplateSplitRow {
    pub file: String,
    pub who: &'static str,
    pub n: usize,
}

/// How the selected brokers divide across the three letters.
///
/// The wizard shows this when `auto` is picked, so "best fit for each
/// broker" is a promise with numbers behind it rather than a vibe.
pub fn template_split(brokers: &[&Broker]) -> Vec<TemplateSplitRow> {
    let mut gdpr = 0usize;
    let mut ccpa = 0usize;
    for broker in brokers {
        match crate::template::best_fit(broker) {
            "gdpr" => gdpr += 1,
            "ccpa" => ccpa += 1,
            _ => {}
        }
    }
    let generic = brokers.len().saturating_sub(gdpr + ccpa);

    vec![
        TemplateSplitRow {
            file: "gdpr.txt".into(),
            who: "EU or UK based",
            n: gdpr,
        },
        TemplateSplitRow {
            file: "ccpa.txt".into(),
            who: "US based",
            n: ccpa,
        },
        TemplateSplitRow {
            file: "generic.txt".into(),
            who: "everyone else",
            n: generic,
        },
    ]
}

// -------------------------------------------------------------------
// The mail view
// -------------------------------------------------------------------

/// Which mailbox a thread sits in. The names are the mockup's, and the
/// status bar and folder list both read them, so they are strings rather
/// than an enum the templates would have to translate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MailFolder {
    pub id: &'static str,
    pub label: &'static str,
    pub count: usize,
    /// Hex colour for the count, as the design styles it.
    pub color: &'static str,
}

/// One row of the thread list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MailThread {
    /// Where to go when the row is opened.
    pub href: String,
    pub broker: String,
    /// Short date, as the list shows it.
    pub date: String,
    pub snippet: String,
    /// `NEEDS_YOU · id`, `REMOVED`, and so on.
    pub code: String,
    pub selected: bool,
}

/// A colour for a pipeline stage, matching the mockup's palette.
pub fn stage_color(status: PipelineStatus) -> &'static str {
    match status {
        PipelineStatus::Confirmed => "#7fa88a",
        PipelineStatus::AwaitingResponse | PipelineStatus::AwaitingConfirmation => "#8a8276",
        PipelineStatus::FormRequired
        | PipelineStatus::AwaitingCaptcha
        | PipelineStatus::Failed
        | PipelineStatus::Rejected => "#e0703a",
        _ => "#a79f92",
    }
}

/// A short label for a pipeline stage, as the thread list shows it.
pub fn stage_label(status: PipelineStatus) -> &'static str {
    match status {
        PipelineStatus::EmailSent => "SENT",
        PipelineStatus::AwaitingResponse => "WAITING",
        PipelineStatus::FormRequired => "NEEDS_YOU · form",
        PipelineStatus::FormFilled => "FORM SENT",
        PipelineStatus::AwaitingCaptcha => "NEEDS_YOU · captcha",
        PipelineStatus::CaptchaSolved => "CAPTCHA OK",
        PipelineStatus::AwaitingConfirmation => "NEEDS_YOU · confirm",
        PipelineStatus::Confirmed => "REMOVED",
        PipelineStatus::Failed => "BOUNCED",
        PipelineStatus::Rejected => "REFUSED",
    }
}

/// The display name for a classified reply, as the mail view shows it.
pub fn response_guess(response_type: ResponseType) -> &'static str {
    match response_type {
        ResponseType::FormRequired => "wants a form",
        ResponseType::ConfirmationRequired => "sent a confirm link",
        ResponseType::Success => "removed",
        ResponseType::Rejected => "refused",
        ResponseType::Bounced => "bounced",
        ResponseType::Pending => "still waiting",
        ResponseType::Unknown => "unclear",
    }
}

#[cfg(test)]
mod tests;
