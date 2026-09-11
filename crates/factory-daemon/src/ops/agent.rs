//! `agent.start`, `agent.stop`, `agent.status`, `agent.attach_command`,
//! `agent.list` (design §2.2, §2.3, §7; `agent.list` is ADR 0022).

use factory_adapter::PaneId;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::envelope::ErrorBody;
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
        // Straight off the scope's declaration. The adapter passes it to the
        // harness and nothing here inspects it — ADR 0024.
        model: agent.model.clone(),
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListPayload {
    /// `--session`. Resolved through `factory_session::scope_of_session` —
    /// the same function `factory_delegation::queue::queue_from_session`
    /// uses to enforce §6 — so this answer and the one enforcement gives
    /// cannot drift apart (ADR 0022 decision 7).
    #[serde(default, with = "crate::serde_uuid::optional")]
    session_id: Option<uuid::Uuid>,
    /// `--scope`. An openly hypothetical caller with no live session behind
    /// it — "what would this scope be allowed to target."
    #[serde(default, with = "crate::serde_uuid::optional")]
    scope_id: Option<uuid::Uuid>,
}

/// `agent.list` deliberately does not read the envelope's own `scope_id` —
/// unlike every other `agent.*` operation, which uses it as *the* scope the
/// call concerns, `agent.list` has to tell "the caller wrote `--scope`" apart
/// from "the CLI sent a scope id because the wire format requires one," and
/// only a dedicated payload field can say that (`ops::schedule::resolve_form`
/// takes the identical stance for `schedule.create`'s two forms).
///
/// # Errors
///
/// `validation.conflicting_fields` if both `session_id` and `scope_id` are
/// given; `validation.missing_field` if neither is (ADR 0022 decision 7:
/// "neither, or both, is refused").
fn resolve_caller(payload: &ListPayload) -> Result<CallerRef, ErrorBody> {
    match (payload.session_id, payload.scope_id) {
        (Some(_), Some(_)) => Err(errors::err(
            "validation.conflicting_fields",
            "give exactly one of `session_id` (--session) or `scope_id` (--scope), not both",
        )),
        (None, None) => Err(errors::err(
            "validation.missing_field",
            "give exactly one of `session_id` (--session) or `scope_id` (--scope)",
        )),
        (Some(session_id), None) => Ok(CallerRef::Session(session_id)),
        (None, Some(scope_id)) => Ok(CallerRef::Scope(scope_id)),
    }
}

enum CallerRef {
    Session(uuid::Uuid),
    Scope(uuid::Uuid),
}

/// `factory agent list` (ADR 0022 decisions 7, 8, 9).
///
/// Lists every registered scope and, for each, the agents its instance
/// configuration declares — including a scope with no agent config entry at
/// all (an empty `agents` list, never an omitted scope) and an agent with no
/// live session (an availability of `"no_session"`, never an omitted agent —
/// §12's own acceptance criterion).
///
/// `targetable` is decided entirely by [`factory_delegation::rule::check`],
/// called with [`factory_delegation::rule::Sender::Agent`] and an *empty*
/// delegation chain — this operation is answering "could this scope reach
/// that one at all," never "has it already, in some particular task" (decision
/// 8: reused, never reimplemented, and always as an agent sender regardless
/// of whether the caller named itself by `--session` or `--scope`, because the
/// question is the same hypothetical either way).
pub(crate) fn list(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: ListPayload = errors::parse_payload(&payload)?;
    let caller = resolve_caller(&payload)?;

    let store = h.lock_store();
    let caller_scope_id = match caller {
        CallerRef::Session(session_id) => {
            factory_session::scope_of_session(&store, session_id).map_err(errors::session_error)?
        }
        CallerRef::Scope(scope_id) => scope_id,
    };

    let config = crate::handler::config::load(h.instance_root())?;
    let sessions = factory_session::list(&store).map_err(errors::session_error)?;

    let mut stmt = store
        .connection()
        .prepare("SELECT id, name FROM scopes ORDER BY name, id")
        .map_err(errors::store_error)?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            Ok((id, name))
        })
        .map_err(errors::store_error)?;
    let mut scope_rows = Vec::new();
    for row in rows {
        scope_rows.push(row.map_err(errors::store_error)?);
    }
    drop(stmt);

    let mut scopes_json = Vec::new();
    for (id, name) in scope_rows {
        let target_scope_id = uuid::Uuid::parse_str(&id)
            .unwrap_or_else(|e| panic!("scopes.id is a UUID; read {id:?}: {e}"));

        // Decision 8: this crate never reimplements §6 and never reads
        // kinship itself — it calls the one function that already holds the
        // rule, with an empty chain, because this is an unconditional
        // "could it ever," not "has it already, on this task."
        let check_result = factory_delegation::rule::check(
            store.connection(),
            factory_delegation::rule::Sender::Agent {
                scope_id: caller_scope_id,
            },
            target_scope_id,
            &[],
        );
        let (targetable, refusal_reason) = match check_result {
            Ok(()) => (true, None),
            // `NotEligible` gets its own wording: this row already carries
            // the target's `scope_name` a few lines down, and the caller
            // (whoever passed `--session`/`--scope`) already knows who it
            // is, so a reader should not have to cross-reference either
            // scope's raw id to understand a listing that already shows
            // both names (station 12 drill, defect 4). `kinship` comes
            // straight off the error `rule::check` returned above — this
            // never asks `factory_registry::kinship` a second time, which
            // would make this function a second place that decides
            // targetability rather than merely wording it (ADR 0022
            // decision 8).
            Err(factory_delegation::rule::DelegationError::NotEligible { kinship, .. }) => {
                (false, Some(listing_refusal_reason(kinship).to_string()))
            }
            // `AlreadyInChain`, `Registry`, `Session`, and `Task` cannot
            // actually happen here — the chain is always empty and
            // `target_scope_id` is drawn straight from the `scopes` table —
            // except that `caller_scope_id` can itself be an unregistered id
            // when the caller passed a hypothetical `--scope`
            // (`resolve_caller`'s own doc comment), which surfaces as
            // `DelegationError::Registry`. That case is not this defect's
            // concern, so it keeps the raw error text rather than growing
            // its own wording.
            Err(e) => (false, Some(e.to_string())),
        };

        let agents_json = match config.scopes.iter().find(|s| s.id == target_scope_id) {
            Some(entry) => {
                let mut agents = Vec::with_capacity(entry.agents.len());
                for agent in &entry.agents {
                    let availability =
                        agent_availability(&store, &sessions, target_scope_id, &agent.name)
                            .map_err(errors::task_error)?;
                    agents.push(json!({
                        "name": agent.name,
                        "harness": agent.harness.as_str(),
                        "max_sessions": agent.max_sessions,
                        "lifetime": lifetime_str(agent.lifetime),
                        "availability": availability,
                        // `null` is not "no model" but "whatever the harness
                        // is configured for" — the state every agent was in
                        // before this key existed (ADR 0024).
                        "model": agent.model,
                    }));
                }
                agents
            }
            // Drift between `config.yaml` and `scopes` (a registered scope
            // whose config entry vanished) is `scope.reconcile`'s question,
            // not this one's — an empty agent list, not a refusal.
            None => Vec::new(),
        };

        scopes_json.push(json!({
            "scope_id": target_scope_id.to_string(),
            "scope_name": name,
            "targetable": targetable,
            "refusal_reason": refusal_reason,
            "agents": agents_json,
        }));
    }

    h.success(json!({ "scopes": scopes_json }), false)
}

/// [`factory_registry::Kinship`]'s three refused variants, worded for one
/// row of `agent.list`'s own listing (station 12 drill, defect 4) — never
/// used for `task send`'s refusal, which has no listing to lean on and so
/// keeps `factory_delegation::rule::DelegationError`'s own id-based
/// `Display` (`errors::delegation_error`).
///
/// Neither scope's id, nor even its name, appears here: the row this
/// reason sits in already carries the target's `scope_name`, and "the
/// calling scope" is unambiguous because a listing only ever answers on
/// behalf of one caller.
fn listing_refusal_reason(kinship: factory_registry::Kinship) -> &'static str {
    use factory_registry::Kinship;
    match kinship {
        Kinship::SameScope => "the calling scope itself",
        Kinship::Ancestor => "an ancestor of the calling scope",
        Kinship::Unrelated => {
            "unrelated to the calling scope — a nephew or cousin, reached through its parent"
        }
        Kinship::Descendant | Kinship::Sibling => unreachable!(
            "rule::check only returns NotEligible for a kinship it refuses, and Descendant/Sibling are never refused"
        ),
    }
}

fn lifetime_str(l: factory_config::Lifetime) -> &'static str {
    match l {
        factory_config::Lifetime::Permanent => "permanent",
        factory_config::Lifetime::Temporary => "temporary",
    }
}

/// One agent's availability, derived from every session recorded for
/// `(scope_id, agent_name)` (ADR 0022 decision 9).
///
/// An agent can hold more than one live session at once (`max_sessions` can
/// exceed 1), so this is not a single session's state read straight off —
/// it is the answer to one question, asked across the whole set: **can a
/// caller send work here right now?** Every rank below follows from that
/// question, not from an arbitrary ordering of the five words:
///
/// 1. any `running` session with no running task — `"idle"`. Yes, right
///    now — this ranks first regardless of what the agent's *other*
///    sessions are doing, because a session sitting idle takes work
///    immediately. Ranking `"busy"` above this would report an agent as
///    unavailable while a free session waits — the exact failure design
///    §7 warns against ("a caller does not queue work behind a session
///    that will not free up"), reproduced by the ranking meant to prevent
///    it.
/// 2. else any `starting` session — `"starting"`. Not yet, but sooner than
///    a `running` one already mid-task: it will free up on its own, where
///    a busy session's task has no promised end.
/// 3. else any `running` session with a running task — `"busy"`. Not now,
///    on a known fact.
/// 4. else any `disconnected` session — `"unknown"`, never `"idle"`: the
///    observer has lost sight of it, and reporting `"idle"` is the queue-
///    behind-a-stuck-session failure again, this time for a fact nobody
///    actually has (ADR 0017, ADR 0022 decision 9). `"unknown"` outranks
///    `"no_session"` for the same reason it stays below `"busy"`: a known
///    fact beats an unknown one, and an unknown one beats claiming there is
///    nothing there.
/// 5. else — no session at all, or only `stopped`/`failed` — `"no_session"`.
///
/// An agent with no session at all still reaches step 5 and gets an answer,
/// never an omission (§12's own acceptance criterion: "listed with its
/// availability stated, never omitted").
fn agent_availability(
    store: &factory_store::Store,
    sessions: &[factory_session::Session],
    scope_id: uuid::Uuid,
    agent_name: &str,
) -> Result<&'static str, factory_task::TaskError> {
    let mut any_busy = false;
    let mut any_idle = false;
    let mut any_starting = false;
    let mut any_disconnected = false;

    for session in sessions
        .iter()
        .filter(|s| s.scope_id == scope_id && s.agent_name == agent_name)
    {
        match session.state {
            factory_session::SessionState::Running => {
                let running_task =
                    factory_task::assign::running_task_of_session(store, session.id)?;
                if running_task.is_some() {
                    any_busy = true;
                } else {
                    any_idle = true;
                }
            }
            factory_session::SessionState::Starting => any_starting = true,
            factory_session::SessionState::Disconnected => any_disconnected = true,
            factory_session::SessionState::Stopped | factory_session::SessionState::Failed => {}
        }
    }

    Ok(if any_idle {
        "idle"
    } else if any_starting {
        "starting"
    } else if any_busy {
        "busy"
    } else if any_disconnected {
        "unknown"
    } else {
        "no_session"
    })
}
