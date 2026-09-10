//! Shared helpers for factory-recovery's integration tests.
//!
//! Every test builds a fresh temp database with [`open_store`] and seeds the
//! ADR 0019 post-reconciliation starting state directly with the helpers
//! below, rather than calling `crate::restore`'s reconciliation (another
//! agent's file, and not yet built) or `factory_session::begin_start` (which
//! cannot produce a `disconnected` row at all — see its own transition
//! table). This mirrors `factory-session/tests/common/mod.rs`'s own stance:
//! seed the row shape a slice needs directly, rather than depend on a
//! not-yet-built or out-of-scope producer of it.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;

use factory_adapter::{Adapter, AdapterError, Observation, PaneId, StartRequest, StartedSession};
use factory_store::Store;

/// A deterministic, distinct, syntactically valid UUID — mirrors
/// `factory-session/tests/common/mod.rs::uid` exactly, for the same reason:
/// `uuid` is pinned workspace-wide without the `v4` feature.
pub fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// A fresh, empty database in its own temp directory. Returns the
/// `TempDir` too, so the caller can keep it alive for the test's duration —
/// dropping it deletes the database out from under a `Store` still using it.
pub fn open_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open_at(dir.path().join("factory.sqlite")).expect("open store");
    (dir, store)
}

/// Insert a scope row directly, bypassing `factory-registry` (which this
/// crate does not depend on). Mirrors `factory-session`'s own helper.
pub fn seed_scope(store: &mut Store, seed: u32, name: &str, canonical_path: &Path) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO scopes (id, name, declared_path, canonical_path) VALUES (?1, ?2, ?3, ?3)",
        (
            id.to_string(),
            name,
            canonical_path.to_string_lossy().into_owned(),
        ),
    )
    .expect("insert scope");
    tx.commit().expect("commit");
    id
}

/// Insert a `disconnected` session with its lease held — the exact shape
/// ADR 0019 decision 2's reconciliation leaves behind for every lease-holding
/// session, plus the migration-5 correlation columns this slice reads. The
/// accompanying open `workspace_leases` row is what
/// `factory_session::{fail, interrupt}` actually release when this crate
/// gives up on the session, and what `lease_released_at` below reads back.
#[allow(clippy::too_many_arguments)]
pub fn seed_disconnected_session(
    store: &mut Store,
    seed: u32,
    scope_id: uuid::Uuid,
    agent_name: &str,
    workspace_path: &Path,
    herdr_pane_id: Option<&str>,
    harness_session_id: Option<&str>,
) -> uuid::Uuid {
    let id = uid(seed);
    let workspace_path = workspace_path.to_string_lossy().into_owned();
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO sessions \
         (id, scope_id, agent_name, workspace_path, state, herdr_pane_id, harness_session_id) \
         VALUES (?1, ?2, ?3, ?4, 'disconnected', ?5, ?6)",
        (
            id.to_string(),
            scope_id.to_string(),
            agent_name,
            &workspace_path,
            herdr_pane_id,
            harness_session_id,
        ),
    )
    .expect("insert session");
    tx.execute(
        "INSERT INTO workspace_leases (session_id, canonical_workspace_path) VALUES (?1, ?2)",
        (id.to_string(), &workspace_path),
    )
    .expect("insert open lease");
    tx.commit().expect("commit");
    id
}

/// Insert a task row directly. `assigned_session_id` is the column
/// `factory-recovery`'s reconnection functions actually key off of to find
/// the one non-terminal task tied to a disconnected session; ADR 0019's own
/// reconciliation is what leaves it populated on a rewritten `blocked:
/// interrupted` row in the real system, so tests reproduce that shape by
/// hand.
pub fn seed_task(
    store: &mut Store,
    seed: u32,
    target_scope_id: uuid::Uuid,
    status: &str,
    blocked_reason: Option<&str>,
    assigned_session_id: Option<uuid::Uuid>,
) -> uuid::Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, assigned_session_id, prompt, status, blocked_reason) \
         VALUES (?1, ?2, ?3, 'do the thing', ?4, ?5)",
        (
            id.to_string(),
            target_scope_id.to_string(),
            assigned_session_id.map(|s| s.to_string()),
            status,
            blocked_reason,
        ),
    )
    .expect("insert task");
    tx.commit().expect("commit");
    id
}

/// Journal a delivery attempt directly, for tests that need a task to already
/// carry delivery history (ADR 0019 decision 2's "queued with at least one
/// `delivery_attempts` row" case, and the "recorded delivery history"
/// backlog §9 asks [`GiveUpOutcome`] to leave intact).
/// A *refused* attempt: journalled, but with the outcome a writer records
/// when it established that nothing reached the terminal. Excluded from every
/// "possibly delivered" question by
/// `factory_task::deliver::ATTEMPT_MAY_HAVE_REACHED_THE_TERMINAL`.
pub fn seed_refused_delivery_attempt(
    store: &mut Store,
    task_id: uuid::Uuid,
    session_id: uuid::Uuid,
) {
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO delivery_attempts (task_id, session_id, outcome) VALUES (?1, ?2, 'refused')",
        (task_id.to_string(), session_id.to_string()),
    )
    .expect("insert refused delivery attempt");
    tx.commit().expect("commit");
}

pub fn seed_delivery_attempt(store: &mut Store, task_id: uuid::Uuid, session_id: uuid::Uuid) {
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO delivery_attempts (task_id, session_id, outcome) VALUES (?1, ?2, 'sent')",
        (task_id.to_string(), session_id.to_string()),
    )
    .expect("insert delivery attempt");
    tx.commit().expect("commit");
}

/// `sessions.state` for `id`, read directly — this crate's tests must not
/// depend on `factory_session::show` for this, so that a bug in this crate's
/// own writes cannot be masked by a reader built by the same crate.
pub fn session_state(store: &Store, id: uuid::Uuid) -> String {
    store
        .connection()
        .query_row(
            "SELECT state FROM sessions WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("session row must exist")
}

/// `sessions.harness_session_id` for `id`, read directly — for tests
/// exercising `factory_recovery::reconnect`'s migration-5 writer.
pub fn session_harness_session_id(store: &Store, id: uuid::Uuid) -> Option<String> {
    store
        .connection()
        .query_row(
            "SELECT harness_session_id FROM sessions WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("session row must exist")
}

/// `(status, blocked_reason)` for a task, read directly.
pub fn task_status(store: &Store, id: uuid::Uuid) -> (String, Option<String>) {
    store
        .connection()
        .query_row(
            "SELECT status, blocked_reason FROM tasks WHERE id = ?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("task row must exist")
}

/// `tasks.assigned_session_id` for a task, read directly.
pub fn task_assigned_session_id(store: &Store, id: uuid::Uuid) -> Option<String> {
    store
        .connection()
        .query_row(
            "SELECT assigned_session_id FROM tasks WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("task row must exist")
}

/// Whether every `workspace_leases` row for `session_id` has been released —
/// `true` only once none remain open, which is what "the stale lease was
/// recovered" means in this table's own terms.
pub fn every_lease_released(store: &Store, session_id: uuid::Uuid) -> bool {
    let open: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM workspace_leases WHERE session_id = ?1 AND released_at IS NULL",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .expect("query workspace_leases");
    open == 0
}

/// How many `delivery_attempts` rows exist for `task_id`.
pub fn delivery_attempts_count(store: &Store, task_id: uuid::Uuid) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM delivery_attempts WHERE task_id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .expect("query delivery_attempts")
}

/// How many `sessions` rows exist in total — used to prove idempotent
/// re-runs create no extra row.
pub fn session_count(store: &Store) -> i64 {
    store
        .connection()
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .expect("query sessions")
}

/// A `factory_adapter::Adapter` built from recorded, per-pane responses
/// rather than a terminal multiplexer — mirrors `factory-adapter`'s own
/// `FakeHerdr`, one level up the trait stack (`Adapter`, not `HerdrAccess`),
/// since backlog §9 asks this slice's tests to "feed observations directly,
/// exactly as the adapter's own tests do."
pub struct FakeAdapter {
    observations: HashMap<String, Observation>,
    errors: HashMap<String, String>,
}

impl FakeAdapter {
    pub fn new() -> Self {
        Self {
            observations: HashMap::new(),
            errors: HashMap::new(),
        }
    }

    #[must_use]
    pub fn with_observation(mut self, pane: &str, observation: Observation) -> Self {
        self.observations.insert(pane.to_string(), observation);
        self
    }

    /// `observe(pane)` returns `Err` for this pane — the adapter itself
    /// failed, as distinct from an `Ok(Observation { confidence:
    /// Unavailable, .. })` reading (see `with_observation` for that case).
    #[must_use]
    pub fn with_error(mut self, pane: &str, detail: &str) -> Self {
        self.errors.insert(pane.to_string(), detail.to_string());
        self
    }
}

impl Adapter for FakeAdapter {
    // ADR 0017 decision 5, unchanged by the Slice-10 expansion of `Adapter`:
    // this crate's recovery paths only ever call `observe`. `start`, `send`,
    // `interrupt`, `stop`, and `attach_command` are documented operator
    // procedures here, not something a recovery test should ever reach — so
    // `FakeAdapter` implements them as loud failures rather than quiet
    // no-ops, exactly like this file's own `agent_explain must not be
    // called` fakes elsewhere in the crate: a recovery test that starts
    // calling one of these must fail immediately, not silently pass.
    fn start(&self, _req: &StartRequest) -> Result<StartedSession, AdapterError> {
        unreachable!("factory-recovery's tests never call Adapter::start")
    }

    fn send(
        &self,
        _pane: &PaneId,
        _task_id: uuid::Uuid,
        _prompt: &str,
    ) -> Result<(), AdapterError> {
        unreachable!("factory-recovery's tests never call Adapter::send")
    }

    fn interrupt(&self, _pane: &PaneId) -> Result<(), AdapterError> {
        unreachable!("factory-recovery's tests never call Adapter::interrupt")
    }

    fn stop(&self, _pane: &PaneId) -> Result<(), AdapterError> {
        unreachable!("factory-recovery's tests never call Adapter::stop")
    }

    fn attach_command(&self, _pane: &PaneId) -> Result<Vec<String>, AdapterError> {
        unreachable!("factory-recovery's tests never call Adapter::attach_command")
    }

    fn observe(&self, pane: &PaneId) -> Result<Observation, AdapterError> {
        if let Some(detail) = self.errors.get(&pane.0) {
            return Err(AdapterError::UnreadableOutput {
                command: format!("fake observe {}", pane.0),
                detail: detail.clone(),
                help: "fix the test fixture".to_string(),
            });
        }
        self.observations
            .get(&pane.0)
            .cloned()
            .ok_or_else(|| AdapterError::UnreadableOutput {
                command: format!("fake observe {}", pane.0),
                detail: "test fixture has no response for this pane".to_string(),
                help: "fix the test fixture".to_string(),
            })
    }

    fn runtime_version(&self) -> Result<String, AdapterError> {
        Ok("fake 0.0.0".to_string())
    }

    /// This double reports no cost data. Stated rather than omitted: the
    /// trait has no default, so an implementor cannot answer `None` by
    /// forgetting the method. ADR 0021 decision 6 makes `None` a valid
    /// answer, and it is this fake's real one.
    fn cost_sample(
        &self,
        _pane: &PaneId,
    ) -> Result<Option<factory_adapter::CostSample>, AdapterError> {
        Ok(None)
    }
}

/// An authoritative observation confirming the same session, built from a
/// harness session id so tests can control identity correlation explicitly.
pub fn authoritative_observation(pane: &str, harness_session_id: Option<&str>) -> Observation {
    Observation {
        pane: PaneId(pane.to_string()),
        harness_state: "idle".to_string(),
        confidence: factory_adapter::Confidence::Authoritative,
        session_alive: true,
        task_signal: factory_adapter::TaskSignal::NoChange,
        transcript_path: None,
        harness_session_id: harness_session_id.map(str::to_string),
    }
}

/// A degraded observation — Herdr answered, but without hook authority.
/// Never sufficient to promote a session; see `evidence::may_promote_from_disconnected`.
pub fn degraded_observation(pane: &str) -> Observation {
    Observation {
        pane: PaneId(pane.to_string()),
        harness_state: "working".to_string(),
        confidence: factory_adapter::Confidence::Degraded,
        session_alive: true,
        task_signal: factory_adapter::TaskSignal::NoChange,
        transcript_path: None,
        harness_session_id: None,
    }
}
