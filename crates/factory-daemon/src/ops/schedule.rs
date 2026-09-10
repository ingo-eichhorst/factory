//! `schedule.create`, `schedule.list`, `schedule.enable`, `schedule.disable`
//! (design §7, §11; ADR 0021 decisions 1, 3, 3a, 11).
//!
//! This module invents no cron or template rule of its own — `lib.rs`'s
//! own doc comment on `ops` says why: every function here parses its
//! payload and wires straight into `factory_task::schedule` and
//! `factory_task::template`, which already hold the rules ADR 0021 settled.
//!
//! # The two forms of `schedule.create`
//!
//! ADR 0021 decision 11: `create` takes exactly one of `template_name`
//! (`--template <name>`, an existing template) or `template_id`+`name`+
//! `task` (`--task "<prompt>" --name <template-name>`, a new one), and
//! refuses both given or neither. [`resolve_form`] is that check, done here
//! rather than left to the CLI — the same shape `ops::task::send` already
//! uses for its own `agent_name`/`target_session_id` choice, so a raw
//! JSON-RPC client that bypasses the CLI's flags is held to the identical
//! rule instead of trusting the client to have made it first.
//!
//! # `scope_id` is read only by the new-template form
//!
//! The positional `<scope>` `factory schedule create` always takes is sent
//! as every command's own `scope_id` (`lib.rs`'s decision 9). This module
//! reads it only when [`Form::New`] creates a template — a template's own
//! `target_scope_id` is what a run is ever created against
//! (`factory_task::template`'s own doc comment on that column), so a
//! schedule created against an *existing* `--template` runs in whatever
//! scope that template already names, not whatever the operator typed
//! after `factory schedule create`. See `lib.rs`'s payload table for the
//! same note against `schedule.create`'s own row.

use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::envelope::ErrorBody;
use crate::errors;
use crate::handler::FactoryHandler;
use crate::server::HandlerOutcome;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePayload {
    #[serde(with = "crate::serde_uuid::required")]
    schedule_id: uuid::Uuid,
    cron: String,
    timezone: String,
    /// `--template <name>`.
    #[serde(default)]
    template_name: Option<String>,
    /// `--task`/`--name`'s own new template id — minted by the CLI the same
    /// way `task.send`'s own `task_id` is, so the caller can report it
    /// without a second read even before this command answers.
    #[serde(default, with = "crate::serde_uuid::optional")]
    template_id: Option<uuid::Uuid>,
    /// `--task`'s prompt for the new template.
    #[serde(default)]
    task: Option<String>,
    /// `--name`'s new template name.
    #[serde(default)]
    name: Option<String>,
    /// `--agent`, the new template's `target_agent_name`. Read only by
    /// [`Form::New`] — an existing `--template` already has its own.
    #[serde(default)]
    agent_name: Option<String>,
    /// `--acceptance`, the new template's `acceptance_criteria`. Read only
    /// by [`Form::New`], for the identical reason.
    #[serde(default)]
    acceptance_criteria: Option<String>,
}

/// See this module's own doc comment, "The two forms of `schedule.create`."
enum Form {
    Existing {
        template_name: String,
    },
    New {
        template_id: uuid::Uuid,
        name: String,
        task: String,
    },
}

fn resolve_form(payload: &CreatePayload) -> Result<Form, ErrorBody> {
    let has_existing = payload.template_name.is_some();
    let has_new = payload.template_id.is_some() || payload.task.is_some() || payload.name.is_some();
    match (has_existing, has_new) {
        (true, true) => Err(errors::err(
            "validation.conflicting_fields",
            "give exactly one of `template_name` (--template) or `template_id`+`task`+`name` \
             (--task/--name), not both",
        )),
        (false, false) => Err(errors::err(
            "validation.missing_field",
            "give either `template_name` (--template) or `task`+`name` (--task/--name)",
        )),
        (true, false) => Ok(Form::Existing {
            template_name: payload
                .template_name
                .clone()
                .expect("has_existing just checked this is Some"),
        }),
        (false, true) => Ok(Form::New {
            template_id: payload
                .template_id
                .ok_or_else(|| errors::missing_field("template_id"))?,
            name: payload
                .name
                .clone()
                .ok_or_else(|| errors::missing_field("name"))?,
            task: payload
                .task
                .clone()
                .ok_or_else(|| errors::missing_field("task"))?,
        }),
    }
}

pub(crate) fn create(h: &FactoryHandler, scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: CreatePayload = errors::parse_payload(&payload)?;
    let form = resolve_form(&payload)?;

    let mut store = h.lock_store();
    match form {
        Form::Existing { template_name } => {
            factory_task::schedule::create_for_named_template(
                &mut store,
                payload.schedule_id,
                &template_name,
                &payload.cron,
                &payload.timezone,
            )
            .map_err(errors::schedule_error)?;
        }
        Form::New {
            template_id,
            name,
            task,
        } => {
            factory_task::schedule::create_with_new_template(
                &mut store,
                payload.schedule_id,
                template_id,
                &name,
                scope_id,
                payload.agent_name.as_deref(),
                &task,
                payload.acceptance_criteria.as_deref(),
                &payload.cron,
                &payload.timezone,
            )
            .map_err(errors::schedule_error)?;
        }
    }

    let schedule =
        factory_task::schedule::get(&store, payload.schedule_id).map_err(errors::schedule_error)?;
    h.success(schedule_json(&store, &schedule, Utc::now()), true)
}

pub(crate) fn list(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {}
    let _: Payload = errors::parse_payload(&payload)?;

    let store = h.lock_store();
    let schedules = factory_task::schedule::list(&store).map_err(errors::schedule_error)?;
    let now = Utc::now();
    let rendered: Vec<Value> = schedules
        .iter()
        .map(|s| schedule_json(&store, s, now))
        .collect();
    h.success(json!({ "schedules": rendered }), false)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TogglePayload {
    #[serde(with = "crate::serde_uuid::required")]
    schedule_id: uuid::Uuid,
}

/// Backlog §11: `enable`/`disable` "toggle a schedule without deleting its
/// history or affecting already-created runs." Both here are exactly
/// `factory_task::schedule::enable`/`::disable` plus a read-back to render
/// — see that module's own doc comment for why `cron`, `timezone`,
/// `last_fired_at`, and `created_at` are untouched by either.
pub(crate) fn enable(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: TogglePayload = errors::parse_payload(&payload)?;
    let mut store = h.lock_store();
    factory_task::schedule::enable(&mut store, payload.schedule_id)
        .map_err(errors::schedule_error)?;
    let schedule =
        factory_task::schedule::get(&store, payload.schedule_id).map_err(errors::schedule_error)?;
    h.success(schedule_json(&store, &schedule, Utc::now()), true)
}

pub(crate) fn disable(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: TogglePayload = errors::parse_payload(&payload)?;
    let mut store = h.lock_store();
    factory_task::schedule::disable(&mut store, payload.schedule_id)
        .map_err(errors::schedule_error)?;
    let schedule =
        factory_task::schedule::get(&store, payload.schedule_id).map_err(errors::schedule_error)?;
    h.success(schedule_json(&store, &schedule, Utc::now()), true)
}

/// Render one schedule the way `factory schedule list` needs it: its
/// template (by name, not just id), its cron and timezone exactly as the
/// operator typed them, whether it is enabled, its last run, and its next
/// run — the backlog's own acceptance criterion for `list`, and shared by
/// `create`/`enable`/`disable` so all four operations describe a schedule
/// identically.
///
/// Infallible by design: one hand-edited or otherwise broken row must not
/// fail the whole listing. Mirrors `factory_task::schedule::due`'s own
/// stance for the fire path — "[o]ne unreadable schedule must not silence
/// every other one" — applied here to the read path instead. A template
/// that cannot be read, or a `cron`/`timezone` pair that no longer parses,
/// is reported in place rather than aborting the query.
fn schedule_json(
    store: &factory_store::Store,
    schedule: &factory_task::schedule::Schedule,
    now: chrono::DateTime<Utc>,
) -> Value {
    let (template_name, template_error) =
        match factory_task::template::get_by_id(store, schedule.template_id) {
            Ok(template) => (Some(template.name), None),
            Err(e) => (None, Some(errors::template_error(e).message)),
        };

    // `next_run` stays a plain, machine-parseable RFC3339 string or `null`,
    // never a prose fallback: ADR 0021 decision 3a's `None` means one thing
    // only, "this expression can never fire," and folding "disabled" or
    // "unreadable" into that same field would destroy the distinction the
    // 1500-day horizon exists to preserve. `next_run_state` is what a
    // reader — and `factory schedule list`'s own rendering — branches on to
    // get something readable for the cases that are not a timestamp; the
    // backlog's own acceptance criterion is about that rendering, not about
    // the wire shape underneath it.
    let (next_run, next_run_state, next_run_error) = if !schedule.enabled {
        (None, "disabled", None)
    } else {
        match factory_task::schedule::next_run(&schedule.cron, &schedule.timezone, now) {
            Ok(Some(at)) => (Some(at.to_rfc3339()), "scheduled", None),
            Ok(None) => (None, "never", None),
            Err(e) => (None, "unreadable", Some(e.to_string())),
        }
    };

    json!({
        "id": schedule.id.to_string(),
        "template_id": schedule.template_id.to_string(),
        "template_name": template_name,
        "template_error": template_error,
        "cron": schedule.cron,
        "timezone": schedule.timezone,
        "enabled": schedule.enabled,
        "last_fired_at": schedule.last_fired_at,
        "next_run": next_run,
        "next_run_state": next_run_state,
        "next_run_error": next_run_error,
    })
}
