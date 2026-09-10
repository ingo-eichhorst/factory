//! Read-only diagnosis of an installed instance.
//!
//! # Binding decisions for station 10
//!
//! **1. It must work when the daemon is down.** That is the condition an
//! operator runs it to find out about, so this is the one component that opens
//! the database outside the daemon (ADR 0014, consequences). It opens it
//! read-only, through [`factory_store::Store::open_read_only`] — ADR 0018
//! decision 1 exists for exactly this. `Store::open_at` migrates
//! unconditionally, so a doctor built on it would change the schema before
//! reporting it.
//!
//! **2. It reports and never repairs.** Slice 9 kept explicit review, resume
//! and replacement actions for changing state. A repairing doctor would be a
//! second way to change the same state, and the two would drift.
//!
//! **3. A finding means a non-zero exit,** so the whole command is usable from
//! a check without parsing its output.
//!
//! # The six checks (backlog §10)
//!
//! 1. [`config`] — configuration validity for every registered scope.
//! 2. [`schema`] and [`backups`] — database integrity and schema version,
//!    whether migrations are pending (ADR 0018), and the count/size/age span
//!    of `.factory/backups/` (ADR 0019 decision 5).
//! 3. [`registry`] — registry drift (ADR 0016).
//! 4. [`leases`] — leases held with no live session.
//! 5. [`panes`] — database sessions against actual Herdr panes.
//! 6. [`scheduler`] — two questions, not one (ADR 0021 decision 10):
//!    whether Factory's own dispatcher has ticked lately, from
//!    `dispatcher_state`; and whether the foreign, pre-station-11
//!    scheduler is loaded under `launchctl`. A loaded foreign scheduler
//!    next to a fresh Factory tick is two dispatchers against one
//!    database, which ADR 0014 forbids.
//!
//! Every check that can run without the others still running does: a broken
//! configuration does not stop the schema check, a schema behind this build
//! does not stop the scheduler check. Where a later check genuinely cannot
//! run without an earlier one's result (registry drift needs a valid
//! configuration; registry drift, the pane audit, and the dispatcher tick
//! need schema columns or tables that do not exist before a specific
//! migration), that is reported as
//! [`Finding::CheckSkipped`] rather than silently omitted or allowed to
//! crash the whole pass — "I could not check X" is exactly the kind of gap
//! the backlog says nothing reveals today, so it is itself a finding.
//!
//! This crate never prints and never calls [`std::process::exit`]; it
//! returns [`DoctorReport`], whose [`DoctorReport::is_healthy`] is what a
//! caller (the `factory-cli` crate) uses to decide the process exit code.

mod backups;
mod config;
mod leases;
mod panes;
mod registry;
mod scheduler;
mod schema;

pub use backups::BackupsSummary;
pub use factory_store::latest_schema_version;
pub use panes::{LivePanes, SystemHerdr};
pub use scheduler::{LaunchdAccess, SCHEDULER_LABEL, SchedulerStatus, SystemLaunchd};

use std::path::Path;

/// Everything [`diagnose`] found wrong, one entry per problem.
///
/// Carries its own payload rather than a formatted string, so a caller (the
/// CLI) can render each variant however it likes and so a test can assert
/// the exact fact a check found rather than matching against rendered text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// Check 1. `.factory/config.yaml` failed to load or validate —
    /// including simply being missing or unreadable, which
    /// [`factory_config::load`] reports the same way it reports a validation
    /// problem.
    ConfigInvalid(factory_config::ConfigError),

    /// Check 2. `PRAGMA integrity_check` returned something other than
    /// `"ok"`, carried verbatim.
    IntegrityCheckFailed(String),

    /// Check 2 / ADR 0018. The database's schema is behind what this build
    /// understands: an update was installed and nothing has migrated the
    /// database yet (nothing has opened it read-write since).
    SchemaBehindBuild {
        database_schema: i64,
        built_schema: i64,
    },

    /// Check 2 / ADR 0018. The database's schema is ahead of what this
    /// build understands: an older build is running against a database a
    /// newer one already migrated.
    SchemaAheadOfBuild {
        database_schema: i64,
        built_schema: i64,
    },

    /// A check that depends on another finding, or on a schema column that
    /// does not exist yet, could not run. Reported explicitly rather than
    /// silently omitted or allowed to abort the whole pass.
    CheckSkipped { check: CheckName, reason: String },

    /// Check 3 / ADR 0016. One disagreement between the configuration, the
    /// `scopes` table, and the filesystem — the whole
    /// [`factory_registry::Drift`] report, not filtered to any one variant:
    /// ADR 0016 draws the "never applied automatically, a human must look"
    /// line itself, and filtering here would silently drop the variants
    /// that line names.
    RegistryDrift(factory_registry::Drift),

    /// Check 3. `factory_registry::resolve` or `::reconcile` itself failed
    /// (for example a scope declares a path that escapes the instance
    /// root). Stringified because `factory_registry::RegistryError` does not
    /// implement `PartialEq`.
    RegistryUnresolvable(String),

    /// Check 4. A `workspace_leases` row has `released_at IS NULL` (held),
    /// but the session that acquired it is no longer in a lease-holding
    /// state.
    OrphanedLease {
        session_id: uuid::Uuid,
        agent_name: String,
        session_state: factory_session::SessionState,
        workspace_path: String,
        lease_id: i64,
    },

    /// Check 5. The database calls this session live (its state holds a
    /// lease) and it names a pane, but that pane no longer exists in
    /// Herdr's own pane list.
    SessionPaneGone {
        session_id: uuid::Uuid,
        pane_id: String,
    },

    /// Check 5, the reverse direction. The database does not call this
    /// session live (its state no longer holds a lease), but the pane it
    /// last recorded is still alive in Herdr — Factory believes the session
    /// is gone while its terminal pane is not.
    PaneStillAliveForDeadSession {
        session_id: uuid::Uuid,
        pane_id: String,
    },

    /// Check 5. Herdr could not be queried at all (not running, binary
    /// missing, unreadable output), so the pane audit could not run for any
    /// session.
    HerdrPaneQueryFailed(String),

    /// Check 6. `launchctl` itself could not be queried.
    SchedulerQueryFailed { label: String, error: String },

    /// Check 6. `dispatcher_state.last_tick_at` is older (in either
    /// direction — see `scheduler::report_tick_health`) than the staleness
    /// window. A dispatcher ran against this database and stopped ticking.
    /// `age_seconds` is signed: negative means the timestamp is in the
    /// future (clock skew or a corrupted row), carried honestly rather than
    /// folded into a positive number.
    DispatcherTickStale {
        last_tick_at: String,
        age_seconds: i64,
    },

    /// Check 6. The foreign scheduler (`label`, [`SCHEDULER_LABEL`]) is
    /// loaded under `launchctl`, and Factory's own dispatcher has a fresh
    /// tick at `last_tick_at`. Two writers against one database — ADR 0021
    /// decision 10, ADR 0014.
    TwoDispatchers { label: String, last_tick_at: String },
}

/// A check that [`Finding::CheckSkipped`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckName {
    RegistryDrift,
    SessionPaneAudit,
    /// Check 6's `dispatcher_state` read — see `scheduler::read_last_tick`.
    DispatcherTick,
}

/// What [`diagnose`] found: the facts it always reports (schema version,
/// integrity verdict, backup directory summary, scheduler status) alongside
/// every [`Finding`] — the empty case included, since a clean instance is a
/// real, reportable outcome and not the absence of a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    /// `PRAGMA user_version`, read through the read-only door.
    pub schema_version: i64,
    /// `PRAGMA integrity_check`'s verdict, verbatim. `"ok"` means healthy;
    /// anything else is also carried as [`Finding::IntegrityCheckFailed`].
    pub integrity_check: String,
    /// ADR 0019 decision 5: count, total size, oldest and newest snapshot of
    /// `.factory/backups/`. Purely informational — Factory deletes nothing
    /// from that directory and no threshold makes its size itself a finding.
    pub backups: BackupsSummary,
    pub scheduler: SchedulerStatus,
    pub findings: Vec<Finding>,
}

impl DoctorReport {
    /// Binding decision 3: a finding means a non-zero exit. The CLI calls
    /// this to choose its exit code; this crate never decides the exit code
    /// itself and never prints.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Everything that stops [`diagnose`] from producing a [`DoctorReport`] at
/// all, as opposed to a report that lists something wrong.
///
/// Deliberately small: only a failure that makes *every* check meaningless
/// belongs here. The database itself is that failure — schema, integrity,
/// leases and the pane audit all read through it — so a missing or
/// unopenable database is the one hard stop. A missing or invalid
/// `config.yaml`, an unreachable Herdr, or an unqueryable `launchctl` are
/// not: each is instead a [`Finding`], because the other checks still have
/// something useful to say.
#[derive(Debug, thiserror::Error)]
pub enum DoctorError {
    #[error("could not open the database read-only: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error("could not read session or lease records: {0}")]
    Session(#[from] factory_session::SessionError),
}

/// Diagnose the Factory instance rooted at `company_root`, against the real
/// `herdr` and `launchctl` binaries.
///
/// # Errors
///
/// [`DoctorError`] if `<company_root>/.factory/factory.sqlite` does not
/// exist or cannot be opened read-only.
pub fn diagnose(company_root: &Path) -> Result<DoctorReport, DoctorError> {
    diagnose_with(company_root, &SystemHerdr, &SystemLaunchd)
}

/// [`diagnose`], with the Herdr and launchd queries supplied by the caller —
/// what lets tests exercise checks 5 and 6 without a real terminal
/// multiplexer or a real `launchctl`, and what a future caller uses to
/// supply a `factory_adapter`-backed [`LivePanes`] once one exists.
///
/// # Errors
///
/// See [`diagnose`].
pub fn diagnose_with(
    company_root: &Path,
    herdr: &dyn LivePanes,
    launchd: &dyn LaunchdAccess,
) -> Result<DoctorReport, DoctorError> {
    let db_path = company_root.join(".factory").join("factory.sqlite");
    let store = factory_store::Store::open_read_only(&db_path)?;

    let schema_version = store.schema_version()?;
    let integrity_check = store.integrity_check()?;
    let backups = backups::summarize(&db_path);

    let mut findings = Vec::new();

    match schema::compare(schema_version) {
        schema::SchemaComparison::UpToDate => {}
        schema::SchemaComparison::Behind => findings.push(Finding::SchemaBehindBuild {
            database_schema: schema_version,
            built_schema: factory_store::latest_schema_version(),
        }),
        schema::SchemaComparison::Ahead => findings.push(Finding::SchemaAheadOfBuild {
            database_schema: schema_version,
            built_schema: factory_store::latest_schema_version(),
        }),
    }

    if integrity_check != "ok" {
        findings.push(Finding::IntegrityCheckFailed(integrity_check.clone()));
    }

    let config = match config::check(company_root) {
        Ok(cfg) => Some(cfg),
        Err(err) => {
            findings.push(Finding::ConfigInvalid(err));
            None
        }
    };

    registry::check(
        company_root,
        &store,
        schema_version,
        config.as_ref(),
        &mut findings,
    );

    leases::check(&store, &mut findings)?;

    panes::check(&store, schema_version, herdr, &mut findings)?;

    let scheduler = scheduler::check(&store, schema_version, launchd, &mut findings);

    Ok(DoctorReport {
        schema_version,
        integrity_check,
        backups,
        scheduler,
        findings,
    })
}
