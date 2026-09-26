//! L1 Backup (`#116`): the page's report, a backup taken on request or on
//! the schedule, retention after it, verification, and owner-only restore
//! into a new root.
//!
//! The file work is `archive.rs`'s and the record of what happened is
//! `store.rs`'s; everything that decides -- retention, how old is too old,
//! which warnings hold -- is pure and lives in `factory_core::backup`. This
//! module only gathers the facts those decisions are made from, and runs the
//! daemon job.
//!
//! **Never takes the daemon down.** A destination that is missing, full or
//! read-only, a database that will not copy, an archive that will not write:
//! each is a `backup_failed` event with the reason, a row in the store, a
//! warning on the page and a line in the log -- never a panic out of the
//! job, and never a partial archive left where it could be mistaken for a
//! backup.

pub mod archive;
mod repos;
pub mod store;

pub use store::BackupStore;

use chrono::{DateTime, Duration, Utc};
use factory_core::backup::{
    age_level, parse_archive_name, refuse_bad_snapshot_name, retain, warnings, AgeLevel, BackupConfig,
    BackupFailure, BackupReport, BackupTrigger, CheckStatus, DestinationFacts, ExcludeRow, Group, IncludeRow,
    KeptBy, ManifestInstance, Restoration, Snapshot, SnapshotRow, Verification, VerifySummary, WarningFacts, AUTHORED,
    EXCLUDED, GRACE_HOURS, OPTIONAL, UNSCHEDULED_OVERDUE_HOURS, UNSCHEDULED_STALE_HOURS,
};
use factory_core::config::{Factory, CONFIG_FILE, FACTORY_DIR};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::Engine;
use store::Recorded;

/// How often the job looks at the clock. A backup is due at most once a
/// night, so a minute's lateness is nothing, and a look is cheap.
const JOB_TICK: std::time::Duration = std::time::Duration::from_secs(60);

/// One archive of this instance found in the destination.
#[derive(Debug, Clone)]
struct Found {
    name: String,
    at: DateTime<Utc>,
    size_bytes: u64,
}

/// Every archive of this instance in `destination`, newest first -- only
/// names `parse_archive_name` accepts, so nothing else in a shared folder is
/// ever listed, verified or deleted.
fn list_archives(destination: &Path, instance: &str) -> Vec<Found> {
    let Ok(read) = std::fs::read_dir(destination) else { return Vec::new() };
    let mut found: Vec<Found> = read
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let at = parse_archive_name(instance, &name)?;
            let meta = entry.metadata().ok().filter(|m| m.is_file())?;
            Some(Found { name, at, size_bytes: meta.len() })
        })
        .collect();
    found.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.name.cmp(&a.name)));
    found
}

/// What retention says about each archive, in the same order. Days, weeks
/// and months are the schedule's timezone's, so a 03:00 Berlin backup is
/// the day's backup in Berlin.
fn kept_by(found: &[Found], config: &BackupConfig) -> Vec<Vec<KeptBy>> {
    let tz = config
        .schedule
        .as_ref()
        .and_then(|s| s.timezone.as_deref())
        .and_then(|name| crate::schedule::timezone(name).ok());
    let dates: Vec<chrono::NaiveDate> = found
        .iter()
        .map(|f| match tz {
            Some(tz) => f.at.with_timezone(&tz).date_naive(),
            None => f.at.date_naive(),
        })
        .collect();
    retain(&dates, &config.keep)
}

/// The facts the page and `WarningFacts` need about the destination. The
/// device is only compared when the destination exists: the parent of a
/// missing mount point is on the system disk, and saying "same device"
/// then would be a guess.
fn destination_facts(root: &Path, destination: &Path) -> DestinationFacts {
    use std::os::unix::fs::MetadataExt;
    let exists = destination.is_dir();
    let same_device = if exists {
        match (std::fs::metadata(root), std::fs::metadata(destination)) {
            (Ok(r), Ok(d)) => Some(r.dev() == d.dev()),
            _ => None,
        }
    } else {
        None
    };
    let disk = exists
        .then(|| destination.to_str().and_then(crate::host::disk))
        .flatten();
    DestinationFacts {
        path: destination.display().to_string(),
        exists,
        same_device,
        free_bytes: disk.as_ref().map(|d| d.free_bytes),
        total_bytes: disk.as_ref().map(|d| d.total_bytes),
    }
}

/// `(due_by, overdue_by)` for the newest backup: the schedule's next slot
/// after it, and the one after that, each plus grace; or, unscheduled, the
/// fixed yardsticks.
fn deadlines(config: &BackupConfig, newest: DateTime<Utc>) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let grace = Duration::hours(GRACE_HOURS);
    match &config.schedule {
        Some(schedule) => {
            let schedule = schedule.as_task_schedule();
            let first = crate::schedule::next_after(&schedule, newest).ok();
            let second = first.and_then(|f| crate::schedule::next_after(&schedule, f).ok());
            (first.map(|t| t + grace), second.map(|t| t + grace))
        }
        None => (
            Some(newest + Duration::hours(UNSCHEDULED_STALE_HOURS)),
            Some(newest + Duration::hours(UNSCHEDULED_OVERDUE_HOURS)),
        ),
    }
}

/// Refuse at start a schedule the job could never fire, the same way an
/// unregistered knowledge provider is refused there -- in front of whoever
/// started the daemon, not as a `backup_failed` every minute afterwards.
pub fn validate_schedule(factory: &Factory) -> Result<()> {
    if let Some(schedule) = factory.config.infrastructure.backup.as_ref().and_then(|b| b.schedule.as_ref()) {
        crate::schedule::next_after(&schedule.as_task_schedule(), Utc::now()).map_err(|e| {
            FactoryError::BadRequest(format!("infrastructure.backup.schedule: {e}"))
        })?;
    }
    Ok(())
}

impl Engine {
    fn backup_config(&self) -> Result<(Factory, BackupConfig)> {
        let factory = self.factory_snapshot();
        let config = factory.config.infrastructure.backup.clone().ok_or_else(|| {
            FactoryError::BadRequest(
                "no backup is configured; add infrastructure.backup (destination, schedule, keep) to the root \
                 .factory/config.yaml and restart the daemon"
                    .into(),
            )
        })?;
        Ok((factory, config))
    }

    /// The newest attempt, successful or not, and the newest archive in the
    /// destination: whichever is later is when the schedule counts from.
    async fn last_attempt(&self, factory: &Factory, config: &BackupConfig) -> Option<DateTime<Utc>> {
        let recorded = self.backups.all().await.unwrap_or_default();
        let attempted = recorded
            .iter()
            .filter(|r| matches!(r, Recorded::Completed { .. } | Recorded::Failed { .. }))
            .map(|r| r.at())
            .max();
        let destination = config.destination.clone();
        let instance = factory.config.instance.name.clone();
        let newest = tokio::task::spawn_blocking(move || list_archives(&destination, &instance).first().map(|f| f.at))
            .await
            .ok()
            .flatten();
        attempted.max(newest)
    }

    /// `GET /api/backup`. Never fails for a destination that is missing or
    /// unreadable: that is a fact the report carries, and a warning.
    pub(crate) async fn backup_report(&self) -> Result<BackupReport> {
        let now = Utc::now();
        let factory = self.factory_snapshot();
        let running = self.backup_busy.try_lock().is_err();
        let recorded = self.backups.all().await?;
        // `#155`: gathered whether or not a backup is even configured --
        // source code is backed up by pushing it, not by this snapshot --
        // so both branches below carry them.
        let (code, time_machine) = tokio::join!(
            repos::repository_facts(&factory.root, &factory.config.scopes),
            repos::time_machine_fact(),
        );
        let Some(config) = factory.config.infrastructure.backup.clone() else {
            return Ok(BackupReport {
                now,
                config: None,
                destination: None,
                age: AgeLevel::None,
                due_by: None,
                next_run: None,
                running,
                last_verified: None,
                last_failure: None,
                warnings: warnings(&WarningFacts { code: code.clone(), time_machine: Some(time_machine.clone()), ..Default::default() }),
                snapshots: Vec::new(),
                include: include_rows(false, None),
                exclude: exclude_rows(false),
                code,
                time_machine: Some(time_machine),
            });
        };

        let (root, destination, instance) =
            (factory.root.clone(), config.destination.clone(), factory.config.instance.name.clone());
        let (facts, found) = tokio::task::spawn_blocking(move || {
            (destination_facts(&root, &destination), list_archives(&destination, &instance))
        })
        .await
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("listing the destination: {e}")))?;

        let completed: Vec<&Snapshot> = recorded
            .iter()
            .filter_map(|r| match r {
                Recorded::Completed { snapshot } => Some(snapshot),
                _ => None,
            })
            .collect();
        let verifications: Vec<&Verification> = recorded
            .iter()
            .filter_map(|r| match r {
                Recorded::Verified { verification } => Some(verification),
                _ => None,
            })
            .collect();
        let kept = kept_by(&found, &config);
        let snapshots: Vec<SnapshotRow> = found
            .iter()
            .zip(kept)
            .map(|(f, kept_by)| SnapshotRow {
                name: f.name.clone(),
                at: f.at,
                size_bytes: f.size_bytes,
                files: completed.iter().find(|s| s.name == f.name).map(|s| s.files),
                // Newest first, so the first one found is the latest word.
                verified: verifications.iter().find(|v| v.snapshot == f.name).map(|v| v.summary()),
                kept_by,
                encrypted: false,
            })
            .collect();

        let newest = snapshots.first().map(|s| s.at);
        let (due_by, overdue_by) = match newest {
            Some(at) => deadlines(&config, at),
            None => (None, None),
        };
        let age = age_level(now, newest, due_by, overdue_by);
        // Only verifications of archives still in the destination count: a
        // pass on a snapshot since deleted proves nothing about what is left.
        let last_verified: Option<VerifySummary> = verifications
            .iter()
            .find(|v| found.iter().any(|f| f.name == v.snapshot))
            .map(|v| v.summary());
        let last_failure: Option<BackupFailure> = recorded.iter().find_map(|r| match r {
            Recorded::Failed { failure } => Some(failure.clone()),
            _ => None,
        });
        let next_run = match &config.schedule {
            Some(schedule) => {
                let base = self.last_attempt(&factory, &config).await.unwrap_or(self.booted_at);
                crate::schedule::next_after(&schedule.as_task_schedule(), base)
                    .ok()
                    .map(|next| next.max(now))
            }
            None => None,
        };
        let warning_facts = WarningFacts {
            configured: true,
            destination: Some(facts.path.clone()),
            destination_exists: facts.exists,
            same_device: facts.same_device,
            scheduled: config.schedule.is_some(),
            newest,
            age: Some(age),
            last_verified: last_verified.as_ref().map(|v| (v.at, v.ok)),
            failure_since_newest: last_failure
                .as_ref()
                .filter(|f| newest.is_none_or(|n| f.at > n))
                .map(|f| (f.at, f.reason.clone())),
            code: code.clone(),
            time_machine: Some(time_machine.clone()),
        };
        // The include table's numbers are the newest snapshot this daemon
        // took itself: the one it has a manifest summary for.
        let newest_taken = snapshots.first().and_then(|row| completed.iter().find(|s| s.name == row.name).copied());
        Ok(BackupReport {
            now,
            include: include_rows(config.include_logs, newest_taken),
            exclude: exclude_rows(config.include_logs),
            config: Some(config),
            destination: Some(facts),
            age,
            due_by,
            next_run,
            running,
            last_verified,
            last_failure,
            warnings: warnings(&warning_facts),
            snapshots,
            code,
            time_machine: Some(time_machine),
        })
    }

    /// Take a backup now and apply retention after it. Refused while another
    /// backup operation holds the lock; every failure past that point
    /// is recorded and published as `backup_failed` before it is returned.
    pub(crate) async fn backup_run(self: &Arc<Self>, trigger: BackupTrigger, by: String) -> Result<Snapshot> {
        let (factory, config) = self.backup_config()?;
        let Ok(_busy) = self.backup_busy.try_lock() else {
            return Err(FactoryError::BadRequest(
                "another backup operation is already running; try again when it has finished".into(),
            ));
        };
        let at = Utc::now();
        match self.take_backup(&factory, &config, trigger, by, at).await {
            Ok(snapshot) => {
                self.backups.append(Recorded::Completed { snapshot: snapshot.clone() }).await?;
                tracing::info!(
                    snapshot = %snapshot.name,
                    size = snapshot.size_bytes,
                    files = snapshot.files,
                    pruned = snapshot.pruned.len(),
                    trigger = trigger.as_str(),
                    "backup completed"
                );
                self.bus.publish(Event::BackupCompleted { snapshot: snapshot.clone() });
                Ok(snapshot)
            }
            Err(e) => {
                let failure = BackupFailure { at, trigger, reason: e.to_string() };
                tracing::warn!(trigger = trigger.as_str(), "backup failed: {}", failure.reason);
                if let Err(store) = self.backups.append(Recorded::Failed { failure: failure.clone() }).await {
                    tracing::warn!("could not record the failed backup: {store}");
                }
                self.bus.publish(Event::BackupFailed { at, trigger, reason: failure.reason });
                Err(e)
            }
        }
    }

    async fn take_backup(
        &self,
        factory: &Factory,
        config: &BackupConfig,
        trigger: BackupTrigger,
        by: String,
        at: DateTime<Utc>,
    ) -> Result<Snapshot> {
        let plan = archive::Plan {
            root: factory.root.clone(),
            database: factory.database_path(),
            instance: ManifestInstance {
                id: factory.config.instance.id.clone(),
                name: factory.config.instance.name.clone(),
            },
            scope_configs: factory
                .config
                .scopes
                .iter()
                .map(|s| {
                    let dir = s.path.to_string_lossy().replace('\\', "/");
                    if dir.is_empty() || dir == "." {
                        format!("{FACTORY_DIR}/{CONFIG_FILE}")
                    } else {
                        format!("{}/{FACTORY_DIR}/{CONFIG_FILE}", dir.trim_end_matches('/'))
                    }
                })
                .collect(),
            include_logs: config.include_logs,
        };
        let name = factory_core::backup::archive_name(&factory.config.instance.name, at);
        let destination = config.destination.clone();
        let config_destination = config.destination.clone();
        let instance = factory.config.instance.name.clone();
        let config = config.clone();
        let started = std::time::Instant::now();
        let (taken, pruned) = tokio::task::spawn_blocking(move || -> Result<(archive::Taken, Vec<String>)> {
            let destination = archive::prepare_destination(&plan.root, &destination)?;
            let taken = archive::take(&plan, &destination, &name, at)?;
            // Retention only ever runs after a backup that succeeded, so a
            // run of failures never eats into the snapshots there are.
            let found = list_archives(&destination, &instance);
            let mut pruned = Vec::new();
            for (f, rules) in found.iter().zip(kept_by(&found, &config)) {
                if rules.is_empty() {
                    match std::fs::remove_file(destination.join(&f.name)) {
                        Ok(()) => pruned.push(f.name.clone()),
                        Err(e) => tracing::warn!(snapshot = %f.name, "retention could not delete it: {e}"),
                    }
                }
            }
            Ok((taken, pruned))
        })
        .await
        .map_err(|e| FactoryError::adapter("backup", format!("the backup task stopped: {e}")))??;
        Ok(Snapshot {
            name: taken.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            // As configured, not canonicalized: the path a person wrote is the
            // one they will look for.
            path: config_destination.join(taken.path.file_name().unwrap_or_default()).display().to_string(),
            at,
            trigger,
            by,
            size_bytes: taken.size_bytes,
            files: taken.files,
            database_bytes: taken.database_bytes,
            duration_ms: started.elapsed().as_millis() as u64,
            pruned,
            groups: taken.groups,
        })
    }

    /// Verify `snapshot`, or the newest. The result is recorded and
    /// published whether it passed or not; only a snapshot that cannot be
    /// found, or a lock already held, is refused without a record.
    pub(crate) async fn backup_verify(self: &Arc<Self>, snapshot: Option<String>, by: String) -> Result<Verification> {
        let (factory, config) = self.backup_config()?;
        if let Some(name) = &snapshot {
            refuse_bad_snapshot_name(name)?;
        }
        let Ok(_busy) = self.backup_busy.try_lock() else {
            return Err(FactoryError::BadRequest(
                "another backup operation is already running; try again when it has finished".into(),
            ));
        };
        let destination = config.destination.clone();
        let instance = factory.config.instance.name.clone();
        let found = tokio::task::spawn_blocking({
            let destination = destination.clone();
            move || list_archives(&destination, &instance)
        })
        .await
        .unwrap_or_default();
        let name = match snapshot {
            Some(name) => found.iter().find(|f| f.name == name).map(|f| f.name.clone()).ok_or_else(|| {
                FactoryError::BadRequest(format!(
                    "no snapshot {name:?} in {}; `factory backup list` shows them",
                    destination.display()
                ))
            })?,
            None => found.first().map(|f| f.name.clone()).ok_or_else(|| {
                FactoryError::BadRequest(format!("there is no snapshot in {} to verify", destination.display()))
            })?,
        };

        let at = Utc::now();
        let started = std::time::Instant::now();
        let path: PathBuf = destination.join(&name);
        let instance_id = factory.config.instance.id.clone();
        let checks = tokio::task::spawn_blocking(move || archive::verify(&path, &instance_id))
            .await
            .unwrap_or_else(|e| {
                vec![factory_core::backup::VerifyCheck {
                    name: "archive".into(),
                    status: CheckStatus::Fail,
                    detail: format!("the verification task stopped: {e}"),
                }]
            });
        let verification = Verification {
            ok: !checks.iter().any(|c| c.status == CheckStatus::Fail),
            snapshot: name,
            at,
            by,
            checks,
            duration_ms: started.elapsed().as_millis() as u64,
        };
        self.backups.append(Recorded::Verified { verification: verification.clone() }).await?;
        tracing::info!(snapshot = %verification.snapshot, ok = verification.ok, "backup verified");
        self.bus.publish(Event::BackupVerified { verification: verification.summary() });
        Ok(verification)
    }

    /// Restore one named snapshot into a new root. Authorization makes this
    /// owner-only before it reaches here; the daemon supplies its active root
    /// so the archive layer can refuse aliases of the live instance. Unlike a
    /// backup or verify, restore never changes this instance's history.
    pub(crate) async fn backup_restore(self: &Arc<Self>, snapshot: String, into: PathBuf) -> Result<Restoration> {
        let (factory, config) = self.backup_config()?;
        refuse_bad_snapshot_name(&snapshot)?;
        let Ok(_busy) = self.backup_busy.try_lock() else {
            return Err(FactoryError::BadRequest(
                "another backup operation is already running; try again when it has finished".into(),
            ));
        };
        let destination = config.destination.clone();
        let instance = factory.config.instance.name.clone();
        let found = tokio::task::spawn_blocking({
            let destination = destination.clone();
            move || list_archives(&destination, &instance)
        })
        .await
        .unwrap_or_default();
        if !found.iter().any(|f| f.name == snapshot) {
            return Err(FactoryError::BadRequest(format!(
                "no snapshot {snapshot:?} in {}; `factory backup list` shows them",
                destination.display()
            )));
        }

        let started = std::time::Instant::now();
        let archive_path = destination.join(&snapshot);
        let instance_id = factory.config.instance.id.clone();
        let active_root = factory.root.clone();
        let restored = tokio::task::spawn_blocking(move || {
            archive::restore(&archive_path, &instance_id, &active_root, &into)
        })
        .await
        .map_err(|e| FactoryError::adapter("backup", format!("the restore task stopped: {e}")))??;
        Ok(Restoration {
            snapshot,
            into: restored.into.display().to_string(),
            files: restored.files,
            checks: restored.checks,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }
}

/// The include table: the database, the configs, every authored directory,
/// and the two optional ones -- with numbers from the newest snapshot this
/// daemon took, when there is one.
fn include_rows(include_logs: bool, newest: Option<&Snapshot>) -> Vec<IncludeRow> {
    let totals = |group: Group| newest.and_then(|s| s.groups.iter().find(|g| g.group == group)).map(|g| (g.files, g.bytes));
    let numbers = |group: Group| match (newest, totals(group)) {
        (Some(_), Some((files, bytes))) => (Some(files), Some(bytes)),
        // Taken, and there was nothing there: zero, not unknown.
        (Some(_), None) => (Some(0), Some(0)),
        (None, _) => (None, None),
    };
    let mut rows = Vec::new();
    let mut push = |path: &str, why: &str, group: Group, included: bool| {
        let (files, bytes) = if included { numbers(group) } else { (None, None) };
        rows.push(IncludeRow { path: path.into(), why: why.into(), included, files, bytes });
    };
    push(
        ".factory/factory.sqlite",
        "tasks, runs, the journal, the workflow and bench stores, attestations, goal check-ins -- copied with VACUUM INTO",
        Group::Database,
        true,
    );
    push(
        ".factory/config.yaml and every scope's",
        "the instance's shape: its scopes, agents, roles and this block",
        Group::Config,
        true,
    );
    for (group, path, why) in AUTHORED {
        push(path, why, group, true);
    }
    for (group, path, why) in OPTIONAL {
        push(path, why, group, include_logs);
    }
    rows
}

fn exclude_rows(include_logs: bool) -> Vec<ExcludeRow> {
    let mut rows: Vec<ExcludeRow> =
        EXCLUDED.iter().map(|(path, why)| ExcludeRow { path: (*path).into(), why: (*why).into() }).collect();
    if !include_logs {
        for (_, path, _) in OPTIONAL {
            rows.push(ExcludeRow { path: path.into(), why: "optional: infrastructure.backup.include_logs is off".into() });
        }
    }
    rows
}

/// The daemon job: once a minute, take a backup if the schedule says one is
/// due. Due is counted from the later of the newest attempt and the newest
/// archive, so a daemon that was down at 03:00 takes the night's backup as
/// soon as it is back, exactly once -- the same "survives a missed night"
/// a scheduled task gets. With no schedule configured this does nothing.
pub async fn run(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(JOB_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() { return; }
            }
        }
        let factory = engine.factory_snapshot();
        let Some(config) = factory.config.infrastructure.backup.clone() else { continue };
        let Some(schedule) = config.schedule.clone() else { continue };
        let base = engine.last_attempt(&factory, &config).await.unwrap_or(engine.booted_at);
        let due = match crate::schedule::next_after(&schedule.as_task_schedule(), base) {
            Ok(due) => due,
            Err(e) => {
                tracing::warn!("infrastructure.backup.schedule: {e}");
                continue;
            }
        };
        if due > Utc::now() {
            continue;
        }
        // Busy (a person's backup operation) is not a failure: the
        // next tick looks again.
        if engine.backup_busy.try_lock().is_err() {
            continue;
        }
        // Its failures are recorded, published and logged inside; nothing
        // here can take the daemon down.
        let _ = engine.backup_run(BackupTrigger::Schedule, "schedule".into()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn config(yaml: &str) -> BackupConfig {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    /// A throwaway instance with a real database file, its root scope's own
    /// config and a knowledge page, backing up to a sibling directory.
    fn engine_backing_up(keep: &str, destination: &str) -> (Arc<Engine>, PathBuf) {
        use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration};
        use factory_plugins::{Registry, SqliteStore};
        let base = std::env::temp_dir().join(format!("factory-backup-engine-{}", uuid::Uuid::new_v4()));
        let root = base.join("instance");
        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::write(root.join(".factory/config.yaml"), "version: 1\ninstance:\n  id: test\n  name: test\n").unwrap();
        std::fs::write(root.join(".factory/knowledge/page.md"), "# A page\n").unwrap();
        let database = root.join(".factory/factory.sqlite");
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::open(&database).unwrap());
        let mut company: factory_core::config::Scope =
            serde_yaml_ng::from_str("id: company-id\nname: company\n").unwrap();
        company.path = PathBuf::from(".");
        let destination = base.join(destination);
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(company.clone()),
            scopes: vec![company],
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration::default(),
            quality: Default::default(),
            infrastructure: serde_yaml_ng::from_str(&format!(
                "backup:\n  destination: {}\n  keep: {keep}\n",
                destination.display()
            ))
            .unwrap(),
            plugins_dir: None,
        };
        let factory = Factory { root, config };
        let engine = Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new())
            .with_backup_store(BackupStore::open(&database).unwrap());
        (Arc::new(engine), base)
    }

    /// `#155`, end to end through the engine: a scope's remote can carry a
    /// token, and it must never survive onto the wire `GET /api/backup`
    /// answers with -- only the redacted URL, and only for the tracked row.
    #[tokio::test]
    async fn a_secret_in_a_scopes_remote_never_reaches_the_serialized_report() {
        async fn run(dir: &Path, args: &[&str]) {
            assert!(
                tokio::process::Command::new("git").args(args).current_dir(dir).status().await.unwrap().success(),
                "git {args:?} in {}",
                dir.display()
            );
        }
        async fn output(dir: &Path, args: &[&str]) -> String {
            let out = tokio::process::Command::new("git").args(args).current_dir(dir).output().await.unwrap();
            assert!(out.status.success(), "git {args:?} in {}", dir.display());
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        let (engine, base) = engine_backing_up("{ daily: 7 }", "destination");
        let root = engine.factory_snapshot().root;
        run(&root, &["init", "-q"]).await;
        run(&root, &["config", "user.email", "factory@example.com"]).await;
        run(&root, &["config", "user.name", "factory"]).await;
        run(&root, &["add", "-A"]).await;
        run(&root, &["commit", "-q", "-m", "base"]).await;
        run(&root, &["remote", "add", "origin", "https://x-access-token:SECRET-TOKEN@github.com/o/r.git"]).await;
        // Plant the remote-tracking ref at the current commit and set the
        // upstream directly -- never fetching -- then commit once more so
        // `ahead == 1` without this probe ever contacting the remote.
        let head_sha = output(&root, &["rev-parse", "HEAD"]).await;
        let branch = output(&root, &["symbolic-ref", "--short", "HEAD"]).await;
        let tracking_ref = format!("refs/remotes/origin/{branch}");
        run(&root, &["update-ref", tracking_ref.as_str(), head_sha.as_str()]).await;
        let upstream = format!("origin/{branch}");
        run(&root, &["branch", "--set-upstream-to", upstream.as_str()]).await;
        run(&root, &["commit", "-q", "--allow-empty", "-m", "second"]).await;

        let report = engine.backup_report().await.unwrap();
        let repo = report
            .code
            .iter()
            .find(|f| f.scopes.iter().any(|s| s == "company"))
            .expect("the company scope's repository fact");
        match &repo.state {
            factory_core::backup::RepositoryState::Tracked { ahead, .. } => assert_eq!(*ahead, 1, "{repo:?}"),
            other => panic!("expected Tracked, got {other:?}"),
        }
        assert_eq!(repo.remote_url.as_deref(), Some("https://github.com/o/r.git"));

        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("SECRET-TOKEN"), "a secret in a remote reached the wire: {json}");
        assert!(json.contains("https://github.com/o/r.git"), "the redacted url should still be on the wire: {json}");
        std::fs::remove_dir_all(base).ok();
    }

    /// The whole v1 path through the engine: run, list, verify, and the
    /// events a watcher sees -- then retention after a second backup.
    #[tokio::test]
    async fn a_backup_is_taken_listed_verified_and_pruned_through_the_engine() {
        let (engine, base) = engine_backing_up("{ daily: 1, weekly: 0, monthly: 0 }", "destination");
        let mut events = engine.bus.subscribe();

        let before = engine.backup_report().await.unwrap();
        assert_eq!(before.age, AgeLevel::None);
        assert!(before.warnings.iter().any(|w| w.kind == "no_backup"));
        assert!(before.warnings.iter().any(|w| w.kind == "destination_missing"));

        let first = engine.backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        assert_eq!(first.files, 3, "the database, the root config and one page");
        assert!(matches!(events.recv().await.unwrap(), Event::BackupCompleted { .. }));

        let report = engine.backup_report().await.unwrap();
        assert_eq!(report.age, AgeLevel::Fresh);
        assert_eq!(report.snapshots.len(), 1);
        assert_eq!(report.snapshots[0].files, Some(3));
        assert_eq!(report.snapshots[0].kept_by, [KeptBy::Newest, KeptBy::Daily]);
        let kinds: Vec<&str> = report.warnings.iter().map(|w| w.kind.as_str()).collect();
        assert!(kinds.contains(&"same_device"), "a temp dir beside the instance is the same disk: {kinds:?}");
        assert!(kinds.contains(&"never_verified"), "{kinds:?}");
        let knowledge = report.include.iter().find(|r| r.path == ".factory/knowledge/").unwrap();
        assert_eq!(knowledge.files, Some(1));

        let verification = engine.backup_verify(None, "owner".into()).await.unwrap();
        assert!(verification.ok, "{:?}", verification.checks);
        assert_eq!(verification.snapshot, first.name);
        assert!(matches!(events.recv().await.unwrap(), Event::BackupVerified { .. }));
        let report = engine.backup_report().await.unwrap();
        assert_eq!(report.last_verified.as_ref().map(|v| v.ok), Some(true));
        assert!(!report.warnings.iter().any(|w| w.kind == "never_verified"));

        // A second backup a second later, the same day: `daily: 1` keeps
        // only the newest, so retention deletes the first.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let second = engine.backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        assert_eq!(second.pruned, [first.name.clone()]);
        let report = engine.backup_report().await.unwrap();
        assert_eq!(report.snapshots.len(), 1);
        assert_eq!(report.last_verified, None, "the verified snapshot is gone, so nothing verified is left");

        let e = engine.backup_verify(Some("../etc.tar.zst".into()), "owner".into()).await.unwrap_err();
        assert!(e.to_string().contains("not a snapshot name"), "{e}");
        std::fs::remove_dir_all(base).ok();
    }

    /// A destination that cannot be written is a recorded, published
    /// failure -- and the next report says so -- never a crash.
    #[tokio::test]
    async fn a_failed_backup_is_recorded_published_and_warned_about() {
        let (engine, base) = engine_backing_up("{ daily: 7 }", "unmounted/volume");
        let mut events = engine.bus.subscribe();
        let e = engine.backup_run(BackupTrigger::Schedule, "schedule".into()).await.unwrap_err();
        assert!(e.to_string().contains("mounted"), "{e}");
        assert!(matches!(events.recv().await.unwrap(), Event::BackupFailed { .. }));
        let report = engine.backup_report().await.unwrap();
        assert_eq!(report.last_failure.as_ref().map(|f| f.trigger), Some(BackupTrigger::Schedule));
        assert!(report.warnings.iter().any(|w| w.kind == "last_failed"));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_nightly_backup_is_fresh_until_the_next_night_plus_grace() {
        let c = config("destination: /b\nschedule: { cron: \"0 3 * * *\" }\n");
        let newest = Utc.with_ymd_and_hms(2026, 9, 25, 3, 0, 5).unwrap();
        let (due, overdue) = deadlines(&c, newest);
        assert_eq!(due, Some(Utc.with_ymd_and_hms(2026, 9, 26, 5, 0, 0).unwrap()));
        assert_eq!(overdue, Some(Utc.with_ymd_and_hms(2026, 9, 27, 5, 0, 0).unwrap()));
    }

    #[test]
    fn without_a_schedule_the_yardsticks_are_a_day_and_a_week() {
        let c = config("destination: /b\n");
        let newest = Utc.with_ymd_and_hms(2026, 9, 25, 3, 0, 0).unwrap();
        let (due, overdue) = deadlines(&c, newest);
        assert_eq!(due, Some(newest + Duration::hours(26)));
        assert_eq!(overdue, Some(newest + Duration::days(7)));
    }

    #[test]
    fn listing_ignores_everything_that_is_not_this_instances_archive() {
        let dir = std::env::temp_dir().join(format!("factory-backup-list-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "factory-backup-demo-20260925T030000Z.tar.zst",
            "factory-backup-demo-20260924T030000Z.tar.zst",
            ".factory-backup-demo-20260926T030000Z.tar.zst.partial",
            "factory-backup-other-20260925T030000Z.tar.zst",
            "holiday.jpg",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let found = list_archives(&dir, "demo");
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["factory-backup-demo-20260925T030000Z.tar.zst", "factory-backup-demo-20260924T030000Z.tar.zst"]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_include_table_reads_zero_for_an_empty_directory_and_unknown_before_any_backup() {
        let before = include_rows(false, None);
        assert!(before.iter().all(|r| r.files.is_none()));
        assert!(!before.iter().find(|r| r.path == ".factory/logs/").unwrap().included);
        let snapshot = Snapshot {
            name: "s".into(),
            path: "/b/s".into(),
            at: Utc::now(),
            trigger: BackupTrigger::Manual,
            by: "owner".into(),
            size_bytes: 1,
            files: 2,
            database_bytes: 1,
            duration_ms: 1,
            pruned: vec![],
            groups: vec![factory_core::backup::GroupTotal { group: Group::Knowledge, files: 3, bytes: 30 }],
        };
        let after = include_rows(false, Some(&snapshot));
        let knowledge = after.iter().find(|r| r.path == ".factory/knowledge/").unwrap();
        assert_eq!((knowledge.files, knowledge.bytes), (Some(3), Some(30)));
        let goals = after.iter().find(|r| r.path == ".factory/goals/").unwrap();
        assert_eq!(goals.files, Some(0));
        let vex = after.iter().find(|r| r.path == ".factory/vex/").unwrap();
        assert!(vex.included);
        assert_eq!(vex.files, Some(0));
        assert!(exclude_rows(false).iter().any(|r| r.path == ".factory/logs/"));
        assert!(!exclude_rows(true).iter().any(|r| r.path == ".factory/logs/"));
    }
}
