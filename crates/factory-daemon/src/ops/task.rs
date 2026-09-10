//! `task.send`, `task.cancel`, `task.done`, `task.fail`, `task.block`,
//! `task.resume`, `task.list`, `task.show`, `task.wait` (design §2.4, §5,
//! §7).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::envelope::ErrorBody;
use crate::errors;
use crate::handler::FactoryHandler;
use crate::server::HandlerOutcome;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendPayload {
    #[serde(with = "crate::serde_uuid::required")]
    task_id: uuid::Uuid,
    prompt: String,
    #[serde(default, with = "crate::serde_uuid::optional")]
    sender_session_id: Option<uuid::Uuid>,
    #[serde(default, with = "crate::serde_uuid::optional")]
    target_session_id: Option<uuid::Uuid>,
    #[serde(default)]
    target_workspace_path: Option<String>,
    #[serde(default)]
    agent_name: Option<String>,
}

/// Create (design §5 step 1, through `factory_delegation`'s two entry
/// points) and immediately attempt assignment and delivery (design §5 steps
/// 2–4) — station 10 has no background dispatcher (that is backlog slice
/// 11), so this is the whole of "sending" a task.
///
/// `agent_name` is required unless `target_session_id` is given — see
/// `lib.rs`'s payload table for why: `factory_task::assign`'s own module
/// docs record that `tasks` carries no agent-name column at all, so nothing
/// durable resolves which agent an untargeted or workspace-targeted send is
/// for.
pub(crate) fn send(h: &FactoryHandler, scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: SendPayload = errors::parse_payload(&payload)?;

    if payload.target_session_id.is_none() && payload.agent_name.is_none() {
        return Err(errors::missing_field("agent_name"));
    }

    let mut store = h.lock_store();

    let created = if let Some(sender_session_id) = payload.sender_session_id {
        factory_delegation::queue::queue_from_session(
            &mut store,
            payload.task_id,
            sender_session_id,
            scope_id,
            payload.target_session_id,
            payload.target_workspace_path.as_deref(),
            &payload.prompt,
        )
    } else {
        factory_delegation::queue::queue_from_human(
            &mut store,
            payload.task_id,
            scope_id,
            payload.target_session_id,
            payload.target_workspace_path.as_deref(),
            &payload.prompt,
        )
    };
    let task_id = created.map_err(errors::delegation_error)?;

    let (agent_name, max_sessions) = match payload.agent_name.as_deref() {
        Some(name) => {
            let config = crate::handler::config::load(h.instance_root())?;
            let max_sessions =
                crate::handler::config::find_agent(&config, scope_id, name)?.max_sessions;
            (name.to_string(), max_sessions)
        }
        // `target_session_id` case: `assign()`'s requested-session branch
        // reads `tasks.target_session_id` directly and never consults
        // either argument below.
        None => (String::new(), 1),
    };

    let assignment = factory_task::assign::assign(&mut store, task_id, &agent_name, max_sessions)
        .map_err(errors::assign_error)?;

    let (assignment_json, delivery_json, status) = match assignment {
        factory_task::assign::Assignment::Assigned(session_id) => {
            let (pane, _) = crate::handler::pane::read(&store, session_id)?;
            let pane = pane.map(factory_adapter::PaneId);
            let mut writer = crate::handler::deliver::AdapterPromptWriter::new(
                h.adapter(),
                task_id,
                pane.clone(),
            );
            let deliver_result = factory_task::deliver::deliver(&mut store, task_id, &mut writer);

            match deliver_result {
                Ok(()) => {
                    factory_task::deliver::mark_running(&mut store, task_id)
                        .map_err(errors::deliver_error)?;

                    // ADR 0021 decision 6: the baseline is the cost sample
                    // taken when the task is delivered, so a cumulative
                    // source can still answer "what did this run cost" after
                    // a restart. Taken and written *after* `mark_running`
                    // has already committed, in its own transaction, so a
                    // failure here can never undo a delivery that already
                    // happened -- cost is an annotation, never a
                    // precondition. See this task's own report for the
                    // mutation that proves it.
                    let baseline = sample_cost_best_effort(h.adapter(), pane.as_ref());
                    let baseline_json = baseline.as_ref().map(ToString::to_string);
                    let _ = factory_task::deliver::record_cost_baseline(
                        &mut store,
                        task_id,
                        baseline_json.as_deref(),
                    );

                    (
                        json!({ "kind": "assigned", "session_id": session_id.to_string() }),
                        json!({ "sent": true, "error": Value::Null }),
                        "running",
                    )
                }
                Err(e) => (
                    json!({ "kind": "assigned", "session_id": session_id.to_string() }),
                    json!({ "sent": false, "error": e.to_string() }),
                    "queued",
                ),
            }
        }
        factory_task::assign::Assignment::StartSessionAt(path) => (
            json!({ "kind": "start_session_at", "path": path.display().to_string() }),
            Value::Null,
            "queued",
        ),
        factory_task::assign::Assignment::Deferred(reason) => (
            json!({ "kind": "deferred", "reason": defer_reason_json(&reason) }),
            Value::Null,
            "queued",
        ),
    };

    h.success(
        json!({
            "task_id": task_id.to_string(),
            "status": status,
            "assignment": assignment_json,
            "delivery": delivery_json,
        }),
        true,
    )
}

fn defer_reason_json(r: &factory_task::assign::DeferReason) -> Value {
    use factory_task::assign::DeferReason as D;
    match r {
        D::NoIdleSession { agent_name } => {
            json!({ "kind": "no_idle_session", "agent_name": agent_name })
        }
        D::RequestedSessionNotFound { session_id } => {
            json!({ "kind": "requested_session_not_found", "session_id": session_id.to_string() })
        }
        D::SessionNotIdle { session_id, state } => {
            json!({ "kind": "session_not_idle", "session_id": session_id.to_string(), "state": state })
        }
        D::WorkspaceLeasedByAnotherScope {
            session_id,
            scope_id,
        } => json!({
            "kind": "workspace_leased_by_another_scope",
            "session_id": session_id.to_string(),
            "scope_id": scope_id.to_string(),
        }),
        D::WorkspaceNotFound { path } => {
            json!({ "kind": "workspace_not_found", "path": path.display().to_string() })
        }
        D::AgentAtCapacity {
            agent_name,
            max_sessions,
        } => json!({
            "kind": "agent_at_capacity",
            "agent_name": agent_name,
            "max_sessions": max_sessions,
        }),
    }
}

pub(crate) fn cancel(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {
        #[serde(with = "crate::serde_uuid::required")]
        task_id: uuid::Uuid,
    }
    let payload: Payload = errors::parse_payload(&payload)?;

    let mut store = h.lock_store();
    let status =
        factory_task::create::cancel(&mut store, payload.task_id).map_err(errors::task_error)?;

    if status == factory_task::TaskStatus::Cancelled {
        on_terminal(h, &mut store, payload.task_id)?;
    }

    h.success(
        json!({ "task_id": payload.task_id.to_string(), "status": status.to_string() }),
        true,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultPayload {
    #[serde(with = "crate::serde_uuid::required")]
    task_id: uuid::Uuid,
    #[serde(default)]
    result_summary: Option<String>,
    #[serde(default)]
    result_artifact_paths: Option<Vec<String>>,
}

pub(crate) fn done(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: ResultPayload = errors::parse_payload(&payload)?;
    let mut store = h.lock_store();
    factory_task::complete::done(
        &mut store,
        payload.task_id,
        payload.result_summary.as_deref(),
        payload.result_artifact_paths.as_deref(),
    )
    .map_err(errors::complete_error)?;
    on_terminal(h, &mut store, payload.task_id)?;
    h.success(
        json!({ "task_id": payload.task_id.to_string(), "status": "done" }),
        true,
    )
}

pub(crate) fn fail(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: ResultPayload = errors::parse_payload(&payload)?;
    let mut store = h.lock_store();
    factory_task::complete::fail(
        &mut store,
        payload.task_id,
        payload.result_summary.as_deref(),
        payload.result_artifact_paths.as_deref(),
    )
    .map_err(errors::complete_error)?;
    on_terminal(h, &mut store, payload.task_id)?;
    h.success(
        json!({ "task_id": payload.task_id.to_string(), "status": "failed" }),
        true,
    )
}

pub(crate) fn block(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {
        #[serde(with = "crate::serde_uuid::required")]
        task_id: uuid::Uuid,
        reason: String,
    }
    let payload: Payload = errors::parse_payload(&payload)?;
    let reason = parse_blocked_reason(&payload.reason)?;

    let mut store = h.lock_store();
    // `task.block` never reaches a terminal state (design §2.4: `blocked` is
    // not terminal), so — unlike `done`/`fail`/`cancel` — this never calls
    // `on_terminal`: no lifetime teardown, no delegation-completion notice.
    factory_task::complete::blocked(&mut store, payload.task_id, reason)
        .map_err(errors::complete_error)?;

    h.success(
        json!({
            "task_id": payload.task_id.to_string(),
            "status": "blocked",
            "blocked_reason": reason.to_string(),
        }),
        true,
    )
}

fn parse_blocked_reason(s: &str) -> Result<factory_task::BlockedReason, ErrorBody> {
    use factory_task::BlockedReason as B;
    match s {
        "clarification" => Ok(B::Clarification),
        "permission" => Ok(B::Permission),
        "interrupted" => Ok(B::Interrupted),
        "external" => Ok(B::External),
        other => Err(errors::err(
            "validation.invalid_field",
            format!(
                "unknown blocked reason `{other}`; expected clarification, permission, \
                 interrupted, or external"
            ),
        )),
    }
}

pub(crate) fn resume(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {
        #[serde(with = "crate::serde_uuid::required")]
        task_id: uuid::Uuid,
    }
    let payload: Payload = errors::parse_payload(&payload)?;

    let mut store = h.lock_store();
    factory_task::deliver::authorise_resume(&mut store, payload.task_id)
        .map_err(errors::task_error)?;

    h.success(
        json!({ "task_id": payload.task_id.to_string(), "status": "queued" }),
        true,
    )
}

pub(crate) fn list(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {}
    let _: Payload = errors::parse_payload(&payload)?;

    let store = h.lock_store();
    let tasks = factory_task::create::list(&store).map_err(errors::task_error)?;
    h.success(
        json!({ "tasks": tasks.iter().map(task_json).collect::<Vec<_>>() }),
        false,
    )
}

pub(crate) fn show(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {
        #[serde(with = "crate::serde_uuid::required")]
        task_id: uuid::Uuid,
    }
    let payload: Payload = errors::parse_payload(&payload)?;

    let store = h.lock_store();
    let task = factory_task::create::show(&store, payload.task_id).map_err(errors::task_error)?;
    let chain = factory_task::create::delegation_chain_of(&store, payload.task_id)
        .map_err(errors::task_error)?;

    let mut result = task_json(&task);
    result["delegation_chain"] = json!(chain.iter().map(|id| id.to_string()).collect::<Vec<_>>());
    h.success(result, false)
}

/// `task.wait`'s default and maximum timeout. Bounded, not indefinite — see
/// `lib.rs`'s own module docs on why an unbounded wait would hold a thread
/// and a socket, and never mutates on expiry (waiting is a read).
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;
const MAX_WAIT_TIMEOUT_MS: u64 = 300_000;
const WAIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitPayload {
    #[serde(with = "crate::serde_uuid::required")]
    task_id: uuid::Uuid,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Poll `task_id`'s status until it is terminal or `blocked`, or until the
/// deadline passes — never holding `FactoryHandler`'s store mutex while it
/// sleeps (see `handler`'s module docs: an unbounded hold here would block
/// the `task.done` push that would end the wait). A timeout returns the
/// task's current status and `timed_out: true`; it calls no domain mutation
/// at all, so the task is left exactly as it was found — still queued, still
/// running, still delegated, per crate docs decision 9's own requirement.
pub(crate) fn wait(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: WaitPayload = errors::parse_payload(&payload)?;
    let timeout_ms = payload
        .timeout_ms
        .unwrap_or(DEFAULT_WAIT_TIMEOUT_MS)
        .min(MAX_WAIT_TIMEOUT_MS);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);

    loop {
        let task = {
            let store = h.lock_store();
            factory_task::create::show(&store, payload.task_id).map_err(errors::task_error)?
        };

        if task.status.is_terminal() || task.status == factory_task::TaskStatus::Blocked {
            return h.success(
                json!({
                    "task_id": payload.task_id.to_string(),
                    "status": task.status.to_string(),
                    "timed_out": false,
                }),
                false,
            );
        }

        if std::time::Instant::now() >= deadline {
            return h.success(
                json!({
                    "task_id": payload.task_id.to_string(),
                    "status": task.status.to_string(),
                    "timed_out": true,
                }),
                false,
            );
        }

        std::thread::sleep(WAIT_POLL_INTERVAL);
    }
}

fn task_json(t: &factory_task::create::Task) -> Value {
    json!({
        "id": t.id.to_string(),
        "sender_scope_id": t.sender_scope_id.map(|id| id.to_string()),
        "target_scope_id": t.target_scope_id.to_string(),
        "target_session_id": t.target_session_id.map(|id| id.to_string()),
        "target_workspace_path": t.target_workspace_path,
        "assigned_session_id": t.assigned_session_id.map(|id| id.to_string()),
        "prompt": t.prompt,
        "status": t.status.to_string(),
        "blocked_reason": t.blocked_reason.map(|r| r.to_string()),
        "cancel_requested_at": t.cancel_requested_at,
        "result_summary": t.result_summary,
        "result_artifact_paths": t.result_artifact_paths,
        "created_at": t.created_at,
        "updated_at": t.updated_at,
        // Station 11's closeout list already names this gap: `template_id`
        // and `template_version` existed on the row (migration 6) but were
        // never carried into this response. Added alongside the cost
        // figures below rather than left for a separate pass.
        "template_id": t.template_id.map(|id| id.to_string()),
        "template_version": t.template_version,
        // Design §12.6 / ADR 0021 decisions 6-7. `cost_baseline` is
        // deliberately not here: it is the raw adapter-shaped counters this
        // run started from, not a figure about the run itself -- see
        // `Task::cost_baseline`'s own doc comment.
        "cost_model": t.cost_model,
        "cost_input_tokens": t.cost_input_tokens,
        "cost_output_tokens": t.cost_output_tokens,
        "cost_duration_ms": t.cost_duration_ms,
        "context_utilization_percent": t.context_utilization_percent,
    })
}

/// A best-effort cost sample: `None` when there is no pane to ask, when the
/// adapter itself errors, or when the adapter simply has nothing to report.
/// ADR 0021 decision 6's own point: `None` is the ordinary answer for two of
/// four live Pi sessions and every `opencode` session, not the edge, and "a
/// failure to read cost must never fail" the task action this accompanies —
/// this is the one place that rule is enforced for both call sites below.
fn sample_cost_best_effort(
    adapter: &(dyn factory_adapter::Adapter + Send + Sync),
    pane: Option<&factory_adapter::PaneId>,
) -> Option<factory_adapter::CostSample> {
    let pane = pane?;
    adapter.cost_sample(pane).ok().flatten()
}

/// ADR 0021 decision 6's terminal half: a second cost sample, subtracted
/// from the baseline `task.send` recorded at delivery
/// (`Task::cost_baseline`), and written to the run's own cost columns.
///
/// Both `PiAdapter` and `ClaudeAdapter` hand back a *cumulative-to-date*
/// sample (verified in `factory_adapter::sum_pi_transcript_usage`'s own body
/// and doc comment: it sums the whole Pi transcript to date, not a
/// caller-supplied window, so the "sample now, sample later, subtract" rule
/// applies identically to both sources) — so there is exactly one arithmetic
/// here, `factory_adapter::CostSample::since`, and no `match` on
/// `CostSource` is needed at this call site.
///
/// Every step is best-effort, mirroring `teardown_session_if_terminal`'s own
/// stance just below it in `on_terminal`: a task's completion has already
/// committed by the time this runs, and a cost figure is an annotation on
/// it, never a precondition.
fn record_cost_at_terminal(
    h: &FactoryHandler,
    store: &mut factory_store::Store,
    task: &factory_task::create::Task,
) {
    let Some(session_id) = task.assigned_session_id else {
        return;
    };
    let Ok((pane, _)) = crate::handler::pane::read(store, session_id) else {
        return;
    };
    let Some(pane) = pane.map(factory_adapter::PaneId) else {
        return;
    };
    let Some(terminal) = sample_cost_best_effort(h.adapter(), Some(&pane)) else {
        return;
    };

    // No baseline at all -- no pane at delivery, an adapter that reported
    // nothing then -- means there is no run figure to compute, full stop.
    // Writing `terminal`'s own model here regardless would read as "this
    // run's cost was measured" to anything that checks for a cost column's
    // mere presence rather than the token counts specifically: the same
    // right-type-wrong-column defect ADR 0021 decision 7 already names,
    // worn by a different field.
    let Some(baseline) = task
        .cost_baseline
        .as_deref()
        .and_then(|raw| raw.parse::<factory_adapter::CostSample>().ok())
    else {
        return;
    };

    let (model, input_tokens, output_tokens, duration_ms, context_utilization_percent) =
        match terminal.since(&baseline) {
            Ok(Some(run_cost)) => (
                run_cost.model,
                Some(run_cost.input_tokens as i64),
                Some(run_cost.output_tokens as i64),
                run_cost.duration_ms.map(|ms| ms as i64),
                run_cost.context_utilization_percent,
            ),
            // The token counters could not be subtracted -- a compaction or
            // transcript rotation between the two samples
            // (`CostSample::since`'s own doc comment). `model` and
            // `context_utilization_percent` are never diffed in the first
            // place, so that doc comment is explicit that the caller must
            // still read them from `terminal` directly rather than lose
            // them alongside a token delta that could not be computed.
            Ok(None) => (
                terminal.model.clone(),
                None,
                None,
                None,
                terminal.context_utilization_percent,
            ),
            // A caller bug -- `baseline` and `terminal` came from different
            // adapters (`CostSampleError::MismatchedSource`). Cost is an
            // annotation, never a precondition for completion, so this
            // degrades to "nothing to write" like every other cost failure
            // here rather than propagating an error nobody can act on.
            Err(_) => return,
        };

    let _ = factory_task::complete::record_cost_result(
        store,
        task.id,
        model.as_deref(),
        input_tokens,
        output_tokens,
        duration_ms,
        context_utilization_percent,
    );
}

/// After a task's own status write commits (`task.done`, `task.fail`, or
/// `task.cancel`'s immediate path — never `task.block`, which is not
/// terminal), thread through design §2.2's lifetime rule and, when the task
/// was delegated, crate docs decision 6's completion notice.
fn on_terminal(
    h: &FactoryHandler,
    store: &mut factory_store::Store,
    task_id: uuid::Uuid,
) -> Result<(), ErrorBody> {
    let task = factory_task::create::show(store, task_id).map_err(errors::task_error)?;

    // ADR 0021 decision 6's terminal cost sample, taken while the task's
    // session -- if any -- is still `running`. This must run *before*
    // `teardown_session_if_terminal` just below: measured directly (see the
    // ordering test's own mutation), that call commits
    // `sessions.state = 'stopped'` for a temporary agent's session, and
    // sampling after it would read a session the run no longer owns. (It
    // does not, today, also close the real Herdr pane --
    // `factory_session::on_task_terminal` has no `factory-adapter`
    // dependency and never calls `Adapter::stop` -- but `Adapter::cost_sample`
    // takes a pane, and this ordering is what keeps the answer correct if
    // that ever changes, not only correct against today's session-state
    // check.) `cost_sample_runs_before_session_teardown` in `tests/cost.rs`
    // pins the ordering so a later edit cannot move it back down unnoticed.
    record_cost_at_terminal(h, store, &task);

    teardown_session_if_terminal(store, h.instance_root(), &task);

    // `sender_scope_id.is_some()` — not "chain length > 0" — is what
    // distinguishes a delegated task from a merely human-queued one: a
    // human-queued task's own chain is `[target]` (non-empty) but was sent
    // by nobody a notice could reach. See `lib.rs`'s payload table.
    if task.sender_scope_id.is_some() {
        crate::notify::notify_delegator(h, store, &task);
    }

    Ok(())
}

/// Design §2.2: a `temporary` agent's session is torn down once its task
/// reaches a terminal state; a `permanent` one is left alone
/// (`factory_session::on_task_terminal` itself short-circuits for it). Every
/// step here is best-effort: a session already gone, or a configuration that
/// no longer names this agent, leaves the task's own completion — which
/// already committed — untouched either way.
fn teardown_session_if_terminal(
    store: &mut factory_store::Store,
    instance_root: &std::path::Path,
    task: &factory_task::create::Task,
) {
    let Some(session_id) = task.assigned_session_id else {
        return;
    };
    let Ok(session) = factory_session::show(store, session_id) else {
        return;
    };
    let Ok(config) = crate::handler::config::load(instance_root) else {
        return;
    };
    let Ok(agent) =
        crate::handler::config::find_agent(&config, session.scope_id, &session.agent_name)
    else {
        return;
    };
    let _ = factory_session::on_task_terminal(store, session_id, task.id, agent.lifetime);
}
