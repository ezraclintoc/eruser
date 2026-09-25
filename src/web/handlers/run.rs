//! The run wizard: pick who to write to, pick the letter, send.
//!
//! The mockup splits this into four steps — profile, recipients, letter,
//! send — and that is what this builds. The send itself is the same
//! background job the old dashboard started: `/api/send-all` with the
//! wizard's filters in the body.

use axum::extract::{Query, Request, State};
use axum::response::Response;

use super::{csrf_of, render};
use crate::history::BrokerStatus;
use crate::web::auth::CurrentUser;
use crate::web::error::WebError;
use crate::web::state::AppState;
use crate::web::views::{
    BrokerFilters, RunBrokerRow, RunFilters, TemplateSplitRow, template_split,
};

/// GET /run — the wizard.
///
/// Everything the four steps show is rendered at once, server-side; the
/// browser hides and shows them. No step holds state that can go stale,
/// because the only action that matters is the POST to /api/send-all at the
/// end, and the filters travel with it in the body.
pub async fn run(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(filters): Query<RunFilters>,
    request: Request,
) -> Result<Response, WebError> {
    if let Some(redirect) = super::require_setup(&user) {
        return Ok(redirect);
    }

    let filters = RunFilters {
        base: filters.base.normalized(),
        stale_days: filters.stale_days.clamp(0, 3650),
        template: match filters.template.as_str() {
            "generic" | "ccpa" | "gdpr" | "auto" => filters.template.clone(),
            _ => "auto".to_string(),
        },
    };

    let statuses = state.store.all_broker_statuses(user.id()).await?;
    let profile = user.config().profile.clone();

    // Every broker, annotated with the letter this run would send it and
    // whether it passes the filters. The client-side steps filter this list
    // as the person narrows the scope.
    let mut rows: Vec<RunBrokerRow> = state
        .brokers
        .brokers
        .iter()
        .map(|broker| {
            let status = statuses.get(&broker.id);
            RunBrokerRow {
                id: broker.id.clone(),
                name: broker.name.clone(),
                region: broker.region.clone(),
                template: filters.resolved_template(broker),
                status: broker_status_label(status, &profile.email),
            }
        })
        .collect();
    rows.sort_by_key(|a| a.name.to_lowercase());

    // Counts for the scope options, computed once where the data is.
    let total = state.brokers.brokers.len();
    let never = rows.iter().filter(|row| row.status == "never").count();
    let us_only = rows.iter().filter(|row| row.region == "us").count();
    let stale = state
        .brokers
        .brokers
        .iter()
        .filter(|broker| filters.matches_staleness(statuses.get(&broker.id)))
        .count();

    let profile_fields = profile_summary(&user);

    let region_counts: Vec<(String, usize)> = {
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for row in &rows {
            *counts.entry(row.region.clone()).or_default() += 1;
        }
        let mut names: Vec<(String, usize)> = counts.into_iter().collect();
        names.sort();
        names
    };

    render(
        &state,
        &user,
        csrf_of(&request).as_ref(),
        "run.html",
        minijinja::context! {
            title => "Run",
            brokers => rows,
            total => total,
            never_count => never,
            us_count => us_only,
            stale_count => stale,
            stale_days => if filters.stale_days > 0 { filters.stale_days } else { 30 },
            template_choice => filters.template,
            split => split_preview(&state, &filters, &statuses)?,
            profile_fields => profile_fields,
            send_from => user.config().email.from,
            rate_limit_ms => user.config().options.rate_limit_ms,
            region_counts => region_counts,
            capacity => state.store.remaining_capacity_today(user.id()).await?,
        },
    )
}

/// How the auto split falls for the brokers a run would hit right now.
///
/// The real split depends on the scope the person settles on; this shows the
/// split over the whole database as a live-enough preview, and the send
/// confirmation repeats it for exactly the selected set on the next screen.
fn split_preview(
    state: &AppState,
    filters: &RunFilters,
    statuses: &std::collections::HashMap<String, BrokerStatus>,
) -> Result<Vec<TemplateSplitRow>, WebError> {
    let selected: Vec<&crate::broker::Broker> = state
        .brokers
        .brokers
        .iter()
        .filter(|broker| {
            BrokerWithStatusMatches::new(broker, statuses.get(&broker.id)).passes(filters)
        })
        .collect();
    Ok(template_split(&selected))
}

/// A small adapter so the shared `BrokerWithStatus` filter logic can also
/// judge the run's staleness rule.
struct BrokerWithStatusMatches<'a> {
    broker: &'a crate::broker::Broker,
    status: Option<&'a BrokerStatus>,
}

impl<'a> BrokerWithStatusMatches<'a> {
    fn new(broker: &'a crate::broker::Broker, status: Option<&'a BrokerStatus>) -> Self {
        Self { broker, status }
    }

    /// The shared filters (search, region, category, status), plus the
    /// wizard's staleness rule.
    fn passes(&self, filters: &RunFilters) -> bool {
        let base = BrokerFilters {
            search: String::new(),
            category: String::new(),
            region: filters.base.region.clone(),
            status: filters.base.status.clone(),
        };

        if !filters.base.search.is_empty() {
            let needle = filters.base.search.to_lowercase();
            if !self.broker.name.to_lowercase().contains(&needle)
                && !self.broker.email.to_lowercase().contains(&needle)
            {
                return false;
            }
        }

        let adapter = crate::web::views::BrokerWithStatus {
            broker: self.broker.clone(),
            status: match self.status {
                Some(status) => match status.status {
                    crate::history::Status::Sent => "sent",
                    crate::history::Status::Failed => "failed",
                    crate::history::Status::Pending => "pending",
                },
                None => "never",
            },
            last_sent: String::new(),
            total_sent: 0,
        };
        if !adapter.matches(&base) {
            return false;
        }

        filters.matches_staleness(self.status)
    }
}

/// What the wizard calls a broker's standing, in one word.
fn broker_status_label(status: Option<&BrokerStatus>, _profile_email: &str) -> &'static str {
    match status {
        None => "never",
        Some(status) => match status.status {
            crate::history::Status::Sent => "sent",
            crate::history::Status::Failed => "failed",
            crate::history::Status::Pending => "pending",
        },
    }
}

/// The profile as the wizard's first step shows it: name, address, phone.
fn profile_summary(user: &CurrentUser) -> Vec<(&'static str, String)> {
    let profile = &user.config().profile;
    let mut fields = vec![
        ("name", profile.full_name()),
        ("email", profile.email.clone()),
        ("address", profile.address.clone()),
        ("phone", profile.phone.clone()),
    ];
    fields.retain(|(_, value)| !value.is_empty());
    fields
}
