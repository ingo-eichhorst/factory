//! `agent.start`, `agent.stop`, `agent.status`, `agent.attach_command`
//! (design §2.2, §2.3, §7).

use factory_adapter::PaneId;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::errors;
use crate::handler::FactoryHandler;
use crate::server::HandlerOutcome;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartPayload {
    #[serde(with = "crate::serde_uuid::required")]
    session_id: uuid::Uuid,
    agent_name: String,
    #[serde(default)]
    workspace_path: Option<String>,
}

/// Design §2.3: record `starting` before launch (`factory_session::begin_start`),
/// compile this session's context (design §2.5), start the harness
/// (`Adapter::start`), persist its pane (see `handler::pane`'s module docs),
/// and promote to `running` once `Adapter::start`'s own observation is
/// [`factory_adapter::Confidence::Authoritative`] — otherwise the session
/// stays `starting` for `crate::observe`'s loop to confirm later, per
/// `StartedSession::confidence`'s own doc comment ("falls back to manual
/// confirmation").
///
/// A failure compiling context or starting the harness fails the session
/// (`factory_session::fail`) rather than leaving a `starting` row holding a
/// workspace lease with nothing behind it.
pub(crate) fn start(h: &FactoryHandler, scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: StartPayload = errors::parse_payload(&payload)?;

    let config = crate::handler::config::load(h.instance_root())?;
    let agent = crate::handler::config::find_agent(&config, scope_id, &payload.agent_name)?;

    let mut store = h.lock_store();

    let workspace = match &payload.workspace_path {
        Some(p) => factory_paths::CanonicalPath::resolve(p)
            .map_err(|e| errors::err("validation.workspace_not_found", e.to_string()))?,
        None => {
            let scope_row = crate::handler::scope_chain::row(&store, scope_id)?;
            let canonical_path = scope_row.canonical_path.ok_or_else(|| {
                errors::err(
                    "unavailable.scope_path_missing",
                    format!("scope `{scope_id}` has no currently-resolvable canonical path"),
                )
            })?;
            factory_paths::CanonicalPath::resolve(&canonical_path)
                .map_err(|e| errors::err("internal.path_error", e.to_string()))?
        }
    };

    factory_session::begin_start(
        &mut store,
        payload.session_id,
        scope_id,
        &payload.agent_name,
        agent.max_sessions,
        &workspace,
    )
    .map_err(errors::session_error)?;

    let scope_contexts = match crate::ops::context::compile_scope_chain(&store, scope_id) {
        Ok(chain) => chain,
        Err(e) => return fail_started_session(&mut store, payload.session_id, e),
    };
    let compiled = match factory_context::compile(&scope_contexts, agent, None) {
        Ok(c) => c,
        Err(e) => {
            return fail_started_session(&mut store, payload.session_id, errors::context_error(e));
        }
    };

    let started = match h.adapter().start(&factory_adapter::StartRequest {
        scope_id,
        session_id: payload.session_id,
        workspace: workspace.clone(),
        generated_context: compiled.text,
    }) {
        Ok(started) => started,
        Err(e) => {
            return fail_started_session(&mut store, payload.session_id, errors::adapter_error(e));
        }
    };

    crate::handler::pane::record(
        &mut store,
        payload.session_id,
        &started.pane.0,
        started.harness_session_id.as_deref(),
    )?;

    let state = if started.confidence == factory_adapter::Confidence::Authoritative {
        factory_session::mark_running(&mut store, payload.session_id)
            .map_err(errors::session_error)?;
        "running"
    } else {
        "starting"
    };

    h.success(
        json!({
            "session_id": payload.session_id.to_string(),
            "state": state,
            "pane": started.pane.0,
            "confidence": confidence_str(started.confidence),
            "harness_session_id": started.harness_session_id,
        }),
        true,
    )
}

/// A `starting` session with nothing usable behind it — a context compile or
/// an `Adapter::start` failure — is failed rather than left holding a lease
/// forever. Best-effort: if `factory_session::fail` itself errors (the
/// session already left `starting` some other way), the *original* error is
/// still what reaches the caller, since that is what actually went wrong.
fn fail_started_session(
    store: &mut factory_store::Store,
    session_id: uuid::Uuid,
    original: crate::envelope::ErrorBody,
) -> HandlerOutcome {
    let _ = factory_session::fail(
        store,
        session_id,
        &format!(
            "agent.start could not bring up a harness: {}",
            original.message
        ),
    );
    Err(original)
}

fn confidence_str(c: factory_adapter::Confidence) -> &'static str {
    use factory_adapter::Confidence as C;
    match c {
        C::Authoritative => "authoritative",
        C::Degraded => "degraded",
        C::Unavailable => "unavailable",
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopPayload {
    #[serde(with = "crate::serde_uuid::required")]
    session_id: uuid::Uuid,
    #[serde(default)]
    reason: Option<String>,
}

/// `Adapter::stop` first, confirming the process is gone before
/// `factory_session::stop`'s own documented precondition is asked to hold —
/// then the session-side write: `factory_session::interrupt` if a `running`
/// task is still assigned (folding it into `blocked: interrupted` atomically
/// with the session's move to `failed`), otherwise a plain
/// `factory_session::stop`.
pub(crate) fn stop(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: StopPayload = errors::parse_payload(&payload)?;

    let mut store = h.lock_store();

    let (pane, _) = crate::handler::pane::read(&store, payload.session_id)?;
    if let Some(pane) = pane {
        h.adapter()
            .stop(&PaneId(pane))
            .map_err(errors::adapter_error)?;
    }

    let running_task = factory_task::assign::running_task_of_session(&store, payload.session_id)
        .map_err(errors::task_error)?;

    let (state, interrupted_task_id) = match running_task {
        Some(task_id) => {
            factory_session::interrupt(&mut store, payload.session_id, task_id)
                .map_err(errors::session_error)?;
            ("failed", Some(task_id))
        }
        None => {
            let reason = payload.reason.as_deref().unwrap_or("stopped by operator");
            factory_session::stop(&mut store, payload.session_id, reason)
                .map_err(errors::session_error)?;
            ("stopped", None)
        }
    };

    h.success(
        json!({
            "session_id": payload.session_id.to_string(),
            "state": state,
            "interrupted_task_id": interrupted_task_id.map(|id: uuid::Uuid| id.to_string()),
        }),
        true,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusPayload {
    #[serde(default, with = "crate::serde_uuid::optional")]
    session_id: Option<uuid::Uuid>,
}

pub(crate) fn status(h: &FactoryHandler, scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: StatusPayload = errors::parse_payload(&payload)?;
    let store = h.lock_store();

    if let Some(session_id) = payload.session_id {
        let session = factory_session::show(&store, session_id).map_err(errors::session_error)?;
        let leases = factory_session::leases_of_session(&store, session_id)
            .map_err(errors::session_error)?;
        return h.success(
            json!({
                "session": session_json(&session),
                "leases": leases.iter().map(lease_json).collect::<Vec<_>>(),
            }),
            false,
        );
    }

    let sessions = factory_session::list(&store).map_err(errors::session_error)?;
    let sessions: Vec<Value> = sessions
        .iter()
        .filter(|s| s.scope_id == scope_id)
        .map(session_json)
        .collect();
    h.success(json!({ "sessions": sessions }), false)
}

pub(crate) fn attach_command(
    h: &FactoryHandler,
    _scope_id: uuid::Uuid,
    payload: Value,
) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {
        #[serde(with = "crate::serde_uuid::required")]
        session_id: uuid::Uuid,
    }
    let payload: Payload = errors::parse_payload(&payload)?;

    let store = h.lock_store();
    let (pane, _) = crate::handler::pane::read(&store, payload.session_id)?;
    let Some(pane) = pane else {
        return Err(errors::err(
            "not_found.pane",
            format!(
                "session {} has no recorded pane to attach to",
                payload.session_id
            ),
        ));
    };
    drop(store);

    let argv = h
        .adapter()
        .attach_command(&PaneId(pane))
        .map_err(errors::adapter_error)?;
    h.success(json!({ "argv": argv }), false)
}

pub(crate) fn session_json(s: &factory_session::Session) -> Value {
    json!({
        "id": s.id.to_string(),
        "scope_id": s.scope_id.to_string(),
        "agent_name": s.agent_name,
        "state": s.state.to_string(),
        "workspace_path": s.workspace_path,
        "created_at": s.created_at,
        "updated_at": s.updated_at,
    })
}

fn lease_json(l: &factory_session::WorkspaceLease) -> Value {
    json!({
        "id": l.id,
        "canonical_workspace_path": l.canonical_workspace_path,
        "acquired_at": l.acquired_at,
        "released_at": l.released_at,
        "release_reason": l.release_reason,
    })
}
