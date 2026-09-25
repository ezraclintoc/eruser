//! The captchas page: what is stuck, and how stuck things get solved.
//!
//! The fork's rule is kept in view here: a solver is an optimisation, never
//! a gate. The settings shown are the machine's `pipeline.captcha_solver`
//! from config.yaml — a sidecar the user runs, not something eruser ships.

use axum::extract::{Request, State};
use axum::response::Response;

use super::{csrf_of, render};
use crate::history::{FormStatus, TaskFilter, TaskStatus, TaskType};
use crate::web::auth::CurrentUser;
use crate::web::error::WebError;
use crate::web::state::AppState;

/// GET /captchas — the waiting list, plus the solver settings.
pub async fn captchas(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    if let Some(redirect) = super::require_setup(&user) {
        return Ok(redirect);
    }

    // Every pending captcha task, newest first.
    let mut captchas: Vec<_> = state
        .store
        .tasks(
            user.id(),
            TaskFilter {
                task_type: Some(TaskType::Captcha),
                status: Some(TaskStatus::Pending),
            },
        )
        .await?
        .into_iter()
        .map(|task| {
            minijinja::context! {
                id => task.id,
                broker => task.broker_name,
                instructions => if task.notes.is_empty() {
                    "The form needs its challenge solved before it will accept the request."
                } else {
                    task.notes.as_str()
                },
                form_url => task.form_url,
                helper_url => format!("/tasks/{}/helper", task.id),
            }
        })
        .collect();
    captchas.reverse();

    // Forms stuck at the captcha stage count here too: the fill ran, the
    // challenge stopped it, and the screenshot shows what it looked like.
    let stuck_forms: Vec<_> = state
        .store
        .forms_with_status(user.id())
        .await?
        .into_iter()
        .filter(|form| form.status == FormStatus::Captcha)
        .map(|form| {
            minijinja::context! {
                id => form.task_id,
                broker => form.broker_name,
                instructions => format!(
                    "The fill reached the form and stopped at the challenge. {}",
                    if form.form_url.is_empty() { String::new() } else { format!("Form: {}", form.form_url) }
                ),
                form_url => form.form_url,
                helper_url => if form.task_id > 0 {
                    format!("/tasks/{}/helper", form.task_id)
                } else {
                    String::new()
                },
            }
        })
        .collect();

    let solver = state.pipeline().captcha_solver;

    render(
        &state,
        &user,
        csrf_of(&request).as_ref(),
        "captchas.html",
        minijinja::context! {
            title => "Captchas",
            captchas => captchas,
            stuck_forms => stuck_forms,
            waiting => captchas.len() + stuck_forms.len(),
            solver_enabled => solver.enabled,
            solver_url => solver.url,
            solver_timeout => solver.timeout_sec,
        },
    )
}
