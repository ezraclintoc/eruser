//! The page template environment.
//!
//! Go built one `*template.Template` per page, reparsing the layout and every
//! partial into each, because Go templates share a global namespace and two
//! pages both defining "content" would collide. minijinja has real template
//! inheritance, so the layout is loaded once and pages extend it.

use minijinja::{Environment, Value};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "templates/web/"]
struct PageTemplates;

/// Build the environment with every page template loaded.
pub fn build() -> Result<Environment<'static>, minijinja::Error> {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|name| {
        if name.ends_with(".html") {
            minijinja::AutoEscape::Html
        } else {
            minijinja::AutoEscape::None
        }
    });

    // A page naming a field that no longer exists should lose that value,
    // not become a blank error page. Chainable also allows walking into a
    // missing value, so `{{ config.profile.city }}` before setup renders as
    // nothing instead of failing the whole page.
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Chainable);

    for name in PageTemplates::iter() {
        let file = PageTemplates::get(&name).expect("iter only yields embedded files");
        let source = String::from_utf8(file.data.into_owned()).map_err(|_| {
            minijinja::Error::new(
                minijinja::ErrorKind::SyntaxError,
                format!("template {name} is not valid UTF-8"),
            )
        })?;
        env.add_template_owned(name.to_string(), source)?;
    }

    env.add_filter("datetime", format_datetime);
    env.add_filter("date", format_date);
    env.add_filter("relative", format_relative);

    Ok(env)
}

/// `Jan 2, 2006 3:04 PM`, matching Go's `formatTime`.
fn format_datetime(value: Value) -> String {
    match parse_time(&value) {
        Some(time) => time
            .with_timezone(&chrono::Local)
            .format("%b %-d, %Y %-I:%M %p")
            .to_string(),
        None => String::new(),
    }
}

/// `Jan 2, 2006`, matching Go's `formatDate`.
fn format_date(value: Value) -> String {
    match parse_time(&value) {
        Some(time) => time
            .with_timezone(&chrono::Local)
            .format("%b %-d, %Y")
            .to_string(),
        None => String::new(),
    }
}

/// "3 days ago". Go had no equivalent; a wall-clock timestamp answers the
/// wrong question when what you want to know is whether a broker has had
/// long enough to reply.
fn format_relative(value: Value) -> String {
    let Some(time) = parse_time(&value) else {
        return String::new();
    };

    let elapsed = chrono::Utc::now().signed_duration_since(time);
    let (count, unit) = match elapsed.num_seconds() {
        seconds if seconds < 60 => return "just now".to_string(),
        seconds if seconds < 3600 => (elapsed.num_minutes(), "minute"),
        seconds if seconds < 86_400 => (elapsed.num_hours(), "hour"),
        seconds if seconds < 2_592_000 => (elapsed.num_days(), "day"),
        seconds if seconds < 31_536_000 => (elapsed.num_days() / 30, "month"),
        _ => (elapsed.num_days() / 365, "year"),
    };

    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

/// Timestamps arrive as RFC 3339 strings from serde.
fn parse_time(value: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let text = value.as_str()?;
    if text.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.with_timezone(&chrono::Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A syntax error in any template is a blank page at runtime, so every
    /// one is parsed here instead.
    #[test]
    fn every_embedded_template_parses() {
        let env = build().expect("all templates must parse");
        let names: Vec<_> = env.templates().map(|(name, _)| name).collect();

        assert!(names.contains(&"layout.html"), "found {names:?}");
        assert!(names.contains(&"dashboard.html"));
        assert!(names.contains(&"brokers.html"));
        assert!(names.contains(&"partials/broker-list.html"));
        assert!(
            names.len() >= 20,
            "expected every page, found {}",
            names.len()
        );
    }

    #[test]
    fn a_page_inherits_the_layout() {
        let env = build().unwrap();
        let rendered = env
            .get_template("dashboard.html")
            .unwrap()
            .render(minijinja::context! {
                title => "Dashboard",
                profile => minijinja::context! { first_name => "Jane" },
            })
            .expect("the dashboard should render");

        assert!(
            rendered.contains("<!DOCTYPE html>"),
            "the layout is missing"
        );
        assert!(rendered.contains("Welcome back, Jane"));
        assert!(rendered.contains("— eruser</title>"));
    }

    /// The whole reason the assets were vendored.
    #[test]
    fn a_rendered_page_loads_nothing_from_a_third_party() {
        let env = build().unwrap();
        let rendered = env
            .get_template("dashboard.html")
            .unwrap()
            .render(minijinja::context! { title => "Dashboard" })
            .unwrap();

        for host in ["cdn.tailwindcss.com", "unpkg.com", "fonts.googleapis.com"] {
            assert!(!rendered.contains(host), "the page still loads from {host}");
        }
        assert!(rendered.contains("/static/css/app.css"));
        assert!(rendered.contains("/static/js/htmx.min.js"));
    }

    /// Broker names come from a community-edited file and reach the page.
    #[test]
    fn page_output_is_html_escaped() {
        let env = build().unwrap();
        let rendered = env
            .get_template("dashboard.html")
            .unwrap()
            .render(minijinja::context! {
                title => "Dashboard",
                profile => minijinja::context! { first_name => "<script>alert(1)</script>" },
            })
            .unwrap();

        assert!(!rendered.contains("<script>alert(1)</script>"));
        assert!(rendered.contains("&lt;script&gt;"));
    }

    /// A page naming a field that no longer exists should lose that value,
    /// not become an error page.
    #[test]
    fn a_missing_value_renders_as_nothing() {
        let env = build().unwrap();
        let rendered = env
            .get_template("dashboard.html")
            .unwrap()
            .render(minijinja::context! { title => "Dashboard" })
            .expect("a missing field must not fail the render");

        assert!(rendered.contains("Welcome back,"));
    }

    #[test]
    fn timestamps_format_the_way_the_go_version_did() {
        let time = Value::from("2026-08-19T15:04:05Z");
        let formatted = format_datetime(time.clone());

        assert!(formatted.contains("Aug 19, 2026"), "{formatted}");
        assert!(
            formatted.contains("AM") || formatted.contains("PM"),
            "{formatted}"
        );
        assert_eq!(format_date(time), "Aug 19, 2026");
    }

    #[test]
    fn a_missing_or_unparseable_timestamp_renders_as_nothing() {
        for value in [Value::from(""), Value::from("not a time"), Value::from(())] {
            assert_eq!(format_datetime(value.clone()), "");
            assert_eq!(format_date(value.clone()), "");
            assert_eq!(format_relative(value), "");
        }
    }

    #[test]
    fn relative_times_read_naturally() {
        let ago = |duration: chrono::Duration| {
            format_relative(Value::from((chrono::Utc::now() - duration).to_rfc3339()))
        };

        assert_eq!(ago(chrono::Duration::seconds(5)), "just now");
        assert_eq!(ago(chrono::Duration::minutes(1)), "1 minute ago");
        assert_eq!(ago(chrono::Duration::minutes(5)), "5 minutes ago");
        assert_eq!(ago(chrono::Duration::hours(3)), "3 hours ago");
        assert_eq!(ago(chrono::Duration::days(1)), "1 day ago");
        assert_eq!(ago(chrono::Duration::days(45)), "1 month ago");
        assert_eq!(ago(chrono::Duration::days(400)), "1 year ago");
    }
}
