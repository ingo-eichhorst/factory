//! L1's one live backup gatherer and fact projection, also used by its page.
//! Archive listing never runs git, tmutil, a scanner or a restore.
use crate::backup::{
    age_level, looks_encrypted, parse_archive_name, resolve_backup_fact, AgeLevel, BackupConfig,
    BackupFact, DestinationFacts, Snapshot, Verification, VerifySummary, ENCRYPTED_ARCHIVE_SUFFIX,
    GRACE_HOURS, UNSCHEDULED_OVERDUE_HOURS, UNSCHEDULED_STALE_HOURS,
};
use crate::backup_store::Recorded;
use chrono::{DateTime, Duration, Utc};
use factory_kernel::{FactoryError, Result};
use std::path::{Path, PathBuf};

pub struct Provider<'a> {
    pub store: &'a crate::backup_store::BackupStore,
    pub busy: &'a tokio::sync::Mutex<()>,
    pub root: PathBuf,
    pub instance: String,
    pub config: Option<BackupConfig>,
    pub booted_at: DateTime<Utc>,
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L1;
}
#[async_trait::async_trait]
impl factory_kernel::Provide<BackupFact> for Provider<'_> {
    type Query = DateTime<Utc>;
    type Value = BackupFact;
    type Error = FactoryError;
    async fn get(&self, now: &DateTime<Utc>) -> Result<BackupFact> {
        Ok(fact(&self.capture(*now).await?))
    }
}
impl Provider<'_> {
    pub async fn capture(&self, now: DateTime<Utc>) -> Result<Captured> {
        let running = self.busy.try_lock().is_err();
        let recorded = self.store.all().await?;
        let Some(config) = self.config.clone() else {
            return Ok(Captured {
                now,
                running,
                recorded,
                config: None,
                destination: None,
                found: Vec::new(),
                next_run: None,
                next_verify: None,
            });
        };

        let (root, destination, instance) = (
            self.root.to_path_buf(),
            config.destination.clone(),
            self.instance.to_string(),
        );
        let (facts, found) = tokio::task::spawn_blocking(move || {
            (
                destination_facts(&root, &destination),
                list_archives(&destination, &instance),
            )
        })
        .await
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("listing the destination: {e}")))?;

        let next_run = match &config.schedule {
            Some(schedule) => {
                // The same "later of the newest attempt and the newest
                // archive" `last_attempt` computes -- inlined against
                // `recorded`/`found` already in hand, rather than a second
                // `backups.all()` and a second `list_archives` over the
                // same destination for the one gather this method promises.
                let attempted = recorded
                    .iter()
                    .filter(|r| matches!(r, Recorded::Completed { .. } | Recorded::Failed { .. }))
                    .map(|r| r.at())
                    .max();
                let base = attempted
                    .max(found.first().map(|f| f.at))
                    .unwrap_or(self.booted_at);
                factory_kernel::schedule_grid::next_after(&schedule.as_task_schedule(), base)
                    .ok()
                    .map(|next| next.max(now))
            }
            None => None,
        };

        // `#156`: the drill's own next slot -- the same "inline against
        // `recorded` already in hand" `next_run` follows above, so this
        // gather still costs one `backups.all()`, not two.
        let next_verify = match &config.verify_schedule {
            Some(schedule) => {
                let last_verified = recorded
                    .iter()
                    .filter_map(|r| match r {
                        Recorded::Verified { verification } => Some(verification.at),
                        _ => None,
                    })
                    .max();
                let base = last_verified.unwrap_or(self.booted_at);
                factory_kernel::schedule_grid::next_after(&schedule.as_task_schedule(), base)
                    .ok()
                    .map(|next| next.max(now))
            }
            None => None,
        };
        Ok(Captured {
            now,
            running,
            recorded,
            config: Some(config),
            destination: Some(facts),
            found,
            next_run,
            next_verify,
        })
    }
}
/// One archive of this instance found in the destination.
#[derive(Debug, Clone)]
pub struct Found {
    pub name: String,
    pub at: DateTime<Utc>,
    pub size_bytes: u64,
    /// Read from the file's own bytes, never its name or the config -- see
    /// [`peek_encrypted`].
    pub encrypted: bool,
}

/// Every archive of this instance in `destination`, newest first -- only
/// names `parse_archive_name` accepts, so nothing else in a shared folder is
/// ever listed, verified or deleted.
pub fn list_archives(destination: &Path, instance: &str) -> Vec<Found> {
    let Ok(read) = std::fs::read_dir(destination) else {
        return Vec::new();
    };
    let mut found: Vec<Found> = read
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let at = parse_archive_name(instance, &name)?;
            let meta = entry.metadata().ok().filter(|m| m.is_file())?;
            let encrypted = peek_encrypted(&entry.path(), &name);
            Some(Found {
                name,
                at,
                size_bytes: meta.len(),
                encrypted,
            })
        })
        .collect();
    found.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.name.cmp(&a.name)));
    found
}

/// Whether `path` is age-encrypted (`#152`): read from its own first bytes,
/// never trusted from `name`'s suffix or the live config, so a renamed file
/// or a config changed since it was written can never be misreported --
/// falling back to the name only when the file itself could not even be
/// opened, so a listing never wrongly calls an encrypted archive plain and
/// so never asks nobody for the identity it actually needs.
pub fn peek_encrypted(path: &Path, name: &str) -> bool {
    use std::io::Read;
    let mut header = [0u8; crate::backup::AGE_MAGIC.len()];
    match std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut header)) {
        Ok(()) => looks_encrypted(&header),
        Err(_) => name.ends_with(ENCRYPTED_ARCHIVE_SUFFIX),
    }
}

/// The facts the page and `WarningFacts` need about the destination. The
/// device is only compared when the destination exists: the parent of a
/// missing mount point is on the system disk, and saying "same device"
/// then would be a guess.
pub fn destination_facts(root: &Path, destination: &Path) -> DestinationFacts {
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
pub fn deadlines(
    config: &BackupConfig,
    newest: DateTime<Utc>,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let grace = Duration::hours(GRACE_HOURS);
    match &config.schedule {
        Some(schedule) => {
            let schedule = schedule.as_task_schedule();
            let first = factory_kernel::schedule_grid::next_after(&schedule, newest).ok();
            let second =
                first.and_then(|f| factory_kernel::schedule_grid::next_after(&schedule, f).ok());
            (first.map(|t| t + grace), second.map(|t| t + grace))
        }
        None => (
            Some(newest + Duration::hours(UNSCHEDULED_STALE_HOURS)),
            Some(newest + Duration::hours(UNSCHEDULED_OVERDUE_HOURS)),
        ),
    }
}

/// The whole L1 gather, shared by page and fact projections: neither reads
/// a file, the store or the clock again. Repository and Time Machine probes
/// are supplied separately to page composition; the fact never sees them.
pub struct Captured {
    pub now: DateTime<Utc>,
    pub running: bool,
    /// Every `backup_events` row: completed, failed and verified attempts.
    pub recorded: Vec<Recorded>,
    /// `None` when `infrastructure.backup` is not set.
    pub config: Option<BackupConfig>,
    /// `Some` exactly when `config` is -- the destination's own facts,
    /// whether or not it turned out to exist.
    pub destination: Option<DestinationFacts>,
    /// Every archive of this instance found in the destination, newest
    /// first. Empty when unconfigured or when the destination could not be
    /// listed.
    pub found: Vec<Found>,
    pub next_run: Option<DateTime<Utc>>,
    /// `#156`: the verification drill's own next slot -- `None` when
    /// `verify_schedule` is not configured.
    pub next_verify: Option<DateTime<Utc>>,
}

pub fn completed_of(recorded: &[Recorded]) -> Vec<&Snapshot> {
    recorded
        .iter()
        .filter_map(|r| match r {
            Recorded::Completed { snapshot } => Some(snapshot),
            _ => None,
        })
        .collect()
}

pub fn verifications_of(recorded: &[Recorded]) -> Vec<&Verification> {
    recorded
        .iter()
        .filter_map(|r| match r {
            Recorded::Verified { verification } => Some(verification),
            _ => None,
        })
        .collect()
}

/// The newest verification of a snapshot still in the destination --
/// `warnings`' and now `resolve_backup_fact`'s own rule: a pass on a
/// snapshot retention has since deleted proves nothing about what is left.
pub fn last_verified_of_present(
    verifications: &[&Verification],
    found: &[Found],
) -> Option<VerifySummary> {
    verifications
        .iter()
        .find(|v| found.iter().any(|f| f.name == v.snapshot))
        .map(|v| v.summary())
}

/// `#154`'s `BackupFact`, from the same `Captured` state -- never the
/// repository or Time Machine probes `report` takes as extra arguments:
/// this is the whole reason a policy report or a metric call, which only
/// ever wants this projection, never spawns `git` or `tmutil`.
pub fn fact(state: &Captured) -> BackupFact {
    let configured = state.config.is_some();
    let destination_exists = state.destination.as_ref().is_some_and(|d| d.exists);
    let same_device = state.destination.as_ref().and_then(|d| d.same_device);
    let (newest, age, last_verified) =
        if let (Some(config), true) = (&state.config, destination_exists) {
            let newest = state.found.first().map(|f| f.at);
            let (due_by, overdue_by) = match newest {
                Some(at) => deadlines(config, at),
                None => (None, None),
            };
            let age = age_level(state.now, newest, due_by, overdue_by);
            let verifications = verifications_of(&state.recorded);
            let last_verified = last_verified_of_present(&verifications, &state.found);
            (newest, age, last_verified)
        } else {
            (None, AgeLevel::None, None)
        };
    resolve_backup_fact(
        state.now,
        configured,
        destination_exists,
        same_device,
        newest,
        age,
        last_verified,
    )
}
