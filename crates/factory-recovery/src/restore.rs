//! ADR 0019 decision 2's reconciliation: what a restored database is allowed
//! to claim about itself, with no live evidence consulted (ADR 0019 decision
//! 3 — see the crate docs' "the split this crate sits on"). Design §5 and
//! backlog §9 are the wider context; this module is the half of backlog §9
//! that "needs no live evidence," as the crate docs put it.
//!
//! [`reconcile`] writes **exactly** the two things ADR 0019 decision 2
//! specifies, in one transaction (ADR 0019 decision 1), and nothing else:
//!
//! 1. every lease-holding session becomes `disconnected`, keeping its lease —
//!    delegated to `factory_session::reconcile_to_disconnected`, the one
//!    dedicated entry point coordinator decision 2 created for exactly this
//!    caller and no other;
//! 2. every task is decided by the delivery journal, not by its status
//!    alone — implemented directly against `tasks` and `delivery_attempts`
//!    below, because that decision needs no session-side machinery.
//!
//! Coordinator decision 4 makes idempotence load-bearing, not a nice-to-have:
//! ADR 0019 decision 1 accepts that an operator who copies a snapshot into
//! place by hand gets stale live-looking rows with **no warning**, and the
//! only reason that is tolerable is that running [`reconcile`] afterwards
//! reaches the same state a normal `factory restore` would have. Every write
//! below is therefore guarded by "does this row already show the target
//! value" before it is attempted, so a row [`reconcile`] already fixed on a
//! previous call is left completely untouched — including its `updated_at` —
//! on the next one. See `reconcile_is_idempotent_a_second_run_changes_nothing`
//! in `tests/restore.rs`, and its mutation.

use std::path::{Path, PathBuf};

/// One session [`reconcile`] moved, with its state before and after.
///
/// Only sessions that actually changed appear here — a session already
/// `disconnected` when [`reconcile`] ran is not "changed to `disconnected`"
/// a second time, per this module's idempotence guarantee, and is therefore
/// absent from this list on that call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionChange {
    pub id: uuid::Uuid,
    pub before: factory_session::SessionState,
    pub after: factory_session::SessionState,
}

/// One task [`reconcile`] moved, with its status, blocked reason, and
/// assignment before and after — "before and after values," per ADR 0019
/// decision 2's last paragraph.
///
/// Read and written as plain strings, never `factory_task::TaskStatus`:
/// that type's `from_db_str`/`as_db_str` are `pub(crate)` to `factory-task`
/// (this crate cannot reach them), and `factory_session::on_task_terminal`
/// already establishes the precedent of a *different* crate reading
/// `tasks.status` as a raw string read from a row it does not own, rather
/// than duplicating another crate's private vocabulary type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskChange {
    pub id: uuid::Uuid,
    pub before_status: String,
    pub before_blocked_reason: Option<String>,
    pub before_assigned_session_id: Option<uuid::Uuid>,
    pub after_status: String,
    pub after_blocked_reason: Option<String>,
    pub after_assigned_session_id: Option<uuid::Uuid>,
}

/// ADR 0019 decision 2's audit record: "a report file, not a schema change."
///
/// `report_path` names where [`reconcile`] wrote it — under
/// `<db-directory>/restores/`, which is `.factory/restores/` for every
/// `Store` this crate expects to run against (see [`report_path_for`]'s doc
/// comment for exactly how that directory is derived and why that keeps this
/// module inside design §4's allowlist without reconstructing a company root
/// of its own).
///
/// `snapshot_user_version` is the schema version *of the database
/// [`reconcile`] just ran against* — `Store::schema_version`, read inside the
/// same call. ADR 0019 decision 2 asks the report to name "the snapshot,"
/// but `reconcile`'s signature (coordinator decision 1: no `factory restore`
/// command exists yet, so this crate is hunded the already-open `Store`, not
/// a snapshot path) gives this function no snapshot identity beyond the
/// database it was handed — reported plainly here rather than invented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    pub snapshot_user_version: i64,
    pub sessions: Vec<SessionChange>,
    pub tasks: Vec<TaskChange>,
    pub report_path: PathBuf,
}

/// Everything that can go wrong reconciling a restored database.
#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error("session error: {0}")]
    Session(#[from] factory_session::SessionError),

    #[error("cannot write restore report at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "database path {db_path} is not inside a `.factory/` directory\n  help: this reconciliation refuses to write its report file outside design §4's allowlist; open the store with `Store::open`, not `Store::open_at` against an arbitrary path"
    )]
    NotInDotFactory { db_path: PathBuf },
}

/// ADR 0019 decision 2, copied verbatim as this function's specification:
///
/// | Row | Becomes | Why |
/// |---|---|---|
/// | `running` | `blocked: interrupted` | Design §5: Factory does not automatically resend a possibly delivered prompt. |
/// | `queued` with at least one `delivery_attempts` row | `blocked: interrupted` | The attempt is journalled *before* the write (design §5 step 3), so this task may already have been delivered. Its status is the weaker evidence. |
/// | `queued` with no delivery attempt | stays `queued`, `assigned_session_id` cleared | Nothing was ever sent. Design §5's recovery table keeps queued work queued; clearing the assignment lets selection choose again. |
/// | `blocked` | unchanged | Already waiting for a human. |
/// | `done`, `failed`, `cancelled` | unchanged | Terminal. |
///
/// And, in the same decision, for sessions: "Every lease-holding session
/// becomes `disconnected`, and keeps its lease" — `starting`, `running`, and
/// `disconnected` all mean, after a restore, the same thing Factory can
/// express: it cannot see the process.
///
/// Runs entirely inside one `BEGIN IMMEDIATE` transaction (ADR 0019 decision
/// 1: "restoring is an explicit command... performs the reconciliation... in
/// one transaction"), consults nothing outside `store` (ADR 0019 decision 3
/// — no `factory_adapter` import appears anywhere in this file, which is the
/// direct, compile-time evidence for "does not consult Herdr or any
/// adapter"), and writes a plain-text report under `.factory/restores/`
/// after that transaction commits (see [`report_path_for`] and
/// [`render_report`]).
///
/// # Idempotence, row by row
///
/// - A lease-holding session already `disconnected` is left untouched by
///   `factory_session::reconcile_to_disconnected` itself (its own doc
///   comment covers this) and is therefore never pushed onto
///   [`RestoreReport::sessions`] on a call where nothing changed it.
/// - A `running` or `queued`-with-attempt task becomes `blocked` on the
///   first call; every later call reads `blocked` and falls through the
///   match's `_` arm untouched — `blocked` is a fixed point this function
///   never revisits, matching the table's own "blocked → unchanged" row.
/// - A `queued`-with-no-attempt task's `assigned_session_id` is cleared only
///   `if it is not already NULL`; once cleared, the second call sees `NULL`
///   and writes nothing.
///
/// # Errors
///
/// [`RecoveryError::Store`] or [`RecoveryError::Session`] if any read or
/// write inside the transaction fails — the transaction is never partially
/// committed (rusqlite rolls back on drop without an explicit `commit`).
/// [`RecoveryError::NotInDotFactory`] if `store.path()`'s directory is not
/// literally named `.factory` (see [`report_path_for`]) — checked, and
/// therefore returned, only after the transaction above has already
/// committed, so a `Store::open_at` caller outside `.factory/` still gets a
/// reconciled database and only loses the audit record. [`RecoveryError::Io`]
/// if the report file cannot be written for any other reason; by this point
/// the transaction has already committed, so the database is reconciled even
/// if the audit record could not be.
pub fn reconcile(store: &mut factory_store::Store) -> Result<RestoreReport, RecoveryError> {
    let db_path = store.path().to_path_buf();
    let snapshot_user_version = store.schema_version()?;

    let mut sessions = Vec::new();
    let mut tasks = Vec::new();

    {
        let tx = store.transaction()?;

        // --- Sessions: every lease-holding session becomes `disconnected`. ---
        let mut stmt = tx
            .prepare("SELECT id, state FROM sessions ORDER BY rowid")
            .map_err(factory_store::StoreError::from)?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(factory_store::StoreError::from)?
            .collect::<Result<_, _>>()
            .map_err(factory_store::StoreError::from)?;
        drop(stmt);

        for (id_str, state_str) in rows {
            let id = uuid::Uuid::parse_str(&id_str)
                .unwrap_or_else(|e| panic!("sessions.id is a UUID; read {id_str:?}: {e}"));
            let before = factory_session::SessionState::from_db_str(&state_str);
            if !before.holds_lease() {
                // `stopped` / `failed`: already what a restore would leave
                // them as, since neither ever held a lease for this
                // reconciliation to preserve.
                continue;
            }

            factory_session::reconcile_to_disconnected(&tx, id)?;

            if before != factory_session::SessionState::Disconnected {
                sessions.push(SessionChange {
                    id,
                    before,
                    after: factory_session::SessionState::Disconnected,
                });
            }
        }

        // --- Tasks: decided by the delivery journal, not status alone. ---
        let mut stmt = tx
            .prepare(
                "SELECT id, status, blocked_reason, assigned_session_id FROM tasks ORDER BY rowid",
            )
            .map_err(factory_store::StoreError::from)?;
        let rows: Vec<(String, String, Option<String>, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(factory_store::StoreError::from)?
            .collect::<Result<_, _>>()
            .map_err(factory_store::StoreError::from)?;
        drop(stmt);

        for (id_str, status, blocked_reason, assigned_session_id) in rows {
            let id = uuid::Uuid::parse_str(&id_str)
                .unwrap_or_else(|e| panic!("tasks.id is a UUID; read {id_str:?}: {e}"));
            let assigned_uuid = assigned_session_id.as_deref().map(|s| {
                uuid::Uuid::parse_str(s).unwrap_or_else(|e| {
                    panic!("tasks.assigned_session_id is a UUID; read {s:?}: {e}")
                })
            });

            match status.as_str() {
                "running" => {
                    tx.execute(
                        "UPDATE tasks SET status = 'blocked', blocked_reason = 'interrupted', \
                         updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                        [&id_str],
                    )
                    .map_err(factory_store::StoreError::from)?;
                    tasks.push(TaskChange {
                        id,
                        before_status: status,
                        before_blocked_reason: blocked_reason,
                        before_assigned_session_id: assigned_uuid,
                        after_status: "blocked".to_string(),
                        after_blocked_reason: Some("interrupted".to_string()),
                        after_assigned_session_id: assigned_uuid,
                    });
                }
                "queued" => {
                    // A refused attempt wrote nothing (see
                    // `factory_task::deliver::ATTEMPT_MAY_HAVE_REACHED_THE_TERMINAL`),
                    // so it is not the "weaker evidence" this row turns on.
                    let has_attempt: bool = tx
                        .query_row(
                            &format!(
                                "SELECT EXISTS(SELECT 1 FROM delivery_attempts \
                                 WHERE task_id = ?1 AND {predicate})",
                                predicate =
                                    factory_task::deliver::ATTEMPT_MAY_HAVE_REACHED_THE_TERMINAL
                            ),
                            [&id_str],
                            |row| row.get(0),
                        )
                        .map_err(factory_store::StoreError::from)?;

                    if has_attempt {
                        // The middle row: journalled before the terminal
                        // write (design §5 step 3), so `queued` here is the
                        // weaker evidence — this task may already have been
                        // delivered.
                        tx.execute(
                            "UPDATE tasks SET status = 'blocked', blocked_reason = 'interrupted', \
                             updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                            [&id_str],
                        )
                        .map_err(factory_store::StoreError::from)?;
                        tasks.push(TaskChange {
                            id,
                            before_status: status,
                            before_blocked_reason: blocked_reason,
                            before_assigned_session_id: assigned_uuid,
                            after_status: "blocked".to_string(),
                            after_blocked_reason: Some("interrupted".to_string()),
                            after_assigned_session_id: assigned_uuid,
                        });
                    } else if assigned_uuid.is_some() {
                        // Nothing was ever sent. Only write when there is an
                        // assignment to clear — see the idempotence section
                        // above: a second call must find this already NULL
                        // and touch nothing.
                        tx.execute(
                            "UPDATE tasks SET assigned_session_id = NULL, \
                             updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                            [&id_str],
                        )
                        .map_err(factory_store::StoreError::from)?;
                        tasks.push(TaskChange {
                            id,
                            before_status: status.clone(),
                            before_blocked_reason: blocked_reason,
                            before_assigned_session_id: assigned_uuid,
                            after_status: status,
                            after_blocked_reason: None,
                            after_assigned_session_id: None,
                        });
                    }
                    // else: `queued`, no attempt, no assignment — already
                    // exactly what this reconciliation would leave it as.
                }
                // `blocked`, `done`, `failed`, `cancelled`: unchanged.
                _ => {}
            }
        }

        tx.commit().map_err(factory_store::StoreError::from)?;
    }

    let report_path = report_path_for(store.connection(), &db_path)?;
    let report = RestoreReport {
        snapshot_user_version,
        sessions,
        tasks,
        report_path,
    };
    write_report(&report)?;

    Ok(report)
}

/// Where [`reconcile`] writes its audit record: `<dir>/restores/<timestamp>`,
/// where `<dir>` is `store.path()`'s own parent directory rather than a
/// company root this function reconstructs itself.
///
/// This is deliberately *not* `db_path.parent().parent().join(".factory")` or
/// any other re-derivation of where `.factory/` lives: `Store::open` always
/// opens `<company_root>/.factory/factory.sqlite`, so `db_path`'s parent *is*
/// `.factory/` for a `Store` opened that way, with nothing here needing to
/// know or assume where the company root itself is — `factory_store`'s own
/// path handling (`Store::path`) does the work, in the spirit of
/// `every_created_path_is_under_dot_factory` in
/// `crates/factory-store/tests/no_writes_outside_factory.rs`.
///
/// **Measured, not assumed — this is conditional on how `store` was opened,
/// not a fact about every `Store` this crate can be handed**, and is
/// therefore checked, not trusted: `Store::open_at`, whose own doc comment
/// says "used by tests and by the backup drill," accepts an arbitrary path
/// with no `.factory/` in it at all, so a `db_path` whose parent directory
/// is not literally named `.factory` is refused outright
/// ([`RecoveryError::NotInDotFactory`]) rather than silently writing a
/// report beside the database, wherever that is. This crate's own tests use
/// `Store::open`, which always satisfies the check; the guard exists for a
/// future caller that does not.
///
/// The timestamp comes from SQLite's own clock (`strftime('%Y%m%dT%H%M%SZ',
/// 'now')`), via `conn` (`store.connection()`, called only once `reconcile`'s
/// transaction has committed and released its borrow), rather than a new
/// `std::time` computation — this crate has no date-formatting dependency to
/// spend (`factory-recovery`'s `Cargo.toml` is coordinator-owned), and
/// `factory_store::Store` already has a live connection asking the same
/// underlying clock, so nothing here opens a second connection to the same
/// file. A numeric suffix is appended, and incremented, only if that name is
/// already taken — two restores in the same wall-clock second must not
/// silently overwrite one another's report, but must also not *fail*
/// reconciliation merely because a report collided; unlike
/// `Store::backup_to`, an audit record losing a race for its own filename is
/// not the same class of mistake as one snapshot silently overwriting
/// another.
fn report_path_for(conn: &rusqlite::Connection, db_path: &Path) -> Result<PathBuf, RecoveryError> {
    let dot_factory = db_path
        .parent()
        .expect("Store::path always names a file, so it always has a parent");
    if dot_factory.file_name() != Some(std::ffi::OsStr::new(".factory")) {
        return Err(RecoveryError::NotInDotFactory {
            db_path: db_path.to_path_buf(),
        });
    }
    let restores_dir = dot_factory.join("restores");
    std::fs::create_dir_all(&restores_dir).map_err(|source| RecoveryError::Io {
        path: restores_dir.clone(),
        source,
    })?;

    let base: String = conn
        .query_row("SELECT strftime('%Y%m%dT%H%M%SZ', 'now')", [], |row| {
            row.get(0)
        })
        .map_err(factory_store::StoreError::from)?;

    let mut candidate = restores_dir.join(&base);
    let mut suffix = 1u32;
    while candidate.exists() {
        candidate = restores_dir.join(format!("{base}-{suffix}"));
        suffix += 1;
    }
    Ok(candidate)
}

/// Render `report` as plain text and write it to `report.report_path`.
///
/// Plain text, not JSON: `factory-recovery`'s `Cargo.toml` carries no `serde`
/// dependency (coordinator-owned; not this agent's to add), and ADR 0019
/// decision 2's own justification for a report file over a table is "a
/// restore is an operator action with an operator reading its output" — a
/// human-readable file is the more direct answer to that than a
/// machine format with no reader in this slice.
fn render_report(report: &RestoreReport) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "Factory restore reconciliation (ADR 0019 decision 2)");
    let _ = writeln!(
        out,
        "database user_version: {}",
        report.snapshot_user_version
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "sessions changed: {}", report.sessions.len());
    for s in &report.sessions {
        let _ = writeln!(out, "  {} {} -> {}", s.id, s.before, s.after);
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "tasks changed: {}", report.tasks.len());
    for t in &report.tasks {
        let _ = writeln!(
            out,
            "  {} status {:?} -> {:?}; blocked_reason {:?} -> {:?}; assigned_session_id {:?} -> {:?}",
            t.id,
            t.before_status,
            t.after_status,
            t.before_blocked_reason,
            t.after_blocked_reason,
            t.before_assigned_session_id,
            t.after_assigned_session_id,
        );
    }

    out
}

fn write_report(report: &RestoreReport) -> Result<(), RecoveryError> {
    std::fs::write(&report.report_path, render_report(report)).map_err(|source| RecoveryError::Io {
        path: report.report_path.clone(),
        source,
    })
}
