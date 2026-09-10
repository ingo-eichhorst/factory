//! Check 6: two questions about scheduling, not one (ADR 0021 decision 10).
//!
//! **Question 1 — has Factory's own dispatcher ticked lately?** ADR 0021
//! decision 2 makes the dispatcher a thread the daemon owns. It writes
//! `dispatcher_state` on every tick, whether or not anything fired
//! (`factory_store::schema`'s own comment on the table). This module reads
//! that row, not `launchctl`, to answer the question.
//!
//! **Question 2 — is a foreign scheduler also touching this database?**
//! `com.business-factory.scheduler` is not Factory's label. It belongs to
//! the pre-station-11 Python prototype
//! (`scripts/factory_tasks.py dispatch --deliver`), measured directly on
//! 2026-09-10 (`launchctl list com.business-factory.scheduler` on this
//! machine returns a loaded job, `LastExitStatus = 0`, running that
//! command). Station 11 replaces the prototype; the cut-over is an operator
//! procedure this crate does not perform (ADR 0021 decision 10). Until an
//! operator runs it, this label being loaded is the *expected* state of an
//! instance that has not cut over yet, not a fault — the fault is a
//! *loaded* prototype next to a Factory dispatcher that is *also* ticking:
//! two writers against one database, which is what ADR 0014 forbids.
//!
//! # How the two answers combine into findings
//!
//! - No `dispatcher_state` row: no dispatcher has ever ticked against this
//!   database. Ordinary on a fresh instance. Not a finding.
//! - A row whose tick is stale: a dispatcher ran and stopped. A finding,
//!   regardless of whether the foreign label is loaded — a stale dispatcher
//!   is not dispatching, so it cannot also be the second writer the
//!   loaded-label finding warns about.
//! - A row whose tick is fresh, and the foreign label is loaded: two
//!   dispatchers against one database. A finding.
//! - A row whose tick is fresh, and the foreign label is not loaded: the
//!   healthy, cut-over state. Not a finding.

use crate::{CheckName, Finding};

/// The foreign, pre-station-11 Python prototype's `launchd` label — see the
/// module docs. Not Factory's own: Factory's dispatcher is a daemon thread
/// (ADR 0021 decision 2) and installs no `launchd` job.
pub const SCHEDULER_LABEL: &str = "com.business-factory.scheduler";

/// `dispatcher_state` does not exist before migration 6
/// (`factory_store`'s `schema::V6_SCHEMA`). A released migration is never
/// edited (ADR 0012 decision 2), so this fact cannot drift the way
/// [`factory_store::latest_schema_version`] can.
const MIN_SCHEMA: i64 = 6;

/// How long a tick may age before it counts as stale. The dispatcher ticks
/// several times a minute (ADR 0021 decision 2) and cron's own granularity
/// is one minute, so five minutes of silence means at least four minutes of
/// schedules were never evaluated — long enough that "the tick is merely
/// slow" stops being a plausible explanation.
const STALE_AFTER_SECONDS: i64 = 5 * 60;

/// The narrowest fact this crate needs from launchd: whether a label is
/// loaded, and its raw `launchctl list` output when it is.
pub trait LaunchdAccess {
    /// `Ok(Some(raw))` when `label` is loaded (`launchctl list <label>`
    /// exits successfully, with `raw` its stdout); `Ok(None)` when launchd
    /// reports no such service; `Err` for any other failure (the `launchctl`
    /// binary missing, an unreadable process failure).
    fn list(&self, label: &str) -> Result<Option<String>, String>;
}

/// Runs the real `launchctl list <label>`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemLaunchd;

impl LaunchdAccess for SystemLaunchd {
    fn list(&self, label: &str) -> Result<Option<String>, String> {
        let output = std::process::Command::new("launchctl")
            .args(["list", label])
            .output()
            .map_err(|source| format!("could not run `launchctl list {label}`: {source}"))?;
        if output.status.success() {
            Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
        } else {
            // Measured directly on 2026-09-09: an unknown label exits
            // non-zero (113 on this machine) with "Could not find service
            // ... in domain for port" on stderr. `launchctl`'s own exit
            // codes are not documented as stable API, so this treats any
            // non-zero exit as "not loaded" rather than pattern-matching a
            // specific number.
            Ok(None)
        }
    }
}

/// Check 6's report, always present in [`crate::DoctorReport`] regardless of
/// whether anything is wrong — mirroring [`crate::BackupsSummary`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerStatus {
    /// The foreign scheduler's label — see the module docs.
    pub label: String,
    /// Whether `label` is loaded under `launchctl`.
    pub loaded: bool,
    /// Factory's own dispatcher's last tick, read from `dispatcher_state`.
    /// `None` means either no dispatcher has ever ticked against this
    /// database, or the row could not be read — see
    /// [`Finding::CheckSkipped`] for which.
    pub last_tick_at: Option<String>,
}

/// Read `dispatcher_state.last_tick_at`, gated on schema.
///
/// This gate is load-bearing, not defensive dressing. A schema-5-or-lower
/// database — the real shape of a machine still running the foreign
/// prototype, whose own `factory_schema_migrations` table means
/// `PRAGMA user_version` reads 0 there — has no `dispatcher_state` table at
/// all. Reading it with a bare query would fail, and letting that failure
/// propagate out of `diagnose_with` would make the whole report come back
/// `Err` — doctor reporting nothing at all, on exactly the instance an
/// operator most needs it for. So this checks the schema version first and
/// reports [`Finding::CheckSkipped`] instead, the same pattern
/// `registry::check` and `panes::check` already use for the same reason.
fn read_last_tick(
    store: &factory_store::Store,
    schema_version: i64,
    findings: &mut Vec<Finding>,
) -> Option<String> {
    if schema_version < MIN_SCHEMA {
        findings.push(Finding::CheckSkipped {
            check: CheckName::DispatcherTick,
            reason: format!(
                "database schema is {schema_version}; `dispatcher_state` was introduced \
                 in schema {MIN_SCHEMA}"
            ),
        });
        return None;
    }

    use rusqlite::OptionalExtension;
    match store
        .connection()
        .query_row(
            "SELECT last_tick_at FROM dispatcher_state WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
    {
        // No row: no dispatcher has ever ticked against this database.
        // Ordinary on a fresh instance, not a finding — see the module
        // docs.
        Ok(row) => row,
        Err(err) => {
            findings.push(Finding::CheckSkipped {
                check: CheckName::DispatcherTick,
                reason: format!("could not read `dispatcher_state`: {err}"),
            });
            None
        }
    }
}

/// Decide the stale/two-dispatchers finding, if any, for a tick that was
/// actually read.
fn report_tick_health(
    last_tick_at: &str,
    foreign_scheduler_loaded: bool,
    findings: &mut Vec<Finding>,
) {
    let Ok(tick) = chrono::DateTime::parse_from_rfc3339(last_tick_at) else {
        // `record_tick` (factory-daemon) always writes `to_rfc3339()`; a
        // value that does not parse as one is a row this crate did not
        // write and cannot interpret, not a value to guess at.
        findings.push(Finding::CheckSkipped {
            check: CheckName::DispatcherTick,
            reason: format!(
                "`dispatcher_state.last_tick_at` is not a valid RFC 3339 timestamp: \
                 {last_tick_at:?}"
            ),
        });
        return;
    };

    let age_seconds = chrono::Utc::now().signed_duration_since(tick).num_seconds();

    // Compared on magnitude, not sign: a clock-skewed or corrupted row
    // could carry a timestamp in the future, and treating "in the future"
    // as automatically fresh would mask a dead dispatcher forever. The
    // signed value is still carried in the finding so a negative age reads
    // honestly rather than as an implausible multi-year staleness.
    if age_seconds.abs() > STALE_AFTER_SECONDS {
        findings.push(Finding::DispatcherTickStale {
            last_tick_at: last_tick_at.to_string(),
            age_seconds,
        });
    } else if foreign_scheduler_loaded {
        // A stale dispatcher already got its own finding above and is, by
        // definition, not dispatching — so it is never also reported as
        // the second writer here.
        findings.push(Finding::TwoDispatchers {
            label: SCHEDULER_LABEL.to_string(),
            last_tick_at: last_tick_at.to_string(),
        });
    }
}

pub(crate) fn check(
    store: &factory_store::Store,
    schema_version: i64,
    launchd: &dyn LaunchdAccess,
    findings: &mut Vec<Finding>,
) -> SchedulerStatus {
    let last_tick_at = read_last_tick(store, schema_version, findings);

    let loaded = match launchd.list(SCHEDULER_LABEL) {
        Ok(Some(_raw)) => true,
        Ok(None) => false,
        Err(error) => {
            findings.push(Finding::SchedulerQueryFailed {
                label: SCHEDULER_LABEL.to_string(),
                error,
            });
            false
        }
    };

    if let Some(tick) = last_tick_at.as_deref() {
        report_tick_health(tick, loaded, findings);
    }

    SchedulerStatus {
        label: SCHEDULER_LABEL.to_string(),
        loaded,
        last_tick_at,
    }
}
