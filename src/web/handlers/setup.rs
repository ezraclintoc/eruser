//! The setup wizard.
//!
//! Ported from the `/setup` handlers in `internal/web/server.go`. The wizard
//! collects an SMTP password before there is anywhere to save it, so answers
//! accumulate in a server-side session and only reach disk at the last step.

use axum::extract::{Request, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};

use super::{csrf_of, read_form, render};
use crate::config::{Config, EmailConfig, Profile, SmtpConfig};
use crate::email::{Message, new_sender};
use crate::web::error::WebError;
use crate::web::security::cookie_header;
use crate::web::session::{COOKIE_NAME, Session};
use crate::web::state::AppState;

/// Gmail's submission endpoint, which is what nearly everyone uses.
const GMAIL_SMTP_HOST: &str = "smtp.gmail.com";
const GMAIL_SMTP_PORT: u16 = 465;

pub async fn welcome(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let (session_id, _) = session_for(&state, &request);

    let mut response = render(
        &state,
        csrf.as_ref(),
        "setup/welcome.html",
        minijinja::context! { title => "Welcome", step => "welcome" },
    )?;
    attach_session(&mut response, &session_id);
    Ok(response)
}

pub async fn show_profile(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let (session_id, session) = session_for(&state, &request);

    let mut response = render(
        &state,
        csrf.as_ref(),
        "setup/profile.html",
        minijinja::context! {
            title => "Your details",
            step => "profile",
            profile => session.profile,
        },
    )?;
    attach_session(&mut response, &session_id);
    Ok(response)
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct ProfileForm {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub address: String,
    pub city: String,
    pub state: String,
    pub zip_code: String,
    pub country: String,
    pub phone: String,
    pub date_of_birth: String,
}

impl From<ProfileForm> for Profile {
    fn from(form: ProfileForm) -> Self {
        Self {
            first_name: form.first_name.trim().to_string(),
            last_name: form.last_name.trim().to_string(),
            email: form.email.trim().to_string(),
            address: form.address.trim().to_string(),
            city: form.city.trim().to_string(),
            state: form.state.trim().to_string(),
            zip_code: form.zip_code.trim().to_string(),
            country: form.country.trim().to_string(),
            phone: form.phone.trim().to_string(),
            date_of_birth: form.date_of_birth.trim().to_string(),
        }
    }
}

/// What is wrong with a submitted profile, keyed by form field.
pub fn profile_errors(profile: &Profile) -> std::collections::BTreeMap<&'static str, &'static str> {
    let mut errors = std::collections::BTreeMap::new();

    if profile.first_name.is_empty() {
        errors.insert(
            "first_name",
            "Brokers match on your name, so this one is needed.",
        );
    }
    if profile.last_name.is_empty() {
        errors.insert(
            "last_name",
            "Brokers match on your name, so this one is needed.",
        );
    }
    if profile.email.is_empty() {
        errors.insert("email", "Brokers reply to this address, so it is needed.");
    } else if crate::email::validate_email(&profile.email).is_err() {
        errors.insert("email", "That does not look like an email address.");
    }

    errors
}

pub async fn save_profile(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let (session_id, _) = session_for(&state, &request);
    let profile: Profile = read_form::<ProfileForm>(request).await?.into();

    let errors = profile_errors(&profile);
    if !errors.is_empty() {
        // Re-render with what was typed, so nothing has to be retyped.
        let mut response = render(
            &state,
            csrf.as_ref(),
            "setup/profile.html",
            minijinja::context! {
                title => "Your details",
                step => "profile",
                profile => profile,
                errors => errors,
            },
        )?;
        attach_session(&mut response, &session_id);
        return Ok(response);
    }

    state.sessions.update(&session_id, |session| {
        session.profile = profile;
        session.step = "email".to_string();
    });

    let mut response = Redirect::to("/setup/email").into_response();
    attach_session(&mut response, &session_id);
    Ok(response)
}

pub async fn show_email(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let (session_id, session) = session_for(&state, &request);

    let mut response = render(
        &state,
        csrf.as_ref(),
        "setup/email.html",
        minijinja::context! {
            title => "Sending email",
            step => "email",
            // Deliberately not the password: it is never sent back to the
            // browser once it has been given.
            email => minijinja::context! {
                from => session.email.from,
                smtp => minijinja::context! {
                    host => session.email.smtp.host,
                    port => session.email.smtp.port,
                    username => session.email.smtp.username,
                },
            },
            profile => session.profile,
        },
    )?;
    attach_session(&mut response, &session_id);
    Ok(response)
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct EmailForm {
    pub from: String,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_username: String,
    pub smtp_password: String,
}

/// Build the email settings from the form, filling in Gmail's defaults.
pub fn email_config_from(form: &EmailForm) -> EmailConfig {
    let username = if form.smtp_username.trim().is_empty() {
        form.from.trim().to_string()
    } else {
        form.smtp_username.trim().to_string()
    };

    let host = if form.smtp_host.trim().is_empty() {
        GMAIL_SMTP_HOST.to_string()
    } else {
        form.smtp_host.trim().to_string()
    };

    let port = form.smtp_port.trim().parse().unwrap_or(GMAIL_SMTP_PORT);

    EmailConfig {
        provider: "smtp".to_string(),
        from: form.from.trim().to_string(),
        smtp: SmtpConfig {
            host,
            port,
            username,
            password: form.smtp_password.clone(),
            // Every provider worth using takes TLS, and the sender refuses
            // to send credentials without it.
            use_tls: true,
        },
    }
}

pub fn email_errors(
    config: &EmailConfig,
) -> std::collections::BTreeMap<&'static str, &'static str> {
    let mut errors = std::collections::BTreeMap::new();

    if config.from.is_empty() {
        errors.insert(
            "from",
            "Requests are sent from this address, so it is needed.",
        );
    } else if crate::email::validate_email(&config.from).is_err() {
        errors.insert("from", "That does not look like an email address.");
    }
    if config.smtp.password.is_empty() {
        errors.insert(
            "smtp_password",
            "An app password is needed. Your normal account password will not work.",
        );
    }
    if config.smtp.host.is_empty() {
        errors.insert("smtp_host", "Which mail server should this send through?");
    }
    if config.smtp.port == 0 {
        errors.insert("smtp_port", "465 for TLS, or 587 for STARTTLS.");
    }

    errors
}

pub async fn save_email(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let (session_id, _) = session_for(&state, &request);
    let form: EmailForm = read_form(request).await?;
    let email = email_config_from(&form);

    let errors = email_errors(&email);
    if !errors.is_empty() {
        let mut response = render(
            &state,
            csrf.as_ref(),
            "setup/email.html",
            minijinja::context! {
                title => "Sending email",
                step => "email",
                email => minijinja::context! {
                    from => email.from,
                    smtp => minijinja::context! {
                        host => email.smtp.host,
                        port => email.smtp.port,
                        username => email.smtp.username,
                    },
                },
                errors => errors,
            },
        )?;
        attach_session(&mut response, &session_id);
        return Ok(response);
    }

    state.sessions.update(&session_id, |session| {
        session.email = email;
        session.step = "test".to_string();
    });

    let mut response = Redirect::to("/setup/test").into_response();
    attach_session(&mut response, &session_id);
    Ok(response)
}

pub async fn show_test(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let (session_id, session) = session_for(&state, &request);

    let mut response = render(
        &state,
        csrf.as_ref(),
        "setup/test.html",
        minijinja::context! {
            title => "Check it works",
            step => "test",
            profile => session.profile,
            from => session.email.from,
        },
    )?;
    attach_session(&mut response, &session_id);
    Ok(response)
}

/// Send a message to the user's own address to prove the settings work.
///
/// Better to find out here than after 700 requests have silently failed.
pub async fn send_test(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let (session_id, session) = session_for(&state, &request);

    if session.email.from.is_empty() {
        return Err(WebError::BadRequest("Set up sending first.".into()));
    }

    let recipient = if session.profile.email.is_empty() {
        session.email.from.clone()
    } else {
        session.profile.email.clone()
    };

    let result = send_test_message(&session, &recipient).await;
    let (message, ok) = match &result {
        Ok(()) => (
            format!("Sent. Check {recipient} — it should be there in a moment."),
            true,
        ),
        Err(error) => (error.clone(), false),
    };

    let mut response = render(
        &state,
        csrf.as_ref(),
        "setup/test.html",
        minijinja::context! {
            title => "Check it works",
            step => "test",
            profile => session.profile,
            from => session.email.from,
            test_message => message,
            test_success => ok,
        },
    )?;
    attach_session(&mut response, &session_id);
    Ok(response)
}

async fn send_test_message(session: &Session, recipient: &str) -> Result<(), String> {
    let sender = new_sender(&session.email).map_err(|_| {
        "Those settings could not be used. Check the server address and port.".to_string()
    })?;

    let message = Message {
        to: recipient.to_string(),
        from: session.email.from.clone(),
        subject: "eruser is set up".to_string(),
        body: "This is the test message from eruser.\n\n\
               If you are reading it, sending works and you are ready to start \
               sending removal requests.\n"
            .to_string(),
    };

    sender.send(&message).await.map(|_| ()).map_err(|error| {
        // The classified message is safe to show; it names no credentials.
        format!("Could not send: {}", crate::send::error_chain(&error))
    })
}

/// Write the collected answers to disk and finish.
pub async fn complete(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let (session_id, session) = session_for(&state, &request);

    let mut config = Config {
        profile: session.profile.clone(),
        email: session.email.clone(),
        ..state.config().unwrap_or_default()
    };
    config.apply_defaults();

    // Refuse to write a config that cannot send; finishing the wizard with a
    // broken file just moves the failure somewhere less obvious.
    if let Err(problem) = config.validate() {
        return Err(WebError::BadRequest(format!(
            "Setup is not finished: {problem}"
        )));
    }

    state.save_config(config)?;
    // The session held the password; it is on disk now and has no further use.
    state.sessions.delete(&session_id);

    let mut response = render(
        &state,
        csrf.as_ref(),
        "setup/complete.html",
        minijinja::context! {
            title => "Ready",
            step => "complete",
            profile => session.profile,
            broker_count => state.brokers.brokers.len(),
        },
    )?;
    clear_session_cookie(&mut response);
    Ok(response)
}

/// The wizard's own landing page, which just redirects into step one.
pub async fn index() -> Redirect {
    Redirect::to("/setup/welcome")
}

/// Find this request's session, starting one if there is none.
fn session_for(state: &AppState, request: &Request) -> (String, Session) {
    let existing = request
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| crate::web::security::cookie_value(cookies, COOKIE_NAME));

    if let Some(id) = existing
        && let Some(session) = state.sessions.get(&id)
    {
        return (id, session);
    }

    let id = state.sessions.create();
    let session = state.sessions.get(&id).unwrap_or_default();
    (id, session)
}

fn attach_session(response: &mut Response, session_id: &str) {
    if let Ok(value) = HeaderValue::from_str(&cookie_header(COOKIE_NAME, session_id)) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn clear_session_cookie(response: &mut Response) {
    if let Ok(value) = HeaderValue::from_str(&format!(
        "{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
    )) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

#[cfg(test)]
mod tests;
