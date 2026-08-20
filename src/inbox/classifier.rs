//! Deciding what a broker's reply actually says.
//!
//! Ported from `internal/inbox/classifier.go`. The pattern tables come over
//! unchanged: they were built from real broker mail, and every entry is there
//! because some company phrased a refusal in some particular way.

use std::sync::LazyLock;

use regex::RegexSet;

use super::Email;
use super::parser::{self, ExtractedUrls};

/// What a reply turned out to be.
///
/// Ordering is fixed and is what breaks ties between equal scores. Go picked
/// the winner while ranging over a map, so an email that scored equally in
/// two categories classified differently from one run to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResponseType {
    /// The removal happened.
    Success,
    /// Delivery failed; the address is probably dead.
    Bounced,
    /// An opt-out form has to be filled in.
    FormRequired,
    /// A link has to be clicked to confirm.
    ConfirmationRequired,
    /// The broker refused, or says it holds nothing.
    Rejected,
    /// Acknowledged, still being worked on.
    Pending,
    /// Could not tell.
    Unknown,
}

impl ResponseType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Bounced => "bounced",
            Self::FormRequired => "form_required",
            Self::ConfirmationRequired => "confirmation_required",
            Self::Rejected => "rejected",
            Self::Pending => "pending",
            Self::Unknown => "unknown",
        }
    }

    /// The categories that compete for a score. `Bounced`, `Success` from a
    /// test email, and `Unknown` are decided outside the scoring.
    const SCORED: [Self; 5] = [
        Self::Success,
        Self::FormRequired,
        Self::ConfirmationRequired,
        Self::Rejected,
        Self::Pending,
    ];

    /// Whether this reply needs the user to go and do something.
    pub fn is_actionable(self) -> bool {
        matches!(self, Self::FormRequired | Self::ConfirmationRequired)
    }

    /// One line explaining the classification, shown in the UI.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Success => "The broker says the removal is done",
            Self::Bounced => "The message could not be delivered — the address may be dead",
            Self::FormRequired => "There is an opt-out form to fill in",
            Self::ConfirmationRequired => "There is a link to click to confirm",
            Self::Rejected => "The broker refused, or says it holds nothing about you",
            Self::Pending => "Received and being worked on; may need chasing",
            Self::Unknown => "Could not tell what this reply says",
        }
    }
}

impl std::fmt::Display for ResponseType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Map onto the type stored in history.
impl From<ResponseType> for crate::history::ResponseType {
    fn from(value: ResponseType) -> Self {
        match value {
            ResponseType::Success => Self::Success,
            ResponseType::FormRequired => Self::FormRequired,
            ResponseType::ConfirmationRequired => Self::ConfirmationRequired,
            ResponseType::Rejected => Self::Rejected,
            ResponseType::Pending => Self::Pending,
            ResponseType::Bounced => Self::Bounced,
            ResponseType::Unknown => Self::Unknown,
        }
    }
}

/// A reply, and what was made of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedResponse {
    pub response_type: ResponseType,
    pub urls: ExtractedUrls,
    /// The form to fill in, when there is one.
    pub form_url: Option<String>,
    /// The link to click, when there is one.
    pub confirm_url: Option<String>,
    /// The address that bounced, when this is a bounce.
    pub bounced_recipient: Option<String>,
    /// 0.0 to 1.0.
    pub confidence: f64,
    pub reason: &'static str,
    /// Whether a person should look at this one.
    pub needs_review: bool,
}

/// Below this, a classification is not trusted enough to act on.
const REVIEW_THRESHOLD: f64 = 0.4;
/// A subject-line match is worth this much more than a body match.
const SUBJECT_WEIGHT: i32 = 3;

macro_rules! pattern_set {
    ($name:ident, $($pattern:literal),+ $(,)?) => {
        static $name: LazyLock<RegexSet> = LazyLock::new(|| {
            RegexSet::new([$($pattern),+]).expect(concat!(stringify!($name), " must compile"))
        });
    };
}

pattern_set!(
    SUCCESS,
    r"(?i)request\s+(has\s+been\s+)?(completed|processed|fulfilled)",
    r"(?i)successfully\s+(removed|deleted|opted\s*out)",
    r"(?i)your\s+(data|information)\s+(has\s+been\s+)?(removed|deleted)",
    r"(?i)opt[\s-]?out\s+(request\s+)?(is\s+)?(complete|confirmed)",
    r"(?i)we\s+have\s+(removed|deleted)",
    r"(?i)no\s+longer\s+(have|hold|store)\s+your\s+(data|information)",
);

pattern_set!(
    FORM_REQUIRED,
    r"(?i)please\s+(complete|fill\s*(out|in)?|submit)\s+(the|our|this)?\s*(form|request)",
    r"(?i)visit\s+(the\s+)?(following\s+)?(link|url|page)\s+to\s+(complete|submit|verify)",
    r"(?i)click\s+(here|below|the\s+link)\s+to\s+(begin|start|submit|complete)",
    r"(?i)(must|need\s+to)\s+(verify|confirm)\s+your\s+(identity|request)",
    r"(?i)submit\s+a\s+(formal\s+)?request\s+(through|via|at)",
    r"(?i)use\s+(our|the)\s+(online|web)\s*(form|portal|tool)",
    r"(?i)please\s+submit\s+(your\s+)?request\s+(at|via|through)\s+",
    r"(?i)please\s+use\s+(our|the)\s+(opt[\s-]?out|removal|privacy)\s*(form|page|link)",
    r"(?i)complete\s+(the|our|a)\s+(data\s+subject|privacy|opt[\s-]?out)\s*(access\s+)?(request\s+)?form",
    r"(?i)submit\s+(a\s+|your\s+)?(request|form)\s+(via|through|at|using)\s+(our\s+)?(online|web|interactive)",
    r"(?i)(does\s+not|do\s+not|cannot)\s+accept\s+privacy\s+requests?\s+(via|by|through)\s+email",
    r"(?i)this\s+email\s+(address\s+)?is\s+not\s+intended\s+for\s+privacy",
    r"(?i)visit\s+(our|the)\s+(opt[\s-]?out|removal|privacy)\s*(page|form|portal)",
    r"(?i)(data\s+subject|privacy)\s+requests?\s+(can|should|must)\s+be\s+(filed|submitted)\s+(at|via)",
    r"(?i)go\s+to\s+(the\s+)?(link|url|page)\s+(below|above)",
    r"(?i)please\s+click\s+(on\s+)?(the\s+)?following\s+link",
    r"(?i)you\s+(can|may)\s+submit\s+.{0,30}(privacy|opt[\s-]?out)",
    r"(?i)right\s+to\s+(opt[\s-]?out|delete|know)[:\s]",
    r"(?i)we\s+(have\s+)?established\s+a\s+dedicated\s+(online\s+)?form",
    r"(?i)we\s+do\s+not\s+process\s+requests?\s+via\s+email",
    r"(?i)please\s+send\s+(your?\s+)?request\s+to\s+customer\s+service",
    r"(?i)is\s+not\s+(a\s+)?mechanism\s+for.{0,30}(privacy|request)",
    r"(?i)please\s+complete\s+your\s+(request|form)",
);

pattern_set!(
    CONFIRMATION,
    r"(?i)click\s+(here|below|the\s+link)\s+to\s+(confirm|verify|validate)",
    r"(?i)please\s+(confirm|verify)\s+(your\s+)?(email|request|identity)",
    r"(?i)verification\s+(link|email|code)",
    r"(?i)confirm\s+your\s+(email\s+)?(address)?",
    r"(?i)click\s+(to\s+)?confirm",
    r"(?i)(can|could)\s+you\s+(please\s+)?verify",
    r"(?i)verify\s+(your\s+)?(last\s+4|ssn|social)",
);

pattern_set!(
    REJECTION,
    r"(?i)(cannot|can't|unable\s+to)\s+(process|complete|fulfill)\s+(your\s+)?request",
    r"(?i)request\s+(has\s+been\s+)?(denied|rejected|declined)",
    r"(?i)do\s+not\s+have\s+(any\s+)?(data|information|records)\s+(about|for|on)\s+you",
    r"(?i)not\s+found\s+in\s+our\s+(system|database|records)",
    r"(?i)no\s+(matching\s+)?(records?|data|information)\s+found",
    r"(?i)exempt\s+from\s+(CCPA|GDPR|this\s+request)",
    r"(?i)(do\s+not|don't)\s+have\s+(any\s+)?record",
    // Added here, not in upstream: "we have no record of you" is one of the
    // commonest ways a broker says it holds nothing about you, and it
    // classified as Unknown — sending a perfectly clear refusal to the
    // review queue for a person to read.
    r"(?i)(we\s+)?have\s+no\s+(record|data|information)",
    r"(?i)no\s+record\s+of\s+(you|your)",
    r"(?i)no\s+(matching\s+)?record\s+(of\s+)?(a\s+)?report",
    r"(?i)maintains?\s+no\s+(files?|records?|data)",
    r"(?i)no\s+longer\s+(registered|operating)\s+(as\s+)?(an?\s+)?(active\s+)?data\s+broker",
    r"(?i)(this\s+)?(email|inbox)(\s+\w+)?\s+(is\s+)?(no\s+longer|being\s+retired)",
    r"(?i)service\s+offerings?\s+no\s+longer\s+include",
    r"(?i)(we\s+)?(have\s+)?no\s+data\s+(linked|associated|related)\s+to\s+(your|this)",
    r"(?i)not\s+identified\s+in\s+our\s+database",
    r"(?i)(we\s+are|we're)\s+a\s+b2b\s+(platform|company|business)",
    r"(?i)has\s+never\s+existed\s+in\s+our\s+database",
    r"(?i)consumer\s+reporting\s+agenc(y|ies)\s+(is|are)\s+exempt",
    r"(?i)fair\s+credit\s+reporting\s+act.{0,30}exempt",
    r"(?i)we\s+do\s+not\s+(remove|delete)\s+data\s+by\s+request",
    r"(?i)(your\s+)?(email|name|address|information)\s+was\s+not\s+identified",
    r"(?i)(this\s+)?(email|inbox)\s+(address\s+)?(is\s+)?not\s+(a\s+)?(mechanism|intended)\s+(for|to)",
    r"(?i)not\s+intended\s+for\s+(the\s+)?(submission|handling)\s+of\s+privacy",
    r"(?i)will\s+not\s+be\s+considered\s+a\s+valid\s+submission",
);

pattern_set!(
    PENDING,
    r"(?i)(is\s+being|currently\s+being)\s+(processed|reviewed|handled)",
    r"(?i)will\s+(process|complete|handle)\s+(your\s+)?request\s+within",
    r"(?i)please\s+allow\s+(\d+)\s+(days|business\s+days|weeks)",
    r"(?i)we('ll|\s+will)\s+(get\s+back|respond|follow\s+up)",
    r"(?i)request\s+(has\s+been\s+)?(received|acknowledged)",
    r"(?i)thank\s+you\s+for\s+(your\s+)?(inquiry|email|contacting|reaching|privacy)",
    r"(?i)we\s+(have\s+)?received\s+your\s+(request|email|inquiry)",
    r"(?i)(has\s+been\s+)?assigned\s+(a\s+)?(ticket|case|reference)\s*(number|#|id)?",
    r"(?i)one\s+of\s+our\s+.{0,30}(will\s+)?(reach\s+out|respond|contact)",
    r"(?i)ticket\s+(has\s+been\s+)?(created|opened|received)",
    r"(?i)your\s+request\s+has\s+been\s+received",
    r"(?i)support\s+request\s*#?\d+",
    r"(?i)legal\s+request\s+received",
    r"(?i)i\s+(have\s+)?(now\s+)?left\s+",
    r"(?i)no\s+longer\s+with\s+(the\s+)?(company|organization)",
    r"(?i)(will\s+be\s+)?(removed|deleted)\s+from\s+our\s+database.{0,20}\d+\s+days",
    r"(?i)once\s+verified.{0,30}(will\s+be\s+)?(processed|complete)",
    r"(?i)this\s+(message\s+)?confirms\s+(our\s+)?receipt",
    r"(?i)we\s+appreciate\s+your\s+interest\s+in\s+exercising",
    r"(?i)request\s+(will\s+be\s+)?(processed|fulfilled)",
    r"(?i)automatic\s+reply",
    r"(?i)auto[\s-]?response",
);

pattern_set!(
    SUBJECT_PENDING,
    r"(?i)^automatic\s+reply",
    r"(?i)^auto[\s-]?reply",
    r"(?i)^auto[\s-]?response",
    r"(?i)^out\s+of\s+office",
    r"(?i)request\s+received",
    r"(?i)has\s+been\s+received",
    r"(?i)thank\s+you\s+for\s+your\s+(privacy|data|removal|email)",
    r"(?i)thank\s+you\s+for\s+(your\s+)?email\s+to",
    r"(?i)thanks\s+for\s+(reaching|contacting)",
    r"(?i)#[A-Z]{0,3}[-]?\d{5,}",
    r"(?i)request\s*#\s*\d+",
    r"(?i)support\s+request",
    r"(?i)ticket\s*[\(#]\s*:?\s*\d+",
    r"(?i)we\s+have\s+received\s+your\s+ticket",
    r"(?i)i\s+(have\s+)?(now\s+)?left\s+",
    r"(?i)no\s+longer\s+with\s+(the\s+)?(company|organization)",
    r"(?i)office\s+closed",
    r"(?i)response\s+to\s+your\s+email",
);

pattern_set!(
    SUBJECT_REJECTION,
    r"(?i)not\s+found",
    r"(?i)no\s+record",
    r"(?i)unable\s+to\s+(locate|find|process)",
    r"(?i)request\s+(denied|rejected)",
);

pattern_set!(
    SUBJECT_SUCCESS,
    r"(?i)opt[\s-]?out\s+(has\s+been\s+)?completed",
    r"(?i)(has\s+been\s+|successfully\s+)?(removed|deleted)",
    r"(?i)ticket.+solved",
    r"(?i)request\s+(has\s+been\s+)?(completed|processed|fulfilled)",
);

pattern_set!(
    SUBJECT_FORM,
    r"(?i)opt[\s-]?out\s+instructions",
    r"(?i)removal\s+instructions",
    r"(?i)how\s+to\s+(opt[\s-]?out|remove)",
);

pattern_set!(
    BOUNCE,
    r"(?i)delivery\s+(to\s+.+\s+)?(has\s+)?failed",
    r"(?i)undeliverable",
    r"(?i)delivery\s+status\s+notification",
    r"(?i)returned\s+mail",
    r"(?i)mail\s+delivery\s+failed",
    r"(?i)message\s+(could\s+)?not\s+(be\s+)?delivered",
    r"(?i)could\s+not\s+be\s+delivered",
    r"(?i)delivery\s+failure",
    r"(?i)permanent\s+(failure|error)",
    r"(?i)address\s+rejected",
    r"(?i)user\s+unknown",
    r"(?i)mailbox\s+not\s+found",
    r"(?i)no\s+such\s+user",
    r"(?i)(mailbox|recipient|address)\s+(does\s+not|doesn't)\s+exist",
    r"(?i)invalid\s+(recipient|address|mailbox)",
    r"(?i)unknown\s+(recipient|user|address)",
    r"(?i)550\s+.*\s+(rejected|unknown|not\s+found)",
    r"(?i)554\s+.*\s+(rejected|failed)",
);

// Test messages eruser sent to itself during setup.
pattern_set!(
    TEST_EMAIL,
    r"(?i)eruser\s+test\s+email",
    r"(?i)eraser\s+test\s+email",
    r"(?i)test\s+email\s+from\s+er(us|as)er",
    r"(?i)this\s+is\s+a\s+test\s+email",
    r"(?i)eruser\s+is\s+set\s+up",
);

/// Senders that only ever mean a delivery failure.
const BOUNCE_SENDERS: &[&str] = &[
    "mailer-daemon",
    "postmaster",
    "mail delivery system",
    "mail delivery subsystem",
    "mailerdaemon",
    "noreply",
    "no-reply",
    "mailsystem",
];

fn count(set: &RegexSet, text: &str) -> i32 {
    set.matches(text).iter().count() as i32
}

/// Read a broker's reply.
pub fn classify(email: &Email) -> ClassifiedResponse {
    let urls = parser::parse_email_urls(email);
    let content = email.text().to_lowercase();
    let subject = email.subject.to_lowercase();

    // A message eruser sent itself proves the mail settings work and is not
    // a broker reply at all.
    if TEST_EMAIL.is_match(&subject) || TEST_EMAIL.is_match(&content) {
        return ClassifiedResponse {
            response_type: ResponseType::Success,
            urls,
            form_url: None,
            confirm_url: None,
            bounced_recipient: None,
            confidence: 1.0,
            reason: "The test message from eruser — sending works",
            needs_review: false,
        };
    }

    // Bounces are checked before anything else: the wording in a delivery
    // failure often quotes the original request back, which otherwise scores
    // as whatever the request said.
    if is_bounce(email, &subject, &content) {
        return ClassifiedResponse {
            response_type: ResponseType::Bounced,
            bounced_recipient: parser::extract_bounced_recipient(email),
            urls,
            form_url: None,
            confirm_url: None,
            confidence: 0.95,
            reason: ResponseType::Bounced.reason(),
            needs_review: false,
        };
    }

    let scores = score(&subject, &content, &urls);
    let (response_type, top, runner_up) = pick(&scores);

    let mut result = ClassifiedResponse {
        response_type,
        form_url: parser::primary_form_url(&urls, &email.from_domain),
        confirm_url: parser::primary_confirmation_url(&urls, &email.from_domain),
        urls,
        bounced_recipient: None,
        confidence: confidence_for(top, runner_up),
        reason: response_type.reason(),
        needs_review: false,
    };

    if top >= SUBJECT_WEIGHT {
        result.confidence = result.confidence.max(0.75);
    }
    // A classification backed by an actual link is worth more than one backed
    // only by wording.
    if result.response_type == ResponseType::FormRequired && !result.urls.form_urls.is_empty() {
        result.confidence = result.confidence.max(0.85);
    }
    if result.response_type == ResponseType::ConfirmationRequired
        && !result.urls.confirmation_urls.is_empty()
    {
        result.confidence = result.confidence.max(0.85);
    }

    // Nothing matched: fall back to whatever links are in the message.
    if top == 0 {
        if !result.urls.confirmation_urls.is_empty() {
            result.response_type = ResponseType::ConfirmationRequired;
            result.confidence = 0.5;
        } else if !result.urls.form_urls.is_empty() {
            result.response_type = ResponseType::FormRequired;
            result.confidence = 0.5;
        }
        result.reason = result.response_type.reason();
    }

    result.needs_review =
        result.response_type == ResponseType::Unknown || result.confidence < REVIEW_THRESHOLD;

    result
}

/// Score every category from the subject, the body, and the links.
fn score(subject: &str, content: &str, urls: &ExtractedUrls) -> [(ResponseType, i32); 5] {
    let mut success = count(&SUCCESS, content) + count(&SUCCESS, subject);
    let mut form = count(&FORM_REQUIRED, content);
    let mut confirmation = count(&CONFIRMATION, content);
    let mut rejected = count(&REJECTION, content) + count(&REJECTION, subject);
    let mut pending = count(&PENDING, content);

    // A subject line is a much stronger signal than a phrase buried in a
    // footer, so those patterns are weighted.
    success += count(&SUBJECT_SUCCESS, subject) * SUBJECT_WEIGHT;
    form += count(&SUBJECT_FORM, subject) * SUBJECT_WEIGHT;
    rejected += count(&SUBJECT_REJECTION, subject) * SUBJECT_WEIGHT;
    pending += count(&SUBJECT_PENDING, subject) * SUBJECT_WEIGHT;

    if !urls.form_urls.is_empty() {
        form += 2;
    }
    if !urls.confirmation_urls.is_empty() {
        confirmation += 2;
    }

    [
        (ResponseType::Success, success),
        (ResponseType::FormRequired, form),
        (ResponseType::ConfirmationRequired, confirmation),
        (ResponseType::Rejected, rejected),
        (ResponseType::Pending, pending),
    ]
}

/// The winning category, its score, and the runner-up's.
///
/// Ties break by [`ResponseType`] order, which is fixed. Go picked the winner
/// while ranging over a map, so the same email could classify differently on
/// different runs.
fn pick(scores: &[(ResponseType, i32); 5]) -> (ResponseType, i32, i32) {
    debug_assert_eq!(
        scores.map(|(kind, _)| kind),
        ResponseType::SCORED,
        "scores must stay in the declared order for ties to be stable"
    );

    let mut ranked = *scores;
    ranked.sort_by(|(a_type, a), (b_type, b)| b.cmp(a).then(a_type.cmp(b_type)));

    let (winner, top) = ranked[0];
    let runner_up = ranked[1].1;

    if top == 0 {
        return (ResponseType::Unknown, 0, 0);
    }
    (winner, top, runner_up)
}

/// How sure the classification is, from how far ahead the winner was.
fn confidence_for(top: i32, runner_up: i32) -> f64 {
    if top == 0 {
        return 0.0;
    }
    if runner_up == 0 {
        // Only one category matched at all.
        return 0.85;
    }
    let margin = f64::from(top - runner_up) / f64::from(top);
    0.5 + margin * 0.4
}

/// Whether this is a delivery failure rather than a reply.
fn is_bounce(email: &Email, subject: &str, content: &str) -> bool {
    let from = email.from.to_lowercase();
    let from_name = email.from_name.to_lowercase();

    let from_mail_system = BOUNCE_SENDERS
        .iter()
        .any(|sender| from.contains(sender) || from_name.contains(sender));

    // A subject match is worth double: "Undeliverable" in a subject line is
    // conclusive in a way that the same word in a quoted footer is not.
    let score = count(&BOUNCE, subject) * 2 + count(&BOUNCE, content);

    (from_mail_system && score > 0) || score >= 3
}

/// Classify from a subject line alone.
///
/// Used when reclassifying stored replies whose bodies predate the column
/// that keeps them. Confidence is capped lower, because a subject is a
/// fraction of the evidence.
pub fn classify_by_subject(subject: &str) -> (ResponseType, f64, bool) {
    let subject = subject.to_lowercase();

    let scores = [
        (
            ResponseType::Success,
            count(&SUBJECT_SUCCESS, &subject) * SUBJECT_WEIGHT + count(&SUCCESS, &subject),
        ),
        (
            ResponseType::FormRequired,
            count(&SUBJECT_FORM, &subject) * SUBJECT_WEIGHT,
        ),
        (ResponseType::ConfirmationRequired, 0),
        (
            ResponseType::Rejected,
            count(&SUBJECT_REJECTION, &subject) * SUBJECT_WEIGHT + count(&REJECTION, &subject),
        ),
        (
            ResponseType::Pending,
            count(&SUBJECT_PENDING, &subject) * SUBJECT_WEIGHT + count(&PENDING, &subject),
        ),
    ];

    let (response_type, top, _) = pick(&scores);

    match top {
        0 => (ResponseType::Unknown, 0.0, true),
        // A weighted subject pattern matched outright.
        score if score >= SUBJECT_WEIGHT => (response_type, 0.7, false),
        _ => (response_type, 0.4, true),
    }
}

/// Counts across a batch of classified replies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub total: usize,
    pub success: usize,
    pub form_required: usize,
    pub confirmation_required: usize,
    pub rejected: usize,
    pub pending: usize,
    pub bounced: usize,
    pub unknown: usize,
    pub needs_review: usize,
}

pub fn summarize(responses: &[ClassifiedResponse]) -> Summary {
    let mut summary = Summary {
        total: responses.len(),
        ..Default::default()
    };

    for response in responses {
        match response.response_type {
            ResponseType::Success => summary.success += 1,
            ResponseType::FormRequired => summary.form_required += 1,
            ResponseType::ConfirmationRequired => summary.confirmation_required += 1,
            ResponseType::Rejected => summary.rejected += 1,
            ResponseType::Pending => summary.pending += 1,
            ResponseType::Bounced => summary.bounced += 1,
            ResponseType::Unknown => summary.unknown += 1,
        }
        if response.needs_review {
            summary.needs_review += 1;
        }
    }

    summary
}

#[cfg(test)]
mod tests;
