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

use chrono::{DateTime, Utc};
#[cfg(test)]
use chrono::Duration;
use factory_core::backup::{
    age_level, refuse_bad_snapshot_name, retain, warnings,
    AgeLevel, BackupConfig, BackupFailure, BackupReport, BackupTrigger, CheckStatus,
    ExcludeRow, Group, IncludeRow, KeptBy, ManifestInstance, RepositoryFact, Restoration, ScopeIncludeRow, Snapshot,
    SnapshotRow, TimeMachineFact, Verification, VerifyCheck, WarningFacts, AUTHORED,
    EXCLUDED, OPTIONAL,
};
use factory_core::config::{Factory, CONFIG_FILE, FACTORY_DIR};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use std::path::PathBuf;
#[cfg(test)]
use std::path::Path;
use std::sync::Arc;

use crate::engine::Engine;
use crate::l1_service::L1Service;
use store::Recorded;

/// How often the job looks at the clock. A backup is due at most once a
/// night, so a minute's lateness is nothing, and a look is cheap.
const JOB_TICK: std::time::Duration = std::time::Duration::from_secs(60);

pub(crate) use factory_infrastructure::backup_facts::{
    resolve_scope_includes, Captured, Found, completed_of, deadlines,
    last_verified_of_present, list_archives, verifications_of,
};

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


/// Refuse at start a schedule the job could never fire and an `encrypt_to`
/// that is not a usable recipient, the same way an unregistered knowledge
/// provider is refused there -- in front of whoever started the daemon, not
/// as a `backup_failed` every minute (or every backup, `#152`) afterwards.
/// `archive.rs`'s own verification calls this too, on a restored config.
pub fn validate_config(factory: &Factory) -> Result<()> {
    let Some(backup) = factory.config.infrastructure.backup.as_ref() else { return Ok(()) };
    if let Some(schedule) = &backup.schedule {
        crate::schedule::next_after(&schedule.as_task_schedule(), Utc::now())
            .map_err(|e| FactoryError::BadRequest(format!("infrastructure.backup.schedule: {e}")))?;
    }
    if let Some(schedule) = &backup.verify_schedule {
        crate::schedule::next_after(&schedule.as_task_schedule(), Utc::now())
            .map_err(|e| FactoryError::BadRequest(format!("infrastructure.backup.verify_schedule: {e}")))?;
    }
    if let Some(encrypt_to) = &backup.encrypt_to {
        parse_recipient(encrypt_to)?;
    }
    Ok(())
}

/// The one place `age::x25519::Recipient` is parsed from config (`#152`): a
/// single native X25519 recipient only. An SSH public key or a plugin
/// recipient (`age1yubikey1…`) fails the very same bech32 HRP check the
/// `age` crate applies to a native one, so refusing them needs no
/// special-casing here -- only a clearer message than the crate's own.
pub fn parse_recipient(encrypt_to: &str) -> Result<age::x25519::Recipient> {
    encrypt_to.parse::<age::x25519::Recipient>().map_err(|e| {
        FactoryError::BadRequest(format!(
            "infrastructure.backup.encrypt_to {encrypt_to:?} is not usable: {e} -- only a single native X25519 \
             recipient (age1…) is supported; SSH and plugin recipients (age1yubikey1…, ssh-ed25519 …) are refused"
        ))
    })
}

impl L1Service<'_> {
    fn backup_config(&self) -> Result<(Factory, BackupConfig)> {
        let factory = self.wiring.snapshot();
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
        let recorded = self.state.backups.all().await.unwrap_or_default();
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

    /// `#156`: the newest verification recorded of any trigger -- a manual
    /// verify satisfies a drill's own slot exactly as a manual backup
    /// satisfies `last_attempt`'s. `None` before anything has ever been
    /// verified, so the caller falls back to `booted_at`.
    async fn last_verified_at(&self) -> Option<DateTime<Utc>> {
        let recorded = self.state.backups.all().await.unwrap_or_default();
        recorded
            .iter()
            .filter_map(|r| match r {
                Recorded::Verified { verification } => Some(verification.at),
                _ => None,
            })
            .max()
    }

    /// `GET /api/backup`. Never fails for a destination that is missing or
    /// unreadable: that is a fact the report carries, and a warning.
    pub(crate) async fn backup_report(&self) -> Result<BackupReport> {
        let now = Utc::now();
        let factory = self.wiring.snapshot();
        // `#155`: gathered whether or not a backup is even configured --
        // source code is backed up by pushing it, not by this snapshot --
        // so both branches `report` handles carry them. Deliberately not
        // part of `capture`: `backup_fact` (`#154`) must never spawn `git`
        // or `tmutil`.
        let (code, time_machine) = tokio::join!(
            repos::repository_facts(&factory.root, &factory.config.scopes),
            repos::time_machine_fact(),
        );
        let state = self.capture(now).await?;
        let scope_includes = self.scope_include_rows(&factory).await;
        Ok(report(&state, code, time_machine, scope_includes))
    }

    /// `#279`: every scope's own declared `backup.include`, resolved and
    /// probed fresh off-thread -- filled whether or not a backup is even
    /// configured, the same as `code` and `time_machine` above, so a
    /// refused or missing declaration is visible before there is anywhere
    /// to put a snapshot at all. `report` joins this against the newest
    /// snapshot's own cached numbers; this never reopens an archive.
    async fn scope_include_rows(&self, factory: &Factory) -> Vec<ScopeIncludeRow> {
        let declarations = factory.config.backup_includes();
        if declarations.is_empty() {
            return Vec::new();
        }
        let root = factory.root.clone();
        tokio::task::spawn_blocking(move || resolve_scope_includes(&root, &declarations))
            .await
            .unwrap_or_default()
    }

    /// The one gather behind both `backup_report` and `backup_fact`: the
    /// live config, whether an operation is already running, every
    /// recorded `backup_events` row, and -- only when a backup is
    /// configured -- the destination's own facts and its archive listing,
    /// off-thread as today. `now` is a parameter so a metric's `as_of` and
    /// a report's own clock are always the same instant this state was
    /// gathered at.
    async fn capture(&self, now: DateTime<Utc>) -> Result<Captured> {
        self.backup_provider().capture(now).await
    }

    /// Take a backup now and apply retention after it. Refused while another
    /// backup operation holds the lock; every failure past that point
    /// is recorded and published as `backup_failed` before it is returned.
    pub(crate) async fn backup_run(&self, trigger: BackupTrigger, by: String) -> Result<Snapshot> {
        let (factory, config) = self.backup_config()?;
        let Ok(_busy) = self.state.backup_busy.try_lock() else {
            return Err(FactoryError::BadRequest(
                "another backup operation is already running; try again when it has finished".into(),
            ));
        };
        let at = Utc::now();
        match self.take_backup(&factory, &config, trigger, by, at).await {
            Ok(snapshot) => {
                self.state.backups.append(Recorded::Completed { snapshot: snapshot.clone() }).await?;
                tracing::info!(
                    snapshot = %snapshot.name,
                    size = snapshot.size_bytes,
                    files = snapshot.files,
                    pruned = snapshot.pruned.len(),
                    trigger = trigger.as_str(),
                    "backup completed"
                );
                self.wiring.bus().publish(Event::BackupCompleted { snapshot: snapshot.clone() });
                Ok(snapshot)
            }
            Err(e) => {
                let failure = BackupFailure { at, trigger, reason: e.to_string() };
                tracing::warn!(trigger = trigger.as_str(), "backup failed: {}", failure.reason);
                if let Err(store) = self.state.backups.append(Recorded::Failed { failure: failure.clone() }).await {
                    tracing::warn!("could not record the failed backup: {store}");
                }
                self.wiring.bus().publish(Event::BackupFailed { at, trigger, reason: failure.reason });
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
            encrypt_to: config.encrypt_to.clone(),
            scope_includes: factory.config.backup_includes(),
        };
        let name = factory_core::backup::archive_name(&factory.config.instance.name, at, config.encrypt_to.is_some());
        let destination = config.destination.clone();
        let config_destination = config.destination.clone();
        let instance = factory.config.instance.name.clone();
        let encrypted_to = config.encrypt_to.clone();
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
            encrypted_to,
            scope_includes: taken.scope_includes,
        })
    }

    /// Verify `snapshot`, or the newest. The result is recorded and
    /// published whether it passed or not; only a snapshot that cannot be
    /// found, a lock already held, or -- for an encrypted snapshot -- a
    /// missing or unusable `identity` (`#152`), is refused without a record.
    /// A `BadRequest` here is never a verdict about the archive: it is
    /// refused before `archive::verify` runs at all.
    pub(crate) async fn backup_verify(
        &self,
        snapshot: Option<String>,
        identity: Option<PathBuf>,
        by: String,
    ) -> Result<Verification> {
        self.backup_verify_at(snapshot, identity, by, Utc::now()).await
    }

    /// `backup_verify`, with the verification's own `at` supplied rather
    /// than read off the clock. `#156`'s job loop passes its own tick's
    /// `now` here, so a scheduled drill's `Recorded::Verified.at` -- and so
    /// the next slot `next_after` computes from -- is exactly the clock a
    /// test drives, never a race against real time. Every other caller goes
    /// through `backup_verify` above, which is just this with `Utc::now()`.
    async fn backup_verify_at(
        &self,
        snapshot: Option<String>,
        identity: Option<PathBuf>,
        by: String,
        at: DateTime<Utc>,
    ) -> Result<Verification> {
        let (factory, config) = self.backup_config()?;
        if let Some(name) = &snapshot {
            refuse_bad_snapshot_name(name)?;
        }
        let Ok(_busy) = self.state.backup_busy.try_lock() else {
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
        let target = match snapshot {
            Some(name) => found.iter().find(|f| f.name == name).cloned().ok_or_else(|| {
                FactoryError::BadRequest(format!(
                    "no snapshot {name:?} in {}; `factory backup list` shows them",
                    destination.display()
                ))
            })?,
            None => found.first().cloned().ok_or_else(|| {
                FactoryError::BadRequest(format!("there is no snapshot in {} to verify", destination.display()))
            })?,
        };

        let started = std::time::Instant::now();
        let path: PathBuf = destination.join(&target.name);
        let instance_id = factory.config.instance.id.clone();
        let root = factory.root.clone();
        let encrypted = target.encrypted;
        let name = target.name.clone();
        let checks: Vec<VerifyCheck> = tokio::task::spawn_blocking(move || -> Result<Vec<VerifyCheck>> {
            let owner_identity = match (encrypted, identity) {
                (true, None) => {
                    return Err(FactoryError::BadRequest(format!(
                        "snapshot {name:?} is encrypted; run `factory backup verify {name} --identity <file>`"
                    )))
                }
                (true, Some(path)) => Some(archive::OwnerIdentity::read(&path, &root)?),
                (false, _) => None,
            };
            Ok(archive::verify(&path, &instance_id, owner_identity.as_ref()))
        })
        .await
        .map_err(|e| FactoryError::adapter("backup", format!("the verification task stopped: {e}")))??;
        let verification = Verification {
            ok: !checks.iter().any(|c| c.status == CheckStatus::Fail),
            snapshot: target.name,
            at,
            by,
            checks,
            duration_ms: started.elapsed().as_millis() as u64,
        };
        self.state.backups.append(Recorded::Verified { verification: verification.clone() }).await?;
        tracing::info!(snapshot = %verification.snapshot, ok = verification.ok, "backup verified");
        self.wiring.bus().publish(Event::BackupVerified { verification: verification.summary() });
        Ok(verification)
    }

    /// `#156`: run a verification drill if `verify_schedule`'s own next slot
    /// is due at `now`. Shares `backup_verify`'s `backup_busy` exclusion,
    /// `backup_verified` event and row -- busy or no snapshot leave nothing
    /// recorded, so the slot stays due and the next tick tries again. An
    /// encrypted newest snapshot is checked here, directly off the listing,
    /// rather than by matching `backup_verify_at`'s own refusal text: the
    /// drill never holds an identity, so it never even attempts one, and
    /// logs the skip once per slot rather than every tick. Never panics:
    /// any other error `backup_verify_at` returns (busy, or anything else)
    /// is logged and swallowed -- the job's whole promise.
    async fn maybe_verify(&self, factory: &Factory, config: &BackupConfig, now: DateTime<Utc>) {
        let Some(schedule) = &config.verify_schedule else {
            self.clear_verify_skip();
            return;
        };
        let base = self.last_verified_at().await.unwrap_or(self.wiring.booted_at());
        let due = match crate::schedule::next_after(&schedule.as_task_schedule(), base) {
            Ok(due) => due,
            Err(e) => {
                tracing::warn!("infrastructure.backup.verify_schedule: {e}");
                return;
            }
        };
        if due > now {
            self.clear_verify_skip();
            return;
        }

        let destination = config.destination.clone();
        let instance = factory.config.instance.name.clone();
        let found = tokio::task::spawn_blocking(move || list_archives(&destination, &instance)).await.unwrap_or_default();
        let Some(newest) = found.first() else {
            // No snapshot: nothing to verify, nothing recorded. The slot
            // stays due, so the very next tick with a snapshot drills it.
            self.clear_verify_skip();
            return;
        };
        if newest.encrypted {
            self.log_verify_skip_once(due, "newest snapshot is encrypted; verify it with --identity");
            return;
        }
        self.clear_verify_skip();
        if let Err(e) = self.backup_verify_at(None, None, "schedule".into(), now).await {
            tracing::warn!("verification drill: {e}");
        }
    }

    /// Log `reason` at most once for a given due `slot` -- otherwise an
    /// encrypted newest snapshot with no identity would log the same line
    /// on every tick for as long as it stays that way.
    fn log_verify_skip_once(&self, slot: DateTime<Utc>, reason: &str) {
        let mut state = self.state.verify_drill_skip.lock().unwrap_or_else(|p| p.into_inner());
        let unchanged = state.as_ref().is_some_and(|(s, r)| *s == slot && r == reason);
        if !unchanged {
            tracing::info!("verification drill skipped: {reason}");
        }
        *state = Some((slot, reason.to_string()));
    }

    fn clear_verify_skip(&self) {
        *self.state.verify_drill_skip.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// Restore one named snapshot into a new root. Authorization makes this
    /// owner-only before it reaches here; the daemon supplies its active root
    /// so the archive layer can refuse aliases of the live instance. Unlike a
    /// backup or verify, restore never changes this instance's history.
    /// `identity` decrypts an encrypted snapshot (`#152`) -- missing or
    /// unusable, restore is refused the same way verify is, before anything
    /// is staged.
    pub(crate) async fn backup_restore(
        &self,
        snapshot: String,
        into: PathBuf,
        identity: Option<PathBuf>,
    ) -> Result<Restoration> {
        let (factory, config) = self.backup_config()?;
        refuse_bad_snapshot_name(&snapshot)?;
        let Ok(_busy) = self.state.backup_busy.try_lock() else {
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
        let target = found.iter().find(|f| f.name == snapshot).cloned().ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "no snapshot {snapshot:?} in {}; `factory backup list` shows them",
                destination.display()
            ))
        })?;

        let started = std::time::Instant::now();
        let archive_path = destination.join(&target.name);
        let instance_id = factory.config.instance.id.clone();
        let active_root = factory.root.clone();
        let root = factory.root.clone();
        let encrypted = target.encrypted;
        let name = target.name.clone();
        let restored = tokio::task::spawn_blocking(move || -> Result<archive::Restored> {
            let owner_identity = match (encrypted, identity) {
                (true, None) => {
                    return Err(FactoryError::BadRequest(format!(
                        "snapshot {name:?} is encrypted; run `factory backup restore {name} --into <new-root> \
                         --identity <file>`"
                    )))
                }
                (true, Some(path)) => Some(archive::OwnerIdentity::read(&path, &root)?),
                (false, _) => None,
            };
            archive::restore(&archive_path, &instance_id, &active_root, &into, owner_identity.as_ref())
        })
        .await
        .map_err(|e| FactoryError::adapter("backup", format!("the restore task stopped: {e}")))??;
        Ok(Restoration {
            snapshot: target.name,
            into: restored.into.display().to_string(),
            files: restored.files,
            checks: restored.checks,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }
}

/// `GET /api/backup`'s whole page, from a `Captured` state plus `#155`'s
/// code and Time Machine facts (gathered separately, never part of
/// `capture`). Byte-identical to the pre-`#154` `backup_report` for every
/// existing case -- the split changed nothing about what the page shows.
/// `scope_includes` is `#279`'s live resolution (gathered separately too,
/// for the same reason): this only ever joins it against the newest
/// snapshot's own cached numbers, never reopening an archive.
fn report(
    state: &Captured,
    code: Vec<RepositoryFact>,
    time_machine: TimeMachineFact,
    scope_includes: Vec<ScopeIncludeRow>,
) -> BackupReport {
    let Some(config) = &state.config else {
        return BackupReport {
            now: state.now,
            config: None,
            destination: None,
            age: AgeLevel::None,
            due_by: None,
            next_run: None,
            next_verify: None,
            verify_skipped: None,
            running: state.running,
            last_verified: None,
            last_failure: None,
            warnings: warnings(&WarningFacts {
                code: code.clone(),
                time_machine: Some(time_machine.clone()),
                ..Default::default()
            }),
            snapshots: Vec::new(),
            include: include_rows(false, None),
            exclude: exclude_rows(false),
            code,
            time_machine: Some(time_machine),
            scope_includes: join_scope_includes(scope_includes, None),
        };
    };
    let destination = state
        .destination
        .clone()
        .expect("capture gathers destination facts whenever a backup is configured");

    let completed = completed_of(&state.recorded);
    let verifications = verifications_of(&state.recorded);
    let kept = kept_by(&state.found, config);
    let snapshots: Vec<SnapshotRow> = state
        .found
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
            encrypted: f.encrypted,
        })
        .collect();

    let newest = snapshots.first().map(|s| s.at);
    let newest_encrypted = snapshots.first().is_some_and(|s| s.encrypted);
    let (due_by, overdue_by) = match newest {
        Some(at) => deadlines(config, at),
        None => (None, None),
    };
    let age = age_level(state.now, newest, due_by, overdue_by);
    let last_verified = last_verified_of_present(&verifications, &state.found);
    let last_failure: Option<BackupFailure> = state.recorded.iter().find_map(|r| match r {
        Recorded::Failed { failure } => Some(failure.clone()),
        _ => None,
    });
    let warning_facts = WarningFacts {
        configured: true,
        destination: Some(destination.path.clone()),
        destination_exists: destination.exists,
        same_device: destination.same_device,
        scheduled: config.schedule.is_some(),
        newest,
        age: Some(age),
        last_verified: last_verified.as_ref().map(|v| (v.at, v.ok)),
        failure_since_newest: last_failure
            .as_ref()
            .filter(|f| newest.is_none_or(|n| f.at > n))
            .map(|f| (f.at, f.reason.clone())),
        newest_encrypted,
        code: code.clone(),
        time_machine: Some(time_machine.clone()),
    };
    // The include table's numbers are the newest snapshot this daemon took
    // itself: the one it has a manifest summary for.
    let newest_taken = snapshots.first().and_then(|row| completed.iter().find(|s| s.name == row.name).copied());
    // `#156`: a plain projection of the same `newest_encrypted` fact the
    // warnings strip already reads -- never the job loop's own
    // `verify_drill_skip` state, which only throttles how often it logs and
    // would otherwise make this field stale for up to a tick after the
    // config or the destination changes.
    let verify_skipped = (config.verify_schedule.is_some() && newest_encrypted)
        .then(|| "newest snapshot is encrypted; verify it with --identity".to_string());
    BackupReport {
        now: state.now,
        include: include_rows(config.include_logs, newest_taken),
        exclude: exclude_rows(config.include_logs),
        config: Some(config.clone()),
        destination: Some(destination),
        age,
        due_by,
        next_run: state.next_run,
        next_verify: state.next_verify,
        verify_skipped,
        running: state.running,
        last_verified,
        last_failure,
        warnings: warnings(&warning_facts),
        snapshots,
        code,
        time_machine: Some(time_machine),
        scope_includes: join_scope_includes(scope_includes, newest_taken),
    }
}

/// `#279`: fill in each live-resolved row's `files`/`bytes` from the
/// newest snapshot this daemon took, when there is one and it held this
/// exact declaration -- `None` for a declaration newer than that snapshot,
/// exactly as `include_rows` leaves a directory it has not written to yet.
fn join_scope_includes(mut live: Vec<ScopeIncludeRow>, newest: Option<&Snapshot>) -> Vec<ScopeIncludeRow> {
    let Some(newest) = newest else { return live };
    for row in &mut live {
        if let Some(cached) = newest.scope_includes.iter().find(|t| t.scope == row.scope && t.declared == row.declared) {
            row.files = Some(cached.files);
            row.bytes = Some(cached.bytes);
        }
    }
    live
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
/// due, then -- `#156` -- run a verification drill if `verify_schedule`'s
/// own schedule says one is. Each is counted from its own base: a backup
/// from the later of the newest attempt and the newest archive, a drill
/// from the newest verification recorded of any trigger; either falls back
/// to when the daemon booted if there is nothing yet. So a daemon that was
/// down across several slots catches up exactly once for each, the same
/// "survives a missed night" a scheduled task gets. When a backup is also
/// due, it runs first and the drill waits for a later tick, so it never
/// verifies a snapshot mid-write. With neither configured this does
/// nothing.
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
        tick(&engine, Utc::now()).await;
    }
}

/// One tick of [`run`]'s loop, `now` passed in rather than read off the
/// clock so a test can drive it directly instead of waiting on real time.
/// Never takes the daemon down: every failure below is logged and
/// swallowed, the same promise `backup_run`'s and `backup_verify`'s own
/// callers already keep.
async fn tick(engine: &Arc<Engine>, now: DateTime<Utc>) {
    let factory = engine.factory_snapshot();
    let Some(config) = factory.config.infrastructure.backup.clone() else { return };

    if let Some(schedule) = &config.schedule {
        let base = engine.l1_service().last_attempt(&factory, &config).await.unwrap_or(engine.shared.booted_at);
        match crate::schedule::next_after(&schedule.as_task_schedule(), base) {
            Ok(due) if due <= now => {
                // Busy (a person's backup operation) is not a failure: the
                // next tick looks again, and the drill waits for it too.
                if engine.l1.backup_busy.try_lock().is_err() {
                    return;
                }
                // Its failures are recorded, published and logged inside;
                // nothing here can take the daemon down.
                let _ = engine.l1_service().backup_run(BackupTrigger::Schedule, "schedule".into()).await;
                // The drill runs on a later tick, against the fresh
                // snapshot this one just took.
                return;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("infrastructure.backup.schedule: {e}"),
        }
    }

    engine.l1_service().maybe_verify(&factory, &config, now).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn config(yaml: &str) -> BackupConfig {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    /// `#152`: `parse_recipient` is the one place the `age` crate actually
    /// parses `encrypt_to` -- `factory_core::backup::BackupConfig::validate`
    /// stays dependency-free and accepts any non-empty string, so this is
    /// the only test proving an SSH or plugin recipient is truly refused,
    /// not just deferred to nowhere.
    #[test]
    fn parse_recipient_accepts_only_a_native_x25519_recipient() {
        let recipient = age::x25519::Identity::generate().to_public().to_string();
        parse_recipient(&recipient).unwrap();

        let ssh = parse_recipient("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBogusnotarealkeyatall").unwrap_err();
        assert!(ssh.to_string().contains("encrypt_to"), "{ssh}");

        // A plugin recipient's HRP is "age1<plugin-name>", never bare "age" --
        // the same bech32 check that accepts a native recipient refuses this
        // without any special-casing.
        let plugin = parse_recipient("age1yubikey1qtn67d3z5jnzq2crf0zgz2u9r5c9z9x8g3p3f9x7hqjxdq0h4z0").unwrap_err();
        assert!(plugin.to_string().contains("encrypt_to"), "{plugin}");

        let garbage = parse_recipient("age1not-a-real-recipient").unwrap_err();
        assert!(garbage.to_string().contains("encrypt_to"), "{garbage}");
    }

    /// The daemon-start refusal in practice: `main.rs` calls `validate_config`
    /// on the live config before the daemon does anything else, and it must
    /// reject a config asking to encrypt to something that is not a native
    /// recipient just as reliably as it already rejects an unfireable cron
    /// expression.
    #[tokio::test]
    async fn validate_config_refuses_a_bad_recipient_the_same_way_it_refuses_a_bad_schedule() {
        let (engine, base) = engine_backing_up("{ daily: 7 }", "destination");
        let mut factory = engine.factory_snapshot();
        factory.config.infrastructure.backup.as_mut().unwrap().encrypt_to =
            Some("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBogusnotarealkeyatall".into());
        let e = validate_config(&factory).unwrap_err();
        assert!(e.to_string().contains("encrypt_to"), "{e}");

        let (_, recipient) = generated_identity();
        factory.config.infrastructure.backup.as_mut().unwrap().encrypt_to = Some(recipient);
        validate_config(&factory).unwrap();

        std::fs::remove_dir_all(base).ok();
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
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let engine = Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new())
            .with_backup_store(BackupStore::open(&database).unwrap());
        (Arc::new(engine), base)
    }

    /// `engine_backing_up`, with the root scope also declaring
    /// `backup.include: [data/finance]` (`#279`) -- for the engine-level
    /// round trip through `backup_report`/`backup_run`, not just
    /// `archive.rs`'s own unit tests of `take` itself.
    fn engine_backing_up_with_scope_include(keep: &str, destination: &str) -> (Arc<Engine>, PathBuf) {
        use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration};
        use factory_plugins::{Registry, SqliteStore};
        let base = std::env::temp_dir().join(format!("factory-backup-engine-scope-include-{}", uuid::Uuid::new_v4()));
        let root = base.join("instance");
        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::write(root.join(".factory/config.yaml"), "version: 1\ninstance:\n  id: test\n  name: test\n").unwrap();
        std::fs::write(root.join(".factory/knowledge/page.md"), "# A page\n").unwrap();
        let database = root.join(".factory/factory.sqlite");
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::open(&database).unwrap());
        let mut company: factory_core::config::Scope =
            serde_yaml_ng::from_str("id: company-id\nname: company\nbackup:\n  include: [data/finance]\n").unwrap();
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
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let engine = Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new())
            .with_backup_store(BackupStore::open(&database).unwrap());
        (Arc::new(engine), base)
    }

    /// `#279`: the live report shows a declared directory before any
    /// backup has ever run (and before the directory even exists, as a
    /// finding rather than a refusal), then the newest snapshot's own
    /// count once one is taken -- the exact "declared, missing, created,
    /// included with no restart" path `resolve_scope_includes` itself
    /// already covers in isolation, now through the whole report.
    #[tokio::test]
    async fn an_engine_report_resolves_a_declared_directory_live_and_counts_it_once_backed_up() {
        let (engine, base) = engine_backing_up_with_scope_include("{ daily: 1, weekly: 0, monthly: 0 }", "destination");

        let before = engine.backup_report().await.unwrap();
        assert_eq!(before.scope_includes.len(), 1);
        assert_eq!(before.scope_includes[0].scope, "company");
        assert_eq!(before.scope_includes[0].declared, "data/finance");
        assert_eq!(before.scope_includes[0].path.as_deref(), Some("data/finance"));
        assert_eq!(before.scope_includes[0].unavailable.as_deref(), Some("declared, but does not exist yet"));
        assert_eq!(before.scope_includes[0].files, None);

        std::fs::create_dir_all(base.join("instance/data/finance")).unwrap();
        std::fs::write(base.join("instance/data/finance/ledger.csv"), "ok\n").unwrap();

        let taken = engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        assert!(
            taken.scope_includes.iter().any(|t| t.path.as_deref() == Some("data/finance") && t.files == 1),
            "{:?}",
            taken.scope_includes
        );

        let report = engine.backup_report().await.unwrap();
        let row = report.scope_includes.iter().find(|r| r.scope == "company").unwrap();
        assert_eq!(row.unavailable, None, "the directory exists now, with no restart");
        assert_eq!(row.files, Some(1));
        assert_eq!(row.bytes, Some(3));

        std::fs::remove_dir_all(base).ok();
    }

    /// `engine_backing_up`, with `infrastructure.backup.encrypt_to` set to
    /// `recipient` (`#152`) -- for the engine-level round trip through
    /// `backup_run`/`backup_verify`/`backup_restore`, not just `archive.rs`'s
    /// own unit tests.
    fn engine_backing_up_encrypted(keep: &str, destination: &str, recipient: &str) -> (Arc<Engine>, PathBuf) {
        use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration};
        use factory_plugins::{Registry, SqliteStore};
        let base = std::env::temp_dir().join(format!("factory-backup-encrypted-engine-{}", uuid::Uuid::new_v4()));
        let root = base.join("instance");
        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::write(root.join(".factory/config.yaml"), "version: 1\ninstance:\n  id: test\n  name: test\n").unwrap();
        std::fs::write(root.join(".factory/knowledge/page.md"), "# A page\n").unwrap();
        std::fs::write(root.join(".factory/secrets.yaml"), "api: hunter2-secret-marker").unwrap();
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
                "backup:\n  destination: {}\n  keep: {keep}\n  encrypt_to: {recipient}\n",
                destination.display()
            ))
            .unwrap(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let engine = Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new())
            .with_backup_store(BackupStore::open(&database).unwrap());
        (Arc::new(engine), base)
    }

    /// A generated age identity, written to its own file directly under the
    /// OS temp directory (never inside an instance's own `.factory/`, and
    /// independent of any one engine's own temp `base`, since the recipient
    /// has to exist before a config naming it can be built), plus its public
    /// recipient. The caller removes the file when it is done with it.
    fn generated_identity() -> (PathBuf, String) {
        use age::secrecy::ExposeSecret;
        let identity = age::x25519::Identity::generate();
        let recipient = identity.to_public().to_string();
        let path = std::env::temp_dir().join(format!("factory-backup-identity-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, identity.to_string().expose_secret()).unwrap();
        (path, recipient)
    }

    /// A fixed instant, so a job-loop test's own math never depends on when
    /// the test happened to run.
    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    /// `engine_backing_up`, plus `verify_schedule` (and, optionally,
    /// `encrypt_to`) and a `booted_at` fixed at construction rather than at
    /// whatever instant the test happened to run -- `#156`'s own fixture for
    /// the drill's job-loop tests, so `tick`'s due/catch-up math is exact
    /// date arithmetic, never a race against real time.
    fn engine_with_drill(
        keep: &str,
        destination: &str,
        verify_cron: &str,
        encrypt_to: Option<&str>,
        booted_at: DateTime<Utc>,
    ) -> (Arc<Engine>, PathBuf) {
        use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration};
        use factory_plugins::{Registry, SqliteStore};
        let base = std::env::temp_dir().join(format!("factory-backup-drill-engine-{}", uuid::Uuid::new_v4()));
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
        let encrypt_line = encrypt_to.map(|r| format!("\n  encrypt_to: {r}")).unwrap_or_default();
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
                "backup:\n  destination: {}\n  keep: {keep}\n  verify_schedule: {{ cron: \"{verify_cron}\" }}{encrypt_line}\n",
                destination.display()
            ))
            .unwrap(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let mut engine = Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new())
            .with_backup_store(BackupStore::open(&database).unwrap());
        engine.shared.booted_at = booted_at;
        (Arc::new(engine), base)
    }

    /// `#156`: a due slot with a snapshot already in the destination records
    /// exactly one `Verified { by: "schedule" }` row and a `backup_verified`
    /// event, and never rewrites the archive.
    #[tokio::test]
    async fn a_due_drill_records_exactly_one_verified_by_schedule_and_leaves_the_archive_untouched() {
        let booted_at = dt(2026, 1, 1, 0, 0);
        let (engine, base) = engine_with_drill("{ daily: 7 }", "destination", "* * * * *", None, booted_at);
        let snapshot = engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        let mut events = engine.shared.bus.subscribe();
        let path = PathBuf::from(&snapshot.path);
        let before_bytes = std::fs::read(&path).unwrap();
        let before_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        let now = booted_at + Duration::minutes(2);
        tick(&engine, now).await;

        assert!(matches!(events.recv().await.unwrap(), Event::BackupVerified { .. }));
        let verified: Vec<Verification> = engine
            .l1.backups
            .all()
            .await
            .unwrap()
            .into_iter()
            .filter_map(|r| match r {
                Recorded::Verified { verification } => Some(verification),
                _ => None,
            })
            .collect();
        assert_eq!(verified.len(), 1, "{verified:?}");
        assert_eq!(verified[0].by, "schedule");
        assert_eq!(verified[0].at, now, "the drill's own clock, never the wall clock");
        assert_eq!(verified[0].snapshot, snapshot.name);

        assert_eq!(std::fs::read(&path).unwrap(), before_bytes, "a drill never rewrites the archive");
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before_mtime);
        std::fs::remove_dir_all(base).ok();
    }

    /// A second tick in the same due slot drills nothing more: the first
    /// tick's own `Verified` already moved the base past it.
    #[tokio::test]
    async fn a_second_tick_in_the_same_slot_drills_nothing_more() {
        let booted_at = dt(2026, 1, 1, 0, 0);
        let (engine, base) = engine_with_drill("{ daily: 7 }", "destination", "* * * * *", None, booted_at);
        engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        let now = booted_at + Duration::minutes(2);
        tick(&engine, now).await;
        let after_first = engine.l1.backups.all().await.unwrap().len();
        tick(&engine, now).await;
        assert_eq!(engine.l1.backups.all().await.unwrap().len(), after_first, "the same due slot drills only once");
        std::fs::remove_dir_all(base).ok();
    }

    /// A manual verify satisfies the drill's own slot, and catch-up drills
    /// exactly once regardless of how many slots were missed -- never one
    /// per missed slot.
    #[tokio::test]
    async fn a_manual_verify_satisfies_the_slot_and_catch_up_drills_exactly_once() {
        let booted_at = dt(2026, 1, 1, 0, 0);
        let (engine, base) = engine_with_drill("{ daily: 7 }", "destination", "* * * * *", None, booted_at);
        let snapshot = engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        let now = booted_at + Duration::minutes(5);
        engine
            .l1.backups
            .append(Recorded::Verified {
                verification: Verification {
                    snapshot: snapshot.name.clone(),
                    at: now - Duration::minutes(3),
                    by: "owner".into(),
                    ok: true,
                    checks: vec![],
                    duration_ms: 0,
                },
            })
            .await
            .unwrap();

        tick(&engine, now).await;
        let verified_count =
            engine.l1.backups.all().await.unwrap().iter().filter(|r| matches!(r, Recorded::Verified { .. })).count();
        assert_eq!(verified_count, 2, "the seeded manual verify, plus exactly one catch-up drill");
        std::fs::remove_dir_all(base).ok();
    }

    /// Busy (a person's own backup operation) is not a failure: the drill
    /// records nothing and the next tick runs it.
    #[tokio::test]
    async fn a_busy_drill_is_skipped_and_the_next_tick_runs_it() {
        let booted_at = dt(2026, 1, 1, 0, 0);
        let (engine, base) = engine_with_drill("{ daily: 7 }", "destination", "* * * * *", None, booted_at);
        engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        let now = booted_at + Duration::minutes(2);

        let guard = engine.l1.backup_busy.lock().await;
        tick(&engine, now).await;
        assert!(
            !engine.l1.backups.all().await.unwrap().iter().any(|r| matches!(r, Recorded::Verified { .. })),
            "busy: nothing recorded"
        );
        drop(guard);

        tick(&engine, now).await;
        let verified_count =
            engine.l1.backups.all().await.unwrap().iter().filter(|r| matches!(r, Recorded::Verified { .. })).count();
        assert_eq!(verified_count, 1);
        std::fs::remove_dir_all(base).ok();
    }

    /// An empty destination records nothing -- the slot stays due -- and a
    /// backup taken afterward makes the next tick drill exactly once.
    #[tokio::test]
    async fn no_snapshot_yet_leaves_the_slot_due_until_a_backup_exists() {
        let booted_at = dt(2026, 1, 1, 0, 0);
        let (engine, base) = engine_with_drill("{ daily: 7 }", "destination", "* * * * *", None, booted_at);
        let now = booted_at + Duration::minutes(2);

        tick(&engine, now).await;
        assert!(engine.l1.backups.all().await.unwrap().is_empty(), "no snapshot: nothing recorded");

        engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        tick(&engine, now).await;
        let verified_count =
            engine.l1.backups.all().await.unwrap().iter().filter(|r| matches!(r, Recorded::Verified { .. })).count();
        assert_eq!(verified_count, 1);
        std::fs::remove_dir_all(base).ok();
    }

    /// A corrupted archive is recorded `ok: false`, exactly like a manual
    /// verify's own failure, and the loop keeps drilling on later slots.
    #[tokio::test]
    async fn a_corrupted_archive_drill_records_ok_false_and_the_next_slot_still_drills() {
        let booted_at = dt(2026, 1, 1, 0, 0);
        let (engine, base) = engine_with_drill("{ daily: 7 }", "destination", "* * * * *", None, booted_at);
        let snapshot = engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        let mut bytes = std::fs::read(&snapshot.path).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xff;
        std::fs::write(&snapshot.path, &bytes).unwrap();

        let now = booted_at + Duration::minutes(2);
        tick(&engine, now).await;
        let recorded = engine.l1.backups.all().await.unwrap();
        let first = recorded
            .iter()
            .find_map(|r| match r { Recorded::Verified { verification } => Some(verification), _ => None })
            .unwrap();
        assert!(!first.ok, "{:?}", first.checks);
        assert_eq!(first.by, "schedule");

        // The loop keeps going: the next slot still drills.
        tick(&engine, now + Duration::minutes(1)).await;
        let verified_count =
            engine.l1.backups.all().await.unwrap().iter().filter(|r| matches!(r, Recorded::Verified { .. })).count();
        assert_eq!(verified_count, 2);
        std::fs::remove_dir_all(base).ok();
    }

    /// An encrypted newest snapshot is a skip, never a record: the drill
    /// never holds an identity. The report names the reason regardless of
    /// ticking (a plain projection of the live facts), and the job's own
    /// skip tracker moves only when the due slot changes, so the log is not
    /// spammed every tick.
    #[tokio::test]
    async fn an_encrypted_newest_snapshot_is_skipped_and_the_report_names_the_reason() {
        let (_, recipient) = generated_identity();
        let booted_at = dt(2026, 1, 1, 0, 0);
        let (engine, base) = engine_with_drill("{ daily: 7 }", "destination", "* * * * *", Some(&recipient), booted_at);
        engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();

        let report = engine.backup_report().await.unwrap();
        assert_eq!(
            report.verify_skipped.as_deref(),
            Some("newest snapshot is encrypted; verify it with --identity")
        );
        assert!(report.next_verify.is_some());

        let now = booted_at + Duration::minutes(2);
        assert_eq!(*engine.l1.verify_drill_skip.lock().unwrap(), None);
        tick(&engine, now).await;
        assert!(
            !engine.l1.backups.all().await.unwrap().iter().any(|r| matches!(r, Recorded::Verified { .. })),
            "an encrypted snapshot with no identity is refused, never recorded"
        );
        let logged = engine.l1.verify_drill_skip.lock().unwrap().clone();
        assert!(logged.is_some(), "the skip is tracked so the job logs it only once per slot");

        // Ticking again in the same slot must not move the tracked slot.
        tick(&engine, now).await;
        assert_eq!(engine.l1.verify_drill_skip.lock().unwrap().clone(), logged);

        std::fs::remove_dir_all(base).ok();
    }

    /// A config with no `verify_schedule` round-trips with no drill
    /// scheduled -- `next_verify`/`verify_skipped` are both `None`, and
    /// every existing backup-only behaviour is unchanged.
    #[tokio::test]
    async fn a_config_without_verify_schedule_round_trips_with_no_drill_scheduled() {
        let (engine, base) = engine_backing_up("{ daily: 7 }", "destination");
        let report = engine.backup_report().await.unwrap();
        assert_eq!(report.next_verify, None);
        assert_eq!(report.verify_skipped, None);
        assert_eq!(report.config.as_ref().unwrap().verify_schedule, None);
        std::fs::remove_dir_all(base).ok();
    }

    /// The daemon-start refusal covers `verify_schedule` exactly like
    /// `schedule`: an unfireable cron is refused before the daemon does
    /// anything else, never discovered a minute later as a warning log.
    #[tokio::test]
    async fn validate_config_refuses_an_unfireable_verify_schedule_the_same_way_it_refuses_a_bad_backup_schedule() {
        let (engine, base) = engine_backing_up("{ daily: 7 }", "destination");
        let mut factory = engine.factory_snapshot();
        factory.config.infrastructure.backup.as_mut().unwrap().verify_schedule =
            Some(factory_core::backup::BackupSchedule { cron: "not a cron".into(), timezone: None });
        let e = validate_config(&factory).unwrap_err();
        assert!(e.to_string().contains("verify_schedule"), "{e}");
        std::fs::remove_dir_all(base).ok();
    }

    /// `engine_backing_up`, plus a `dsgvo` catalogue naming `backup_verified`
    /// and `backup_offsite` -- `#154`'s engine test needs a real policy
    /// report to prove the lazy evidence wiring end to end, not just
    /// `resolve_backup_fact` on its own.
    fn engine_backing_up_with_backup_policy(keep: &str, destination: &str) -> (Arc<Engine>, PathBuf) {
        use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration};
        use factory_plugins::{Registry, SqliteStore};
        let base = std::env::temp_dir().join(format!("factory-backup-policy-engine-{}", uuid::Uuid::new_v4()));
        let root = base.join("instance");
        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::create_dir_all(root.join(".factory/policies")).unwrap();
        std::fs::write(root.join(".factory/config.yaml"), "version: 1\ninstance:\n  id: test\n  name: test\n").unwrap();
        std::fs::write(root.join(".factory/knowledge/page.md"), "# A page\n").unwrap();
        std::fs::write(
            root.join(".factory/policies/dsgvo.yaml"),
            "framework: dsgvo\n\
             title: DSGVO\n\
             kind: regulation\n\
             controls:\n\
             \x20\x20- id: art-32-restore\n\x20\x20\x20\x20title: Availability can be restored\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: daemon\n\x20\x20\x20\x20\x20\x20\x20\x20fact: backup_verified\n\
             \x20\x20- id: art-32-offsite\n\x20\x20\x20\x20title: Offsite copy\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: daemon\n\x20\x20\x20\x20\x20\x20\x20\x20fact: backup_offsite\n",
        )
        .unwrap();
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
            policies: PolicyDeclaration { frameworks: vec!["dsgvo".to_string()], ..Default::default() },
            quality: Default::default(),
            infrastructure: serde_yaml_ng::from_str(&format!(
                "backup:\n  destination: {}\n  keep: {keep}\n",
                destination.display()
            ))
            .unwrap(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let engine = Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new())
            .with_backup_store(BackupStore::open(&database).unwrap());
        (Arc::new(engine), base)
    }

    /// `#154`, end to end: back up and verify, then a policy report's
    /// `daemon: backup_verified` control reads `Satisfied` and its
    /// `backup_offsite` control stays `Open` -- the temp destination sits
    /// beside the instance, so it is not offsite.
    #[tokio::test]
    async fn backup_and_verify_satisfy_a_backup_verified_control_and_leave_backup_offsite_open() {
        let (engine, base) = engine_backing_up_with_backup_policy("{ daily: 7 }", "destination");
        engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        engine.backup_verify(None, None, "owner".into()).await.unwrap();

        let report = engine.policy_report(None).await.unwrap();
        let company = &report.rows.iter().find(|r| r.scope == "company").unwrap().statuses;

        let verified = company.iter().find(|s| s.control.id == "art-32-restore").unwrap();
        assert_eq!(
            verified.status.kind(),
            factory_core::policy::StatusKind::Satisfied,
            "{:?}",
            verified.status
        );

        let offsite = company.iter().find(|s| s.control.id == "art-32-offsite").unwrap();
        assert_eq!(
            offsite.status.kind(),
            factory_core::policy::StatusKind::Open,
            "the temp destination is on the same device as the instance: {:?}",
            offsite.status
        );

        std::fs::remove_dir_all(base).ok();
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
        let mut events = engine.shared.bus.subscribe();

        let before = engine.backup_report().await.unwrap();
        assert_eq!(before.age, AgeLevel::None);
        assert!(before.warnings.iter().any(|w| w.kind == "no_backup"));
        assert!(before.warnings.iter().any(|w| w.kind == "destination_missing"));

        let first = engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
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

        let verification = engine.backup_verify(None, None, "owner".into()).await.unwrap();
        assert!(verification.ok, "{:?}", verification.checks);
        assert_eq!(verification.snapshot, first.name);
        assert!(matches!(events.recv().await.unwrap(), Event::BackupVerified { .. }));
        let report = engine.backup_report().await.unwrap();
        assert_eq!(report.last_verified.as_ref().map(|v| v.ok), Some(true));
        assert!(!report.warnings.iter().any(|w| w.kind == "never_verified"));

        // A second backup a second later, the same day: `daily: 1` keeps
        // only the newest, so retention deletes the first.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let second = engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        assert_eq!(second.pruned, [first.name.clone()]);
        let report = engine.backup_report().await.unwrap();
        assert_eq!(report.snapshots.len(), 1);
        assert_eq!(report.last_verified, None, "the verified snapshot is gone, so nothing verified is left");

        let e = engine.backup_verify(Some("../etc.tar.zst".into()), None, "owner".into()).await.unwrap_err();
        assert!(e.to_string().contains("not a snapshot name"), "{e}");
        std::fs::remove_dir_all(base).ok();
    }

    /// A destination that cannot be written is a recorded, published
    /// failure -- and the next report says so -- never a crash.
    #[tokio::test]
    async fn a_failed_backup_is_recorded_published_and_warned_about() {
        let (engine, base) = engine_backing_up("{ daily: 7 }", "unmounted/volume");
        let mut events = engine.shared.bus.subscribe();
        let e = engine.l1_service().backup_run(BackupTrigger::Schedule, "schedule".into()).await.unwrap_err();
        assert!(e.to_string().contains("mounted"), "{e}");
        assert!(matches!(events.recv().await.unwrap(), Event::BackupFailed { .. }));
        let report = engine.backup_report().await.unwrap();
        assert_eq!(report.last_failure.as_ref().map(|f| f.trigger), Some(BackupTrigger::Schedule));
        assert!(report.warnings.iter().any(|w| w.kind == "last_failed"));
        std::fs::remove_dir_all(base).ok();
    }

    /// The failure-cleanup path is shared code regardless of encryption, but
    /// `#152` adds a new writer in front of it -- prove an encrypted backup
    /// against an unmounted destination is still a clean, recorded failure,
    /// never a partial `.age` file or a plaintext one left anywhere.
    #[tokio::test]
    async fn a_failed_encrypted_backup_is_recorded_and_leaves_the_destination_empty() {
        let (_, recipient) = generated_identity();
        let (engine, base) = engine_backing_up_encrypted("{ daily: 7 }", "unmounted/volume", &recipient);
        let mut events = engine.shared.bus.subscribe();
        let e = engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap_err();
        assert!(e.to_string().contains("mounted"), "{e}");
        assert!(matches!(events.recv().await.unwrap(), Event::BackupFailed { .. }));
        let report = engine.backup_report().await.unwrap();
        assert!(report.warnings.iter().any(|w| w.kind == "last_failed"));
        std::fs::remove_dir_all(base).ok();
    }

    /// `#152`, end to end through the engine: an encrypted backup names only
    /// `.age` archives, verifying it needs the matching identity or is
    /// refused before anything is recorded, a wrong identity still records a
    /// failed `decrypt` check, and restore follows the same rule -- and
    /// nowhere in any of it, including the serialized wire shapes and the
    /// published events, does the identity's own secret ever appear.
    #[tokio::test]
    async fn an_encrypted_backup_round_trips_through_the_engine_and_never_leaks_its_identity() {
        let (identity_path, recipient) = generated_identity();
        let (engine, base) = engine_backing_up_encrypted("{ daily: 7 }", "destination", &recipient);
        let mut events = engine.shared.bus.subscribe();

        let snapshot = engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        assert!(snapshot.name.ends_with(".tar.zst.age"), "{}", snapshot.name);
        assert_eq!(snapshot.encrypted_to.as_deref(), Some(recipient.as_str()));
        assert!(matches!(events.recv().await.unwrap(), Event::BackupCompleted { .. }));

        // No plaintext byte anywhere in the destination.
        let archive_path = std::path::PathBuf::from(&snapshot.path);
        let bytes = std::fs::read(&archive_path).unwrap();
        assert!(factory_core::backup::looks_encrypted(&bytes));
        let haystack = String::from_utf8_lossy(&bytes);
        assert!(!haystack.contains("hunter2-secret-marker"), "the secret reached the destination");
        assert!(!haystack.contains(factory_core::backup::MANIFEST_FILE));

        let report = engine.backup_report().await.unwrap();
        assert_eq!(report.snapshots.len(), 1);
        assert!(report.snapshots[0].encrypted, "encrypted is read from the archive's own bytes");
        assert!(
            report.warnings.iter().any(|w| w.kind == "never_verified" && w.message.contains("--identity")),
            "{:?}",
            report.warnings
        );

        // No identity: refused, and nothing recorded -- neither a store row
        // nor a published event.
        let before = engine.l1.backups.all().await.unwrap().len();
        let refused = engine.backup_verify(None, None, "owner".into()).await.unwrap_err();
        assert!(refused.to_string().contains("--identity"), "{refused}");
        assert_eq!(engine.l1.backups.all().await.unwrap().len(), before, "a refusal records nothing");

        // The right identity: verifies clean, decrypt named in the checks.
        let verification =
            engine.backup_verify(None, Some(identity_path.clone()), "owner".into()).await.unwrap();
        assert!(verification.ok, "{:?}", verification.checks);
        assert!(verification.checks.iter().any(|c| c.name == "decrypt" && c.status == CheckStatus::Ok));
        assert!(matches!(events.recv().await.unwrap(), Event::BackupVerified { .. }));

        // The wrong identity: still recorded, but only the decrypt check
        // fails.
        let (wrong_path, _) = generated_identity();
        let wrong = engine.backup_verify(None, Some(wrong_path.clone()), "owner".into()).await.unwrap();
        assert!(!wrong.ok);
        assert!(wrong.checks.iter().any(|c| c.name == "decrypt" && c.status == CheckStatus::Fail));

        // Restore: refused without an identity, works with the right one.
        let restore_into = base.join("restored-no-identity");
        let restore_refused =
            engine.backup_restore(snapshot.name.clone(), restore_into.clone(), None).await.unwrap_err();
        assert!(restore_refused.to_string().contains("--identity"), "{restore_refused}");
        assert!(!restore_into.exists());

        let restore_into = base.join("restored");
        let restoration = engine
            .backup_restore(snapshot.name.clone(), restore_into.clone(), Some(identity_path.clone()))
            .await
            .unwrap();
        assert!(factory_core::config::Factory::load(&restore_into).is_ok());
        assert!(!restore_into.join(".factory/secrets.yaml").exists());
        assert_eq!(restoration.snapshot, snapshot.name);

        // Never once does the identity's own secret text reach anything
        // that got serialized or published.
        let secret = std::fs::read_to_string(&identity_path).unwrap();
        for haystack in [
            serde_json::to_string(&verification).unwrap(),
            serde_json::to_string(&wrong).unwrap(),
            serde_json::to_string(&report).unwrap(),
            refused.to_string(),
            restore_refused.to_string(),
        ] {
            assert!(!haystack.contains(secret.trim()), "the identity's secret leaked: {haystack}");
        }

        std::fs::remove_file(&identity_path).ok();
        std::fs::remove_file(&wrong_path).ok();
        std::fs::remove_dir_all(base).ok();
    }

    /// `#152`: retention treats a destination holding both formats as one
    /// history -- a plaintext archive planted as if a pre-`#152` daemon had
    /// taken it is pruned by the very same `daily: 1` rule an encrypted
    /// backup taken afterwards is.
    #[tokio::test]
    async fn retention_prunes_across_a_mixed_plaintext_and_encrypted_history() {
        let (identity_path, recipient) = generated_identity();
        let (engine, base) =
            engine_backing_up_encrypted("{ daily: 1, weekly: 0, monthly: 0 }", "destination", &recipient);
        let destination = base.join("destination");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(
            destination.join("factory-backup-test-20200101T030000Z.tar.zst"),
            b"not a real archive; retention only ever looks at a listed name and date",
        )
        .unwrap();

        let snapshot = engine.l1_service().backup_run(BackupTrigger::Manual, "owner".into()).await.unwrap();
        assert_eq!(
            snapshot.pruned,
            ["factory-backup-test-20200101T030000Z.tar.zst"],
            "the older plaintext archive is pruned by the same daily: 1 rule as the new encrypted one"
        );
        let report = engine.backup_report().await.unwrap();
        assert_eq!(report.snapshots.len(), 1);
        assert!(report.snapshots[0].encrypted);

        std::fs::remove_file(&identity_path).ok();
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

    /// `#152`: a mixed destination of plaintext and encrypted archives is
    /// one history, newest first, each correctly marked from its own bytes
    /// -- and a half-written `.age` archive, or another instance's, stays
    /// exactly as invisible as the plaintext equivalents already are.
    #[test]
    fn a_mixed_plaintext_and_encrypted_destination_is_one_history() {
        let dir = std::env::temp_dir().join(format!("factory-backup-list-mixed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("factory-backup-demo-20260924T030000Z.tar.zst"), b"not really zstd").unwrap();
        std::fs::write(
            dir.join("factory-backup-demo-20260925T030000Z.tar.zst.age"),
            [factory_core::backup::AGE_MAGIC, b"\n-> X25519 ..."].concat(),
        )
        .unwrap();
        std::fs::write(dir.join(".factory-backup-demo-20260926T030000Z.tar.zst.age.partial"), b"partial").unwrap();
        std::fs::write(dir.join("factory-backup-other-20260925T030000Z.tar.zst.age"), b"not ours").unwrap();
        let found = list_archives(&dir, "demo");
        let rows: Vec<(&str, bool)> = found.iter().map(|f| (f.name.as_str(), f.encrypted)).collect();
        assert_eq!(
            rows,
            [
                ("factory-backup-demo-20260925T030000Z.tar.zst.age", true),
                ("factory-backup-demo-20260924T030000Z.tar.zst", false),
            ]
        );
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
            encrypted_to: None,
            scope_includes: vec![],
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
