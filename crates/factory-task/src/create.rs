//! Task creation, read-only inspection, and cooperative cancellation
//! (design §2.4, §5; backlog §7).
//!
//! Nothing here talks to a terminal, a harness, or an adapter. Assignment
//! (choosing a session) and delivery (writing to a PTY) are the other two
//! modules this slice defines — `assign` and `deliver` — which stay
//! placeholders in this task; this module only ever writes `queued`, reads
//! rows back, or moves a non-terminal task straight to `cancelled` /
//! records a cancellation request. There is no PTY automation anywhere in
//! this file.

use rusqlite::OptionalExtension;

use crate::events::EventType;
use crate::{BlockedReason, TaskError, TaskStatus, is_valid_transition, valid_targets};

/// One `tasks` row, as read back by [`list`] and [`show`].
///
/// A plain data record, not a handle: nothing here talks back to the
/// database. Every `TEXT`-typed identifier column is parsed into its typed
/// form ([`uuid::Uuid`] for ids, [`TaskStatus`] / [`BlockedReason`] for the
/// vocabulary columns) since this crate itself wrote them and a value that
/// does not parse is a broken invariant, not an input to handle — the same
/// stance [`TaskStatus::from_db_str`] takes.
///
/// Not `Eq`: `context_utilization_percent` is `f64`, which has none. Nothing
/// in this crate ever needed `Task: Eq` (it is not used as a map key or in a
/// set), so the derive is simply narrowed rather than worked around.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: uuid::Uuid,
    /// `None` means the sender was a human, not another scope.
    pub sender_scope_id: Option<uuid::Uuid>,
    pub target_scope_id: uuid::Uuid,
    /// The session the sender *requested*, if any. See [`Task::assigned_session_id`]
    /// for the column this is deliberately distinct from.
    pub target_session_id: Option<uuid::Uuid>,
    pub target_workspace_path: Option<String>,
    /// The session Factory *chose* — set once the task is assigned, distinct
    /// from [`Task::target_session_id`] (design §2.4 / `factory_store::schema`
    /// migration 3's doc comment: "the session Factory *chose*" vs. "what the
    /// sender *requested*"). Non-NULL whenever `status` is
    /// [`TaskStatus::Running`] (enforced by the database, not this struct).
    pub assigned_session_id: Option<uuid::Uuid>,
    pub prompt: String,
    pub status: TaskStatus,
    pub blocked_reason: Option<BlockedReason>,
    /// Set once an operator asks a *running* task to stop; the task itself
    /// stays `running` until the agent reports a terminal status. See
    /// [`cancel`].
    pub cancel_requested_at: Option<String>,
    pub result_summary: Option<String>,
    /// A JSON array of artifact paths, stored verbatim (this crate does not
    /// parse it — see `factory_store::schema`'s comment on the column).
    pub result_artifact_paths: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// The template this run executed, if any — `None` for a task queued
    /// directly (design §11's "extension of the existing `Task` primitive").
    /// See [`create_from_template`] for how this is set and
    /// [`Task::template_version`] for the field it is always paired with.
    pub template_id: Option<uuid::Uuid>,
    /// The template's `version` at the moment this run was created, frozen
    /// from then on — ADR 0021 decision 2. `Some` exactly when
    /// [`Task::template_id`] is `Some`; both are set together by
    /// [`create_from_template`] and never touched again by anything in this
    /// crate.
    pub template_version: Option<i64>,
    /// The harness's own model identifier for this run's terminal cost
    /// sample — ADR 0021 decision 6. `None` whenever the adapter reported
    /// nothing usable, which is the ordinary case for two of four live Pi
    /// sessions and every `opencode` session (that decision's own
    /// measurement), not the edge.
    pub cost_model: Option<String>,
    /// `later.input_tokens - baseline.input_tokens`, written by
    /// `factory-daemon`'s `ops::task::on_terminal` through
    /// [`crate::complete::record_cost_result`]. `None` exactly when no run
    /// figure could be computed at all — see that function's own doc
    /// comment for the cases that leaves this `NULL`.
    pub cost_input_tokens: Option<i64>,
    /// See [`Task::cost_input_tokens`]; the output-token twin.
    pub cost_output_tokens: Option<i64>,
    /// Milliseconds. `None` whenever either sample carried no duration —
    /// Pi's transcript never does (`factory_adapter`'s module docs).
    pub cost_duration_ms: Option<i64>,
    /// The adapter-shaped sample taken **at delivery**
    /// (`factory_adapter::CostSample`'s own `Display`, i.e. JSON), through
    /// [`crate::deliver::record_cost_baseline`]. ADR 0021 decision 6: a
    /// cumulative source can only answer "what did this run cost" as a
    /// difference between two samples, and a difference held only in memory
    /// does not survive the daemon restart this project drills for — this
    /// column is what does. Deliberately not exposed by
    /// `factory-daemon`'s `task_json`: it is the raw counters this run
    /// started from, not a figure about the run itself.
    pub cost_baseline: Option<String>,
    /// How full the context window was when the terminal sample was taken,
    /// 0–100. Never diffed (ADR 0021 decision 7): it is context *pressure*
    /// at the end of the run, not a token count, and it is written from the
    /// terminal sample directly rather than through a subtraction.
    pub context_utilization_percent: Option<f64>,
    /// §12.2's hook: the run this one reworks, if any — set once, at
    /// creation, by [`create_rework`], and never written again by anything
    /// in this crate. `None` for every run created through [`create`],
    /// [`create_from_template`], or [`create_from_schedule`].
    pub reworks_task_id: Option<uuid::Uuid>,
    /// The finding that caused the rework. `Some` exactly when
    /// [`Task::reworks_task_id`] is `Some` — [`create_rework`] refuses an
    /// empty finding, the same stance `decisions::record` takes toward
    /// `task_decisions.rationale`.
    pub rework_finding: Option<String>,
    /// `manual` or `cron` — where this run came from. Read back so a caller
    /// above SQL can tell the two apart at all.
    ///
    /// The station-11 live drill is why this is here. Every run it created
    /// carried `triggered_by = 'cron'` with its schedule and local minute on
    /// the row, and `factory task list` showed none of the three, so a cron
    /// run and a hand-sent one were identical in every output an operator
    /// has. The columns existed; nothing read them.
    pub triggered_by: String,
    /// The schedule that fired this run. `Some` exactly when
    /// [`Task::triggered_by`] is `cron` — migration 6's own CHECK binds the
    /// three together, so this cannot drift from that field.
    pub schedule_id: Option<uuid::Uuid>,
    /// The local minute this run was fired for, `YYYY-MM-DDTHH:MM` in the
    /// schedule's own timezone. `Some` under the same condition as
    /// [`Task::schedule_id`].
    pub fired_for_minute: Option<String>,
}

const TASK_COLUMNS: &str = "id, sender_scope_id, target_scope_id, target_session_id, \
     target_workspace_path, assigned_session_id, prompt, status, blocked_reason, \
     cancel_requested_at, result_summary, result_artifact_paths, created_at, updated_at, \
     template_id, template_version, cost_model, cost_input_tokens, cost_output_tokens, \
     cost_duration_ms, cost_baseline, context_utilization_percent, reworks_task_id, \
     rework_finding, triggered_by, schedule_id, fired_for_minute";

/// `tasks.id`, `.sender_scope_id`, `.target_scope_id`, `.target_session_id`,
/// and `.assigned_session_id` are all UUIDs this crate — or `factory-session`
/// / a future registry — wrote; a value that fails to parse is a broken
/// invariant in a row this code just selected, so this panics rather than
/// threading a parse error through every caller of [`list`] and [`show`].
fn parse_uuid(column: &str, value: &str) -> uuid::Uuid {
    uuid::Uuid::parse_str(value)
        .unwrap_or_else(|e| panic!("tasks.{column} is a UUID; read {value:?}: {e}"))
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let id: String = row.get(0)?;
    let sender_scope_id: Option<String> = row.get(1)?;
    let target_scope_id: String = row.get(2)?;
    let target_session_id: Option<String> = row.get(3)?;
    let target_workspace_path: Option<String> = row.get(4)?;
    let assigned_session_id: Option<String> = row.get(5)?;
    let prompt: String = row.get(6)?;
    let status: String = row.get(7)?;
    let blocked_reason: Option<String> = row.get(8)?;
    let cancel_requested_at: Option<String> = row.get(9)?;
    let result_summary: Option<String> = row.get(10)?;
    let result_artifact_paths: Option<String> = row.get(11)?;
    let created_at: String = row.get(12)?;
    let updated_at: String = row.get(13)?;
    let template_id: Option<String> = row.get(14)?;
    let template_version: Option<i64> = row.get(15)?;
    let cost_model: Option<String> = row.get(16)?;
    let cost_input_tokens: Option<i64> = row.get(17)?;
    let cost_output_tokens: Option<i64> = row.get(18)?;
    let cost_duration_ms: Option<i64> = row.get(19)?;
    let cost_baseline: Option<String> = row.get(20)?;
    let context_utilization_percent: Option<f64> = row.get(21)?;
    let reworks_task_id: Option<String> = row.get(22)?;
    let rework_finding: Option<String> = row.get(23)?;
    let triggered_by: String = row.get(24)?;
    let schedule_id: Option<String> = row.get(25)?;
    let fired_for_minute: Option<String> = row.get(26)?;

    Ok(Task {
        id: parse_uuid("id", &id),
        sender_scope_id: sender_scope_id
            .as_deref()
            .map(|s| parse_uuid("sender_scope_id", s)),
        target_scope_id: parse_uuid("target_scope_id", &target_scope_id),
        target_session_id: target_session_id
            .as_deref()
            .map(|s| parse_uuid("target_session_id", s)),
        target_workspace_path,
        assigned_session_id: assigned_session_id
            .as_deref()
            .map(|s| parse_uuid("assigned_session_id", s)),
        prompt,
        status: TaskStatus::from_db_str(&status),
        blocked_reason: blocked_reason.as_deref().map(BlockedReason::from_db_str),
        cancel_requested_at,
        result_summary,
        result_artifact_paths,
        created_at,
        updated_at,
        template_id: template_id.as_deref().map(|s| parse_uuid("template_id", s)),
        template_version,
        cost_model,
        cost_input_tokens,
        cost_output_tokens,
        cost_duration_ms,
        cost_baseline,
        context_utilization_percent,
        reworks_task_id: reworks_task_id
            .as_deref()
            .map(|s| parse_uuid("reworks_task_id", s)),
        rework_finding,
        triggered_by,
        schedule_id: schedule_id.as_deref().map(|s| parse_uuid("schedule_id", s)),
        fired_for_minute,
    })
}

/// Write the `queued` row and its delegation chain against an
/// already-open transaction. The one INSERT this crate ever issues against
/// `tasks` to create a run — [`create`] and [`create_from_template`] are
/// both a `Store::transaction` plus whatever each needs to decide
/// `template_id` / `template_version`, then this.
///
/// `delegation_chain` is written in the *same* transaction as the task row.
/// A task whose chain were committed separately could be read back, after a
/// crash between the two commits, as a task that had travelled through no
/// scope at all — and backlog §8 asks for exactly the opposite: "the chain is
/// recorded durably with the task, so a delegation loop is reconstructable
/// after a restart rather than only detectable while running." This function
/// stores the chain it is given and judges none of it; deciding *what* the
/// chain is, and whether the target may be appended to it at all, belongs to
/// `factory_delegation`, which calls in through [`create`].
///
/// Takes `&rusqlite::Transaction` rather than `&mut Store`: [`create_from_template`]
/// needs to read `task_templates.version` and write the row it freezes that
/// value into inside one transaction (ADR 0021 decision 2 — see that
/// function's doc comment), so the transaction has to be open before this is
/// called, not inside it.
///
/// Writes the run's `created` event (`crate`'s station-11 decision 3) in the
/// same transaction as the INSERT above — the one home for "a task was
/// created" is here, since this is the one INSERT this crate ever issues for
/// a run, and [`create`], [`create_from_template`], [`create_from_schedule`],
/// and [`create_rework`] all reach it through this function. No payload: the
/// row this function just inserted already carries every fact about the
/// creation, and `prompt` is exactly the kind of content decision 6 forbids
/// from a payload, so nothing beyond the event type itself is recorded here.
///
/// `reworks_task_id` / `rework_finding` (§12.2's hook) are the one exception
/// to "no payload beyond the event type": when [`create_rework`] passes
/// `Some`, this function writes a **second** event, `Rework`, against the
/// *new* row this call just inserted — never against the run it reworks. See
/// [`create_rework`]'s own doc comment for the validation that runs before
/// this function is ever called, and for why the referenced run's own row is
/// never written to.
#[allow(clippy::too_many_arguments)]
fn insert_task(
    tx: &rusqlite::Transaction<'_>,
    id: uuid::Uuid,
    sender_scope_id: Option<uuid::Uuid>,
    target_scope_id: uuid::Uuid,
    target_session_id: Option<uuid::Uuid>,
    target_workspace_path: Option<&str>,
    prompt: &str,
    delegation_chain: &[uuid::Uuid],
    template_id: Option<uuid::Uuid>,
    template_version: Option<i64>,
    cron_origin: Option<CronOrigin<'_>>,
    rework: Option<(uuid::Uuid, &str)>,
) -> Result<(), TaskError> {
    // `triggered_by` is derived from `cron_origin` rather than passed
    // separately, because migration 6's CHECK ties the three together: a
    // `cron` row must carry both a schedule and a minute, and a `manual` row
    // must carry neither. One argument that can only produce a legal
    // combination is better than three that can produce an illegal one and
    // learn about it from the database.
    let (triggered_by, schedule_id, fired_for_minute) = match cron_origin {
        Some(origin) => (
            "cron",
            Some(origin.schedule_id.to_string()),
            Some(origin.fired_for_minute),
        ),
        None => ("manual", None, None),
    };
    let (reworks_task_id, rework_finding) = match rework {
        Some((reworks_task_id, finding)) => (Some(reworks_task_id.to_string()), Some(finding)),
        None => (None, None),
    };
    tx.execute(
        "INSERT INTO tasks \
         (id, sender_scope_id, target_scope_id, target_session_id, target_workspace_path, \
          prompt, status, template_id, template_version, \
          triggered_by, schedule_id, fired_for_minute, reworks_task_id, rework_finding) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        (
            id.to_string(),
            sender_scope_id.map(|s| s.to_string()),
            target_scope_id.to_string(),
            target_session_id.map(|s| s.to_string()),
            target_workspace_path,
            prompt,
            template_id.map(|t| t.to_string()),
            template_version,
            triggered_by,
            schedule_id,
            fired_for_minute,
            reworks_task_id,
            rework_finding,
        ),
    )
    .map_err(factory_store::StoreError::from)?;
    for (position, scope_id) in delegation_chain.iter().enumerate() {
        tx.execute(
            "INSERT INTO task_delegation_chain (task_id, position, scope_id) \
             VALUES (?1, ?2, ?3)",
            (id.to_string(), position as i64, scope_id.to_string()),
        )
        .map_err(factory_store::StoreError::from)?;
    }
    crate::events::append(tx, id, EventType::Created, None, None)?;
    if let Some((reworks_task_id, finding)) = rework {
        // Against `id` — the row just inserted above — never against
        // `reworks_task_id`. This is the whole of the writing path design
        // §11 / §12.2 ask for: "a run can reference the run it reworks...
        // without modifying the referenced run." Nothing here executes an
        // UPDATE against `reworks_task_id`'s own row, and nothing above does
        // either.
        let payload = serde_json::json!({
            "reworks_task_id": reworks_task_id.to_string(),
            "finding": finding,
        })
        .to_string();
        crate::events::append(tx, id, EventType::Rework, None, Some(&payload))?;
    }
    Ok(())
}

/// Commit `queued` in its own transaction and return the id it was written
/// under.
///
/// Design §5 step 1: the row is durable *before* anything is entered into a
/// terminal. This function is the entirety of that step — it does not choose
/// a session and does not touch a harness or an adapter. `template_id` and
/// `template_version` are left `NULL`: this is the plain, no-template path,
/// unchanged from before this crate carried templates at all — see
/// [`create_from_template`] for the other one.
///
/// `id` is supplied by the caller rather than generated here, mirroring
/// `factory_session::begin_start`'s own `id: uuid::Uuid` parameter: the
/// `uuid` crate is pinned workspace-wide without the `v4` feature (and the
/// workspace manifest is centrally owned, so this crate cannot add it), so
/// there is no `Uuid::new_v4()` available to call inside this crate even if
/// it were this function's job to mint the id. The caller — the same seam
/// that already decides session ids — decides task ids too. Returning `id`
/// back is a convenience for a caller that wants to log or chain on it
/// immediately, not evidence that this function chose it.
#[allow(clippy::too_many_arguments)]
pub fn create(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    sender_scope_id: Option<uuid::Uuid>,
    target_scope_id: uuid::Uuid,
    target_session_id: Option<uuid::Uuid>,
    target_workspace_path: Option<&str>,
    prompt: &str,
    delegation_chain: &[uuid::Uuid],
) -> Result<uuid::Uuid, TaskError> {
    let tx = store.transaction()?;
    insert_task(
        &tx,
        id,
        sender_scope_id,
        target_scope_id,
        target_session_id,
        target_workspace_path,
        prompt,
        delegation_chain,
        None,
        None,
        None,
        None,
    )?;
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(id)
}

/// The template-backed twin of [`create`]: same row, same delegation-chain
/// handling, plus `template_id` and `template_version` — crate docs, station
/// 11 decision 1: "[a]nything that reads like 'create a task run' is
/// [`create`] with `template_id` and `template_version` filled in." This is
/// that filling-in, as a sibling entry point rather than a change to
/// [`create`]'s own signature.
///
/// # Why a sibling function and not an extended `create`
///
/// `create` takes eight positional arguments, and both of its callers today
/// —`factory_delegation::queue::queue_from_human` and `::queue_from_session`,
/// each calling `factory_task::create::create` positionally — are outside
/// this crate. Appending parameters to `create` would require editing both
/// call sites, and neither is this slice's file to touch. A sibling function
/// over the same insert ([`insert_task`]) keeps `create`'s signature and
/// every existing caller exactly as they are, while sharing the one INSERT
/// this crate issues for a run — not a second, parallel path that could
/// drift from the first.
///
/// # The freezing rule (ADR 0021 decision 2)
///
/// Backlog §11: "every run records the template version it executed, and
/// that record does not change when the template is later revised." This
/// function reads `task_templates.version` and writes it into
/// `tasks.template_version` inside **one** transaction — `Store::transaction`
/// opens with `BEGIN IMMEDIATE`, which takes the write lock before the read,
/// so no concurrent `template::revise` can land between the read and the
/// write this function makes. Once committed, nothing else in this crate
/// ever writes `tasks.template_version` again; [`template::revise`] only
/// ever touches `task_templates.version`, a different column of a different
/// table.
///
/// If `template_id` names no real template, the read below returns no row,
/// `template_version` is `None`, and the INSERT is rejected by
/// `tasks.template_id REFERENCES task_templates (id)` — the same backstop
/// `a_failed_chain_insert_leaves_no_task_row_at_all` already proves for a
/// chain entry naming an unregistered scope. This function does not
/// duplicate that check in Rust; the schema already refuses it, and ADR 0021
/// itself prefers a database constraint over "trusting application code to
/// check first" wherever one is available (see the ADR's decision 3).
#[allow(clippy::too_many_arguments)]
pub fn create_from_template(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    sender_scope_id: Option<uuid::Uuid>,
    target_scope_id: uuid::Uuid,
    target_session_id: Option<uuid::Uuid>,
    target_workspace_path: Option<&str>,
    prompt: &str,
    delegation_chain: &[uuid::Uuid],
    template_id: uuid::Uuid,
) -> Result<uuid::Uuid, TaskError> {
    let tx = store.transaction()?;

    let template_version: Option<i64> = tx
        .query_row(
            "SELECT version FROM task_templates WHERE id = ?1",
            [template_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;

    insert_task(
        &tx,
        id,
        sender_scope_id,
        target_scope_id,
        target_session_id,
        target_workspace_path,
        prompt,
        delegation_chain,
        Some(template_id),
        template_version,
        None,
        None,
    )?;
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(id)
}

/// The rework-backed twin of [`create`]: design §11 / §12.2's hook — "a run
/// can reference the run it reworks together with the finding that caused
/// it, without modifying the referenced run." A sibling entry point over the
/// same [`insert_task`], the same shape [`create_from_template`] already
/// takes over `create`'s own signature, and for the same reason: this
/// crate's two outside callers (`factory_delegation::queue::queue_from_human`
/// and `::queue_from_session`) call [`create`] positionally, so a new
/// parameter belongs on a sibling, not on `create` itself.
///
/// # The three rules this function decides, and why
///
/// Nothing in `factory_store::schema` binds `reworks_task_id` or
/// `rework_finding` — both are plain nullable columns with no `CHECK`
/// spanning two rows — so every rule below is a Rust guard, not a database
/// one, and each has exactly one home: here, before [`insert_task`] is ever
/// called. A second copy of any of them, in a CLI layer or a second creation
/// path, is the exact defect this task's own brief warns against: a
/// validation call present on one path and absent on a sibling proves
/// nothing was ever protected.
///
/// **The referenced run must already be finished.** §12.2's analogy is a
/// rework loop that "carries the inspection finding back onto the line" —
/// a finding presumes an inspection already happened, and an inspection
/// presumes the run it inspects has stopped moving. [`TaskStatus::is_terminal`]
/// is the one home of "finished" in this crate, so this function calls it
/// rather than re-listing `done`/`failed`/`cancelled` — the same stance
/// every other terminal check in this crate already takes.
///
/// This deliberately excludes `blocked`, even though design §12.2's own
/// prose reads as if it might include it ("`failed` and `blocked` are
/// terminal states addressed to a human"). `TaskStatus::is_terminal`'s own
/// doc comment is explicit that `blocked` is not terminal: a blocked run can
/// still return to `queued` and finish. Allowing a rework against it would
/// let two live attempts exist for the same piece of work at once — the run
/// still working towards `done`, and a second run already reworking it —
/// which is a worse record than refusing the rework until the first run
/// actually stops. This is a case where this function's rule and §12.2's own
/// prose read differently; the coordinator asked to be told, so it is
/// recorded here rather than only in this task's report.
///
/// **A run may not rework itself.** `reworks_task_id TEXT REFERENCES tasks
/// (id)` does not catch `reworks_task_id == id`: SQLite's foreign-key check
/// runs after the statement, by which point the row being inserted already
/// exists, so a self-reference satisfies the constraint. Checked before the
/// transaction even opens, since it needs no database read at all.
///
/// **Chains are allowed, and no cycle check is needed — as long as this
/// stays the only writer of `reworks_task_id`.** A reworks B reworks C is
/// ordinary provenance, not a defect, and nothing here restricts chain
/// length. A genuine cycle (B reworking A after A already reworks B) cannot
/// form under this function alone: `reworks_task_id` is written once, at
/// creation, by this function, and nothing in this crate ever `UPDATE`s it
/// afterwards — so by the time a later run could name an earlier one as the
/// run it reworks, the earlier one's own `reworks_task_id` was already fixed
/// and cannot be rewritten to point forward. **If a later change adds any
/// path that updates `tasks.reworks_task_id` after creation, that change must
/// add cycle detection alongside it** — this invariant is exactly what makes
/// the omission safe today, and only today.
#[allow(clippy::too_many_arguments)]
pub fn create_rework(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    sender_scope_id: Option<uuid::Uuid>,
    target_scope_id: uuid::Uuid,
    target_session_id: Option<uuid::Uuid>,
    target_workspace_path: Option<&str>,
    prompt: &str,
    delegation_chain: &[uuid::Uuid],
    reworks_task_id: uuid::Uuid,
    rework_finding: &str,
) -> Result<uuid::Uuid, TaskError> {
    if reworks_task_id == id {
        return Err(TaskError::SelfRework(id));
    }
    if rework_finding.is_empty() {
        return Err(TaskError::ReworkFindingRequired(id));
    }

    let tx = store.transaction()?;

    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            [reworks_task_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some(status) = status else {
        return Err(TaskError::NotFound(reworks_task_id));
    };
    let status = TaskStatus::from_db_str(&status);
    if !status.is_terminal() {
        return Err(TaskError::ReworkTargetNotTerminal {
            id: reworks_task_id,
            status,
        });
    }

    insert_task(
        &tx,
        id,
        sender_scope_id,
        target_scope_id,
        target_session_id,
        target_workspace_path,
        prompt,
        delegation_chain,
        None,
        None,
        None,
        Some((reworks_task_id, rework_finding)),
    )?;
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(id)
}

/// The ordered delegation chain recorded with `task_id`, position 0 first —
/// design §2.4's "delegation chain (every scope the task has passed
/// through)" and backlog §8's "[e]very task carries the ordered delegation
/// chain of scopes it has passed through." 0-based, with the target itself
/// last, mirroring exactly what [`create`] was given and wrote — this
/// function judges none of it, the same stance [`create`]'s own doc comment
/// takes.
///
/// Read-only: takes `&Store`, going through [`factory_store::Store::connection`]
/// — never [`factory_store::Store::transaction`], per that method's own doc
/// comment on why a read path must not take the write lock and contend with
/// real writers.
///
/// `ORDER BY position` is load-bearing, not decorative, and for a reason
/// stronger than "insertion order might not be preserved": this table also
/// carries a `UNIQUE (task_id, scope_id)` index, and — confirmed with
/// `EXPLAIN QUERY PLAN` against a throwaway database before this function
/// was written — a query that only selects `scope_id` and has no `ORDER BY`
/// is satisfied by SQLite's query planner from *that* index as a covering
/// scan, which returns rows in `scope_id`'s lexicographic order, not
/// `position` order and not insertion order. Dropping this clause is
/// therefore invisible against a chain whose scopes happen to sort the same
/// way they were positioned, which is why
/// [`tests::delegation_chain_of_round_trips_a_three_scope_chain_in_position_order`]
/// in `tests/create.rs` deliberately chooses scope ids whose lexicographic
/// order differs from their position order.
pub fn delegation_chain_of(
    store: &factory_store::Store,
    task_id: uuid::Uuid,
) -> Result<Vec<uuid::Uuid>, TaskError> {
    let mut stmt = store
        .connection()
        .prepare("SELECT scope_id FROM task_delegation_chain WHERE task_id = ?1 ORDER BY position")
        .map_err(factory_store::StoreError::from)?;
    let scope_ids: Vec<String> = stmt
        .query_map([task_id.to_string()], |row| row.get(0))
        .map_err(factory_store::StoreError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(factory_store::StoreError::from)?;
    Ok(scope_ids
        .into_iter()
        .map(|scope_id| {
            uuid::Uuid::parse_str(&scope_id).unwrap_or_else(|e| {
                panic!("task_delegation_chain.scope_id is a UUID; read {scope_id:?}: {e}")
            })
        })
        .collect())
}

/// Every task, oldest first — read-only inspection before automation
/// (AGENTS.md: "Design read-only inspection... before automation").
///
/// Takes `&Store`, not `&mut Store`: per `Store::connection`'s own doc
/// comment, a read path must never go through `Store::transaction` (which
/// takes a write lock and would needlessly contend with real writers under
/// WAL).
pub fn list(store: &factory_store::Store) -> Result<Vec<Task>, TaskError> {
    let mut stmt = store
        .connection()
        .prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks ORDER BY created_at, id"
        ))
        .map_err(factory_store::StoreError::from)?;
    let rows = stmt
        .query_map([], row_to_task)
        .map_err(factory_store::StoreError::from)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| factory_store::StoreError::from(e).into())
}

/// One task by id — the other half of read-only inspection.
pub fn show(store: &factory_store::Store, id: uuid::Uuid) -> Result<Task, TaskError> {
    store
        .connection()
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
            [id.to_string()],
            row_to_task,
        )
        .optional()
        .map_err(factory_store::StoreError::from)?
        .ok_or(TaskError::NotFound(id))
}

/// Describe `valid_targets(from)` the way `factory_session::transition_in_tx`
/// formats its own `SessionError::InvalidTransition::allowed` field, for the
/// identical `TaskError::InvalidTransition` message shape.
fn describe_targets(from: TaskStatus) -> String {
    let allowed = valid_targets(from);
    if allowed.is_empty() {
        "nothing — this is a terminal state".to_string()
    } else {
        allowed
            .iter()
            .map(TaskStatus::to_string)
            .collect::<Vec<_>>()
            .join(" or ")
    }
}

/// The whole cancellation rule (design §2.4), in one place:
///
/// - a `queued` or `blocked` task moves straight to `cancelled` — nothing is
///   running anywhere, so there is no one to cooperate with;
/// - a `running` task instead gets `cancel_requested_at` set and **stays**
///   `running`; this function does not touch the session, its state, or its
///   lease. `factory_session::interrupt` is the wrong neighbour to reach for
///   here despite sharing the word "interrupt": that function models a
///   session that has *died* — it fails the session and releases its
///   workspace lease. This models the opposite situation: the session is
///   presumed alive and working, and design §2.4's "cooperative" cancellation
///   means the running agent itself is expected to notice the request and
///   report a terminal status; forcing the session down is "a separate user
///   action" the design explicitly declines to fold into cancellation;
/// - a task already in a terminal status is refused with
///   [`TaskError::AlreadyTerminal`] — cancelling something that already
///   finished is a caller bug worth surfacing, not a silent no-op.
///
/// Returns the task's resulting status: [`TaskStatus::Cancelled`] for the
/// immediate path, or [`TaskStatus::Running`] (unchanged) for the cooperative
/// one, so a caller can tell the two outcomes apart without a second read.
///
/// Setting `cancel_requested_at` is idempotent — a second cancel request
/// against an already-requested, still-running task leaves the original
/// timestamp in place (`COALESCE`) rather than overwriting it, so the
/// recorded time is always the *first* request, matching AGENTS.md's
/// "[m]ake mutations transactional and idempotent where retries... are
/// possible."
///
/// Only the immediate path writes a `cancelled` event (`crate`'s station-11
/// decision 3): recording the *request* on a running task changes no column
/// the schema's CHECK lists as material — `status` stays `running` — and
/// there is no `cancel_requested` entry in that list to write. The
/// `running → cancelled` edge itself is
/// [`crate::complete::acknowledge_cancellation`]'s, not this function's, and
/// that is where its own `cancelled` event is written.
pub fn cancel(store: &mut factory_store::Store, id: uuid::Uuid) -> Result<TaskStatus, TaskError> {
    let tx = store.transaction()?;

    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some(status) = status else {
        return Err(TaskError::NotFound(id));
    };
    let current = TaskStatus::from_db_str(&status);

    if current.is_terminal() {
        return Err(TaskError::AlreadyTerminal {
            id,
            status: current,
        });
    }

    let outcome = match current {
        TaskStatus::Queued | TaskStatus::Blocked => {
            if !is_valid_transition(current, TaskStatus::Cancelled) {
                return Err(TaskError::InvalidTransition {
                    id,
                    from: current,
                    to: TaskStatus::Cancelled,
                    allowed: describe_targets(current),
                });
            }
            tx.execute(
                "UPDATE tasks SET status = 'cancelled', blocked_reason = NULL, \
                 updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                [id.to_string()],
            )
            .map_err(factory_store::StoreError::from)?;
            crate::events::append(&tx, id, EventType::Cancelled, None, None)?;
            TaskStatus::Cancelled
        }
        TaskStatus::Running => {
            tx.execute(
                "UPDATE tasks SET \
                 cancel_requested_at = COALESCE(cancel_requested_at, CURRENT_TIMESTAMP), \
                 updated_at = CURRENT_TIMESTAMP \
                 WHERE id = ?1",
                [id.to_string()],
            )
            .map_err(factory_store::StoreError::from)?;
            TaskStatus::Running
        }
        TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled => {
            unreachable!("TaskStatus::is_terminal() already refused these above")
        }
    };

    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(outcome)
}

/// Where a cron run came from: the schedule that fired and the local minute
/// it fired for, exactly as `tasks.fired_for_minute` stores it.
///
/// Carried as one value rather than three loose arguments because migration
/// 6's `triggered_by` CHECK binds them together. A `cron` row must have both
/// a schedule and a minute; a `manual` row must have neither. There is no
/// legal fourth combination, so there is no way to express one here.
#[derive(Debug, Clone, Copy)]
pub struct CronOrigin<'a> {
    pub schedule_id: uuid::Uuid,
    pub fired_for_minute: &'a str,
}

/// What [`create_from_schedule`] did.
///
/// `AlreadyFired` is an outcome, not an error. ADR 0021 decision 3a: on the
/// autumn clock change both instants of a repeated local minute match, an
/// hour apart, so a dispatcher genuinely tries to fire twice and the database
/// rejects the second. A dispatcher told that was a failure would report a
/// fault once a year, at 02:30, to nobody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleFire {
    Fired(uuid::Uuid),
    AlreadyFired,
}

/// Create one cron run and stamp its schedule, in **one transaction**.
///
/// ADR 0021 decision 3 requires the run insert and `schedules.last_fired_at`
/// to commit together, and this is the only function that can honour it. The
/// dispatcher lives in `factory-daemon`, which has no direct `rusqlite`
/// dependency and cannot reach [`crate::events::append`] — so a dispatcher
/// assembling this from [`create_from_template`] plus a second transaction
/// leaves a window: a crash between the two commits strands a run with
/// `triggered_by = 'manual'` and no `schedule_id`, which
/// `tasks_one_run_per_schedule_minute` never indexes and no later tick can
/// see. The schedule would then fire again for the same minute, which is
/// exactly the acceptance criterion the index exists to hold.
///
/// Detecting "already fired" also belongs here rather than in the caller.
/// This crate has `rusqlite` and can match
/// [`rusqlite::ErrorCode::ConstraintViolation`] against the index by name; a
/// caller without the dependency could only compare error text, and the same
/// INSERT can fail the `triggered_by` CHECK or the `schedule_id` foreign key,
/// both real faults that must not be swallowed as an ordinary duplicate.
///
/// `last_fired_at` is stamped only on a run that was actually created. A
/// duplicate leaves it exactly as it was: a schedule that looks like it ran
/// and did not is worse than one that looks like it has never run.
#[allow(clippy::too_many_arguments)]
pub fn create_from_schedule(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    schedule_id: uuid::Uuid,
    template_id: uuid::Uuid,
    target_scope_id: uuid::Uuid,
    target_agent_name: Option<&str>,
    prompt: &str,
    fired_for_minute: &str,
    fired_at: chrono::DateTime<chrono::Utc>,
) -> Result<ScheduleFire, TaskError> {
    let _ = target_agent_name;
    let tx = store.transaction()?;

    let template_version: Option<i64> = tx
        .query_row(
            "SELECT version FROM task_templates WHERE id = ?1",
            [template_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;

    let inserted = insert_task(
        &tx,
        id,
        None,
        target_scope_id,
        None,
        None,
        prompt,
        &[target_scope_id],
        Some(template_id),
        template_version,
        Some(CronOrigin {
            schedule_id,
            fired_for_minute,
        }),
        None,
    );

    match inserted {
        Ok(()) => {}
        Err(e) if is_already_fired(&e) => {
            // Dropping `tx` rolls the whole attempt back, including the
            // `created` event, so a rejected duplicate leaves no trace at
            // all rather than half of one.
            return Ok(ScheduleFire::AlreadyFired);
        }
        Err(e) => return Err(e),
    }

    // `mark_fired` takes this transaction, so the stamp and the run commit
    // or roll back together. Its only failure is a schedule that vanished
    // between `due` and here, which the foreign key on the row just inserted
    // has already ruled out, so the error is surfaced rather than absorbed.
    crate::schedule::mark_fired(&tx, schedule_id, fired_at).map_err(|e| match e {
        crate::schedule::ScheduleError::Store(store) => TaskError::from(store),
        other => TaskError::ScheduleVanished(other.to_string()),
    })?;

    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(ScheduleFire::Fired(id))
}

/// Is this the one constraint that means "this schedule already produced a
/// run for this minute"?
///
/// Matched on `rusqlite`'s own structured error rather than on message text,
/// and narrowed to the index by name. The same INSERT can violate
/// `triggered_by`'s CHECK or the `schedule_id` foreign key, and both of those
/// are real faults a dispatcher must not silently treat as an ordinary
/// duplicate.
fn is_already_fired(error: &TaskError) -> bool {
    let TaskError::Store(factory_store::StoreError::Sqlite(rusqlite::Error::SqliteFailure(
        code,
        Some(message),
    ))) = error
    else {
        return false;
    };
    code.code == rusqlite::ErrorCode::ConstraintViolation
        && message.contains("tasks.schedule_id")
        && message.contains("tasks.fired_for_minute")
}
