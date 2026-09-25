//! The letters: the request templates, read and edited.
//!
//! The three letters ship embedded in the binary and were, until now,
//! editable only by rebuilding. Here they can be read, edited, previewed
//! against a real broker, and sent to yourself as a test. An edit is stored
//! per person in the database and wins over the shipped copy until it is
//! reverted — so a wording fix in a release still reaches anyone who has not
//! gone out of their way to change things.

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};

use super::{Request, csrf_of, read_form, render};
use crate::broker::Broker;
use crate::config::Profile;
use crate::template::Engine;
use crate::web::auth::CurrentUser;
use crate::web::error::WebError;
use crate::web::state::AppState;

/// The letters, in the order the page lists them.
pub const LETTER_NAMES: &[&str] = &["generic", "ccpa", "gdpr"];

/// The shipped text of one letter, straight from the embedded templates.
fn shipped(engine: &Engine, name: &str) -> Option<(String, String)> {
    let body = engine.source_of(name)?;
    let subject = engine.subject_of(name).to_string();
    Some((subject, body.to_string()))
}

/// GET /letters — the list, plus the editor for the selected letter.
pub async fn letters(
    State(state): State<AppState>,
    user: CurrentUser,
    axum::extract::Query(selected): axum::extract::Query<LetterQuery>,
    request: Request,
) -> Result<Response, WebError> {
    let name = match selected.letter.as_str() {
        "" => "generic",
        other => LETTER_NAMES
            .iter()
            .copied()
            .find(|known| *known == other)
            .ok_or(WebError::NotFound)?,
    };

    letters_page(&state, &user, csrf_of(&request).as_ref(), name, None).await
}

/// Render the page, optionally with a message from a save or a revert.
async fn letters_page(
    state: &AppState,
    user: &CurrentUser,
    csrf: Option<&super::CsrfToken>,
    name: &str,
    message: Option<(&str, bool)>,
) -> Result<Response, WebError> {
    let engine = state.engine.clone();
    let stored = state
        .store
        .letter_override(user.id(), name)
        .await
        .unwrap_or(None);

    let (subject, body) = match &stored {
        Some(edit) => (edit.subject.clone(), edit.body.clone()),
        None => shipped(&engine, name)
            .ok_or_else(|| WebError::BadRequest(format!("{name} is not a letter.")))?,
    };

    // What the edit would change, for the "modified" flag in the list.
    let mut rows = Vec::new();
    for letter in LETTER_NAMES {
        let edited = state
            .store
            .letter_override(user.id(), letter)
            .await
            .unwrap_or(None)
            .is_some();
        let file = format!("{letter}.txt");
        let used = match *letter {
            "gdpr" => "brokers based in the EU or UK",
            "ccpa" => "US brokers",
            _ => "everyone the rules don't place",
        };
        rows.push(minijinja::context! {
            name => letter,
            file => file,
            used => used,
            edited => edited,
            selected => *letter == name,
        });
    }

    let (message_text, message_ok) = match message {
        Some((text, ok)) => (Some(text.to_string()), Some(ok)),
        None => (None, None),
    };

    render(
        state,
        user,
        csrf,
        "letters.html",
        minijinja::context! {
            title => "Letters",
            letters => rows,
            current => minijinja::context! {
                name => name,
                file => format!("{name}.txt"),
                subject => subject,
                body => body,
                edited => stored.is_some(),
                words => body.split_whitespace().count(),
            },
            message => message_text,
            message_ok => message_ok,
            test_recipient => user.config().profile.email,
        },
    )
}

/// What the edit form sends.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct LetterForm {
    pub subject: String,
    pub body: String,
}

/// POST /letters/{name} — store an edit.
pub async fn save_letter(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(name): Path<String>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    if !LETTER_NAMES.contains(&name.as_str()) {
        return Err(WebError::NotFound);
    }

    let form: LetterForm = read_form(request).await?;
    if form.body.trim().is_empty() {
        return letters_page(
            &state,
            &user,
            csrf.as_ref(),
            &name,
            Some((
                "The letter is empty. Write something, or revert to the shipped wording.",
                false,
            )),
        )
        .await;
    }

    state
        .store
        .save_letter_override(user.id(), &name, &form.subject, &form.body)
        .await?;

    Ok(Redirect::to(&format!("/letters?letter={name}")).into_response())
}

/// POST /letters/{name}/revert — forget an edit; the shipped letter returns.
pub async fn revert_letter(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(name): Path<String>,
) -> Result<Response, WebError> {
    if !LETTER_NAMES.contains(&name.as_str()) {
        return Err(WebError::NotFound);
    }
    state.store.delete_letter_override(user.id(), &name).await?;
    Ok(Redirect::to(&format!("/letters?letter={name}")).into_response())
}

/// GET /letters/{name}/preview?broker=acme — the letter as one broker gets it.
///
/// A preview renders against a real broker from the database, so what you
/// read here is byte-for-byte what goes on the wire (short of the SMTP
/// envelope).
pub async fn preview_letter(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<PreviewQuery>,
) -> Result<Response, WebError> {
    if !LETTER_NAMES.contains(&name.as_str()) {
        return Err(WebError::NotFound);
    }

    let broker = pick_preview_broker(
        &state,
        Some(query.broker.as_str()).filter(|id| !id.is_empty()),
    )?;
    let profile = user.config().profile.clone();
    let rendered =
        render_letter(&state.engine, user.id(), &name, &profile, &broker, &state).await?;

    let from = user.config().email.from.clone();

    render(
        &state,
        &user,
        None,
        "partials/letter-preview.html",
        minijinja::context! {
            title => format!("Preview: {name}.txt"),
            name => name,
            file => format!("{name}.txt"),
            to => broker.email,
            to_name => broker.name,
            from => from,
            subject => rendered.subject,
            paras => rendered.body.split("\n\n").map(str::to_string).collect::<Vec<_>>(),
            brokers => preview_broker_options(&state)?,
            selected_broker => broker.id,
        },
    )
}

/// POST /letters/{name}/test — send the letter to yourself.
///
/// Goes through the ordinary sender against the real broker data for one
/// sample broker, addressed to the person's own profile email.
pub async fn test_letter(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(name): Path<String>,
) -> Result<Response, WebError> {
    if !LETTER_NAMES.contains(&name.as_str()) {
        return Err(WebError::NotFound);
    }

    let config = user.config().clone();
    config.validate().map_err(|_| WebError::NotConfigured)?;

    let broker = pick_preview_broker(&state, None)?;
    let profile = config.profile.clone();
    let rendered =
        render_letter(&state.engine, user.id(), &name, &profile, &broker, &state).await?;

    let recipient = if profile.email.is_empty() {
        config.email.from.clone()
    } else {
        profile.email.clone()
    };

    let sender = crate::email::new_sender(&config.email)?;
    let message = crate::email::Message {
        to: recipient.clone(),
        from: config.email.from.clone(),
        subject: format!("[test] {}", rendered.subject),
        body: rendered.body,
    };

    sender.send(&message).await.map_err(|error| {
        WebError::BadRequest(format!(
            "Could not send: {}",
            crate::send::error_chain(&error)
        ))
    })?;

    Ok(Redirect::to(&format!("/letters?letter={name}")).into_response())
}

/// Render the letter named `name` for one broker, honouring an override.
///
/// `auto` resolves through the best-fit rule; that path is for the run, not
/// the editor, but the preview accepts it so what the run will send can be
/// checked letter by letter.
async fn render_letter(
    engine: &Engine,
    user_id: i64,
    name: &str,
    profile: &Profile,
    broker: &Broker,
    state: &AppState,
) -> Result<crate::template::Email, WebError> {
    let stored = state
        .store
        .letter_override(user_id, name)
        .await
        .unwrap_or(None);

    match stored {
        Some(edit) => {
            Ok(engine.render_override(name, &edit.subject, &edit.body, profile, broker)?)
        }
        None => Ok(engine.render(name, profile, broker)?),
    }
}

/// The broker a preview or test renders against.
///
/// The asked-for one, or the first broker in the database — deterministic, so
/// the same page always shows the same sample until someone picks another.
fn pick_preview_broker(state: &AppState, broker_id: Option<&str>) -> Result<Broker, WebError> {
    if let Some(id) = broker_id {
        return state
            .brokers
            .find_by_id(id)
            .cloned()
            .ok_or(WebError::NotFound);
    }
    state
        .brokers
        .brokers
        .first()
        .cloned()
        .ok_or_else(|| WebError::BadRequest("The broker database is empty.".into()))
}

/// A handful of brokers for the preview picker: three, covering the regions.
fn preview_broker_options(state: &AppState) -> Result<Vec<minijinja::Value>, WebError> {
    let mut picked: Vec<&Broker> = Vec::new();
    for region in ["us", "eu", "global"] {
        if let Some(broker) = state
            .brokers
            .brokers
            .iter()
            .find(|b| b.region == region && !picked.iter().any(|p| p.id == b.id))
        {
            picked.push(broker);
        }
    }
    // Top up from whatever exists if the regions are thin.
    for broker in &state.brokers.brokers {
        if picked.len() >= 3 {
            break;
        }
        if !picked.iter().any(|p| p.id == broker.id) {
            picked.push(broker);
        }
    }

    Ok(picked
        .into_iter()
        .map(|b| {
            minijinja::context! {
                id => b.id.clone(),
                name => b.name.clone(),
            }
        })
        .collect())
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct LetterQuery {
    pub letter: String,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct PreviewQuery {
    pub broker: String,
}
