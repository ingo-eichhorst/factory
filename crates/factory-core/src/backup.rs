//! L1 Backup (`#116`): what a snapshot of the instance's own state is, how
//! long each one is kept, and what the page is told about them.
//!
//! Everything here is pure -- the config block, the manifest every archive
//! carries, the wire report, and the three decisions that must never depend
//! on a disk being there to be tested: grandfather-father-son retention
//! ([`retain`]), how old is too old ([`age_level`]) and which honest warnings
//! a set of facts adds up to ([`warnings`]). Taking a snapshot, verifying one
//! and the daemon job that runs them live in `factory-daemon`'s `backup`
//! module, which is the only place that touches a file and also stages a
//! verified archive into a new root for owner-only restore.
//!
//! **What a backup is.** A consistent copy of the database (taken with
//! `VACUUM INTO`, never a file copy -- the stores use WAL), the
//! authored-content directories nothing in Factory regenerates, and the
//! configs that give the instance its shape, as one
//! `factory-backup-<instance>-<utc>.tar.zst` with a `manifest.json` naming
//! the path, size and sha256 of every other file in it. Secrets are never
//! read, let alone copied.

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::error::{FactoryError, Result};

/// Every archive's name starts with this, then the instance's slug.
pub const ARCHIVE_PREFIX: &str = "factory-backup-";
/// And ends with this, or -- when it is encrypted (`#152`) --
/// [`ENCRYPTED_ARCHIVE_SUFFIX`].
pub const ARCHIVE_SUFFIX: &str = ".tar.zst";
/// An archive encrypted to `infrastructure.backup.encrypt_to` ends with this
/// instead. `parse_archive_name` and `refuse_bad_snapshot_name` accept
/// either, so retention and listing treat one archive history across both.
pub const ENCRYPTED_ARCHIVE_SUFFIX: &str = ".tar.zst.age";
/// The first bytes of every age-encrypted file (the `age-encryption.org/v1`
/// version line, without its newline) -- what [`looks_encrypted`] reads,
/// straight from the archive, never from its name or the config.
pub const AGE_MAGIC: &[u8] = b"age-encryption.org/v1";
/// Written into every archive, after every file it describes -- see
/// [`Manifest`].
pub const MANIFEST_FILE: &str = "manifest.json";
/// The format of [`Manifest`], bumped when a reader would need to know.
pub const MANIFEST_VERSION: u32 = 1;
/// Where the database copy sits inside an archive: the same path it has
/// under the instance root, so an unpacked archive is laid out like one.
pub const DATABASE_ENTRY: &str = ".factory/factory.sqlite";
/// The spelling of an archive's UTC timestamp: `20260925T030000Z`.
const STAMP_FORMAT: &str = "%Y%m%dT%H%M%SZ";

/// How long past its due time a backup may be before it counts as stale: a
/// nightly job that takes a few minutes, or a daemon restarted a little after
/// the slot, is not a missed night.
pub const GRACE_HOURS: i64 = 2;

/// With no `schedule:`, the age a backup may reach before it counts as
/// stale, and then as overdue. Backups then happen only when somebody asks
/// for one, so this is only a yardstick for the page, never a trigger.
pub const UNSCHEDULED_STALE_HOURS: i64 = 26;
pub const UNSCHEDULED_OVERDUE_HOURS: i64 = 7 * 24;

// ================================================================== config

/// The root config's `infrastructure.backup` block.
///
/// ```yaml
/// infrastructure:
///   backup:
///     destination: /Volumes/Backup/factory
///     schedule: { cron: "0 3 * * *", timezone: Europe/Berlin }
///     keep: { daily: 7, weekly: 4, monthly: 6 }
///     include_logs: false
/// ```
///
/// `deny_unknown_fields`, like the rest of `infrastructure:`. `encrypt_to`
/// is v2 (`#152`, `age`): a single native X25519 recipient (`age1…`) every
/// snapshot is encrypted to. The `age` crate that actually parses it lives
/// in `factory-daemon`, not here -- this struct and [`BackupConfig::validate`]
/// stay dependency-free, the same way the cron expression below is only
/// checked for emptiness here and parsed for real at daemon load (`croner`,
/// `chrono-tz`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupConfig {
    /// A local path: an external disk, a NAS mount, a synced folder. Must be
    /// absolute. Created on the first backup if it does not exist yet.
    pub destination: PathBuf,
    /// When the daemon takes one on its own. Without it, backups happen only
    /// when somebody runs one -- and the page says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<BackupSchedule>,
    /// When the daemon runs a verification drill on its own (`#156`),
    /// independent of when a backup itself runs -- the same shape as
    /// `schedule`. `None` (the default) means no drill: verification then
    /// happens only when somebody runs `factory backup verify`. Due is
    /// counted from the newest verification recorded of any trigger (a
    /// manual verify satisfies the slot too), or from when the daemon
    /// booted if there is none yet, so a daemon down across several slots
    /// drills exactly once when it returns -- `factory_daemon::backup::run`'s
    /// own job loop, not this crate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_schedule: Option<BackupSchedule>,
    #[serde(default)]
    pub keep: Keep,
    /// Also copy `.factory/guides/` and `.factory/logs/`: useful, not
    /// essential, and the one part of a snapshot that may grow without bound.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_logs: bool,
    /// A single native X25519 recipient (`age1…`) to encrypt every snapshot
    /// to (`#152`). An SSH or plugin recipient (`age1yubikey1…`,
    /// `ssh-ed25519 …`) is refused at daemon load
    /// (`factory_daemon::backup::validate_config`), not here. `None`
    /// (the default) writes plaintext archives, exactly as v1 did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypt_to: Option<String>,
}

/// A cron expression and the timezone its fields are read in, exactly as a
/// task's schedule has them (`task::CronSchedule`); `None` is UTC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupSchedule {
    pub cron: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl BackupSchedule {
    /// The same schedule as a task would carry it, for the daemon's one
    /// `schedule::next_after`.
    pub fn as_task_schedule(&self) -> crate::task::Schedule {
        crate::task::Schedule::Cron(crate::task::CronSchedule {
            expr: self.cron.clone(),
            timezone: self.timezone.clone(),
        })
    }

    /// `0 3 * * * (Europe/Berlin)`.
    pub fn describe(&self) -> String {
        match &self.timezone {
            Some(tz) => format!("{} ({tz})", self.cron),
            None => format!("{} (UTC)", self.cron),
        }
    }
}

/// Grandfather-father-son: how many of the newest days, ISO weeks and months
/// keep their newest snapshot. See [`retain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keep {
    #[serde(default = "default_daily")]
    pub daily: u32,
    #[serde(default = "default_weekly")]
    pub weekly: u32,
    #[serde(default = "default_monthly")]
    pub monthly: u32,
}

fn default_daily() -> u32 {
    7
}
fn default_weekly() -> u32 {
    4
}
fn default_monthly() -> u32 {
    6
}

impl Default for Keep {
    fn default() -> Self {
        Self { daily: default_daily(), weekly: default_weekly(), monthly: default_monthly() }
    }
}

impl BackupConfig {
    /// The load-time refusals that need nothing but the block itself. The
    /// cron expression and timezone are checked by the daemon at start
    /// (`croner` and `chrono-tz` are its dependencies, not this crate's), and
    /// whether the destination is inside the instance by the daemon too,
    /// which is the one that knows the root.
    pub fn validate(&self) -> Result<()> {
        if !self.destination.is_absolute() {
            return Err(FactoryError::BadRequest(format!(
                "infrastructure.backup.destination {:?} must be an absolute path -- an external disk, \
                 a NAS mount or a synced folder",
                self.destination.display().to_string()
            )));
        }
        if let Some(schedule) = &self.schedule {
            if schedule.cron.trim().is_empty() {
                return Err(FactoryError::BadRequest(
                    "infrastructure.backup.schedule has no cron expression".into(),
                ));
            }
        }
        if let Some(schedule) = &self.verify_schedule {
            if schedule.cron.trim().is_empty() {
                return Err(FactoryError::BadRequest(
                    "infrastructure.backup.verify_schedule has no cron expression".into(),
                ));
            }
        }
        if self.keep.daily == 0 && self.keep.weekly == 0 && self.keep.monthly == 0 {
            return Err(FactoryError::BadRequest(
                "infrastructure.backup.keep keeps nothing: every snapshot but the newest would be deleted \
                 after each backup. Keep at least one daily, weekly or monthly snapshot"
                    .into(),
            ));
        }
        if let Some(encrypt_to) = &self.encrypt_to {
            if encrypt_to.trim().is_empty() {
                return Err(FactoryError::BadRequest("infrastructure.backup.encrypt_to is empty".into()));
            }
        }
        Ok(())
    }
}

// ================================================================ the files

/// Which part of the instance a file in a snapshot belongs to. The include
/// table on the page is one row per group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Database,
    Config,
    Knowledge,
    Datasets,
    Policies,
    Goals,
    Scenarios,
    Quality,
    Vex,
    Intake,
    Guides,
    Logs,
}

impl Group {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::Config => "config",
            Self::Knowledge => "knowledge",
            Self::Datasets => "datasets",
            Self::Policies => "policies",
            Self::Goals => "goals",
            Self::Scenarios => "scenarios",
            Self::Quality => "quality",
            Self::Vex => "vex",
            Self::Intake => "intake",
            Self::Guides => "guides",
            Self::Logs => "logs",
        }
    }
}

/// The authored-content directories under `.factory/`, each copied whole:
/// the exception AGENTS.md makes to "everything under `.factory/` is the
/// daemon's", because nothing regenerates them. `quality/` and `vex/` are
/// here although the issue's table predates them -- AGENTS.md names them
/// with the others. `intake/` (`#169`, per-scope definitions of ready) is
/// the newest of the same kind.
pub const AUTHORED: [(Group, &str, &str); 8] = [
    (Group::Knowledge, ".factory/knowledge/", "the knowledge vault: pages and documents"),
    (Group::Datasets, ".factory/datasets/", "benchmark datasets"),
    (Group::Policies, ".factory/policies/", "policy catalogues, drafts included"),
    (Group::Goals, ".factory/goals/", "direction and cycle files"),
    (Group::Scenarios, ".factory/scenarios/", "scenario files"),
    (Group::Quality, ".factory/quality/", "quality profiles"),
    (Group::Vex, ".factory/vex/", "authored CycloneDX VEX judgments"),
    (Group::Intake, ".factory/intake/", "definitions of ready"),
];

/// The two optional directories `include_logs: true` adds.
pub const OPTIONAL: [(Group, &str, &str); 2] = [
    (Group::Guides, ".factory/guides/", "the guide files Factory writes for harnesses"),
    (Group::Logs, ".factory/logs/", "the daemon's logs"),
];

/// What a snapshot never holds, and why -- the page's exclude table, and the
/// same rules `is_excluded` applies file by file.
pub const EXCLUDED: [(&str, &str); 6] = [
    (".factory/secrets.yaml", "a secret: never read, never copied"),
    ("…/secrets/…", "anything under a secrets directory, for the same reason"),
    (".factory/factory.sqlite-wal, -shm", "transient: the database copy is taken whole with VACUUM INTO"),
    (".factory/worktrees/", "derivable: a run's worktree is rebuilt from its scope's repository"),
    (".factory/factory.sock", "transient: the daemon's control socket"),
    ("scope source code", "backed up by pushing each scope's repository to its git remote"),
];

/// Whether a file under an included directory is nonetheless left out, and
/// why. Checked on the path relative to the instance root. A secret is never
/// even opened: this runs before a file is read.
pub fn is_excluded(relative: &str) -> Option<&'static str> {
    let segments: Vec<&str> = relative.split('/').collect();
    let name = segments.last().copied().unwrap_or_default();
    if name == "secrets.yaml" || segments.iter().any(|s| *s == "secrets") {
        return Some("a secret: never read, never copied");
    }
    if name == ".env" || name.starts_with(".env.") {
        return Some("an environment file, which may hold secrets");
    }
    if name.ends_with("-wal") || name.ends_with("-shm") || name.ends_with(".sock") {
        return Some("transient");
    }
    None
}

/// The archive name for a snapshot taken at `at`:
/// `factory-backup-<instance>-20260925T030000Z.tar.zst`, or
/// `...tar.zst.age` when `encrypted`.
pub fn archive_name(instance: &str, at: DateTime<Utc>, encrypted: bool) -> String {
    let suffix = if encrypted { ENCRYPTED_ARCHIVE_SUFFIX } else { ARCHIVE_SUFFIX };
    format!("{ARCHIVE_PREFIX}{}-{}{suffix}", slug(instance), at.format(STAMP_FORMAT))
}

/// The time an archive of `instance` was taken, read back off its name --
/// `None` for anything that is not one of this instance's archives. The only
/// test retention and the listing apply before they look at a file: nothing
/// in a destination that does not match is ever listed, verified or deleted.
/// Accepts either suffix (`#152`), so a mixed destination of plaintext and
/// encrypted archives is one history.
pub fn parse_archive_name(instance: &str, name: &str) -> Option<DateTime<Utc>> {
    let rest = name.strip_prefix(ARCHIVE_PREFIX)?;
    let rest = rest.strip_suffix(ENCRYPTED_ARCHIVE_SUFFIX).or_else(|| rest.strip_suffix(ARCHIVE_SUFFIX))?;
    let stamp = rest.strip_prefix(&slug(instance))?.strip_prefix('-')?;
    chrono::NaiveDateTime::parse_from_str(stamp, STAMP_FORMAT)
        .ok()
        .map(|t| t.and_utc())
}

/// Whether a header of at least [`AGE_MAGIC`]'s length starts with it -- the
/// one honest source of `SnapshotRow.encrypted`: read from the archive
/// itself, never trusted from its name's suffix or the config, so a renamed
/// or hand-copied file can never be misreported either way.
pub fn looks_encrypted(header: &[u8]) -> bool {
    header.starts_with(AGE_MAGIC)
}

/// The instance name as it appears in a file name: lowercase ASCII letters,
/// digits and single dashes.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "instance".into() } else { out }
}

/// Refuse a snapshot name off the wire that could name anything but a file
/// directly in the destination.
pub fn refuse_bad_snapshot_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.starts_with('.')
        || !name.starts_with(ARCHIVE_PREFIX)
        || !(name.ends_with(ARCHIVE_SUFFIX) || name.ends_with(ENCRYPTED_ARCHIVE_SUFFIX))
    {
        return Err(FactoryError::BadRequest(format!(
            "{name:?} is not a snapshot name; `factory backup list` shows them \
             ({ARCHIVE_PREFIX}<instance>-<utc>{ARCHIVE_SUFFIX} or {ENCRYPTED_ARCHIVE_SUFFIX})"
        )));
    }
    Ok(())
}

// ================================================================ manifest

/// `manifest.json`: what is in an archive, written into it after every file
/// it describes, so each file is read exactly once -- hashed and archived
/// from the same bytes, never re-read -- and an edit landing mid-backup can
/// never make the two disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub instance: ManifestInstance,
    /// `CARGO_PKG_VERSION` of the daemon that wrote it. There is no build
    /// commit compiled in, so none is claimed.
    pub daemon_version: String,
    pub created_at: DateTime<Utc>,
    pub database: DatabaseFacts,
    /// Every file in the archive but this manifest, sorted by path.
    pub files: Vec<ManifestFile>,
    /// What was found under an included directory and left out, and why.
    #[serde(default)]
    pub excluded: Vec<ExcludedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestInstance {
    pub id: String,
    pub name: String,
}

/// What the database copy said about itself when it was taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseFacts {
    pub path: String,
    /// `PRAGMA user_version`: the task store's schema version.
    pub user_version: i64,
    /// Every table in the copy, sorted -- the workflow, bench, policy and
    /// goals stores keep no version of their own, so their tables are the
    /// schema fact there is.
    pub tables: Vec<String>,
    /// `PRAGMA integrity_check` on the copy: `ok`, or the first problem.
    pub integrity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Relative to the instance root, `/`-separated.
    pub path: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
    pub group: Group,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedFile {
    pub path: String,
    pub reason: String,
}

/// A snapshot restored into a new instance root. Restore is deliberately a
/// CLI-only, owner-only operation; this is its socket response and `--json`
/// shape, not state kept by the running instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restoration {
    pub snapshot: String,
    pub into: String,
    pub files: u64,
    /// The same checks `backup verify` ran before the staged root was made
    /// visible. A restoration is returned only when none failed.
    pub checks: Vec<VerifyCheck>,
    pub duration_ms: u64,
}

impl Manifest {
    /// Files and bytes per group, for the include table.
    pub fn totals(&self, group: Group) -> (u64, u64) {
        self.files
            .iter()
            .filter(|f| f.group == group)
            .fold((0, 0), |(n, bytes), f| (n + 1, bytes + f.size))
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

// ================================================================ retention

/// Why a snapshot survives retention. A snapshot kept by no rule is deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeptBy {
    /// The newest snapshot is never deleted, whatever `keep` says.
    Newest,
    Daily,
    Weekly,
    Monthly,
}

/// Grandfather-father-son retention over snapshots sorted newest first, each
/// given as the calendar date it was taken on in the schedule's timezone.
/// Answers, in the same order, the rules that keep each one; an empty list
/// means delete it.
///
/// Each rule keeps the newest snapshot of each of its `n` most recent
/// buckets that *have* a snapshot -- days, ISO weeks, months -- so a week
/// the daemon was off costs no daily slot. The rules overlap freely: last
/// night's backup is usually the daily, the weekly and the monthly one.
pub fn retain(dates_newest_first: &[NaiveDate], keep: &Keep) -> Vec<Vec<KeptBy>> {
    let mut out: Vec<Vec<KeptBy>> = vec![Vec::new(); dates_newest_first.len()];
    if let Some(first) = out.first_mut() {
        first.push(KeptBy::Newest);
    }
    let mut apply = |rule: KeptBy, limit: u32, bucket: &dyn Fn(NaiveDate) -> (i32, u32)| {
        let mut seen = BTreeSet::new();
        for (i, date) in dates_newest_first.iter().enumerate() {
            if seen.len() as u32 >= limit {
                break;
            }
            // Newest first, so the first snapshot met in a bucket is its newest.
            if seen.insert(bucket(*date)) {
                out[i].push(rule);
            }
        }
    };
    apply(KeptBy::Daily, keep.daily, &|d| (d.year(), d.ordinal()));
    apply(KeptBy::Weekly, keep.weekly, &|d| {
        let w = d.iso_week();
        (w.year(), w.week())
    });
    apply(KeptBy::Monthly, keep.monthly, &|d| (d.year(), d.month()));
    out
}

// ================================================================ age

/// How the newest backup's age reads against its schedule: the colour of the
/// status hero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgeLevel {
    /// Within its schedule (plus [`GRACE_HOURS`]). Green.
    Fresh,
    /// One slot has passed without a newer backup. Amber.
    Stale,
    /// Two or more. Red.
    Overdue,
    /// There is no backup at all. Red.
    None,
}

/// `due_by` is when the newest backup stops being fresh -- the next slot
/// after it, plus grace -- and `overdue_by` the slot after that, plus grace.
/// The daemon computes both from the schedule; this only compares.
pub fn age_level(
    now: DateTime<Utc>,
    newest: Option<DateTime<Utc>>,
    due_by: Option<DateTime<Utc>>,
    overdue_by: Option<DateTime<Utc>>,
) -> AgeLevel {
    if newest.is_none() {
        return AgeLevel::None;
    }
    match (due_by, overdue_by) {
        (Some(due), _) if now <= due => AgeLevel::Fresh,
        (_, Some(overdue)) if now <= overdue => AgeLevel::Stale,
        _ => AgeLevel::Overdue,
    }
}

// ==================================================================== code
//
// `#155`: scope source code is backed up by pushing it to its git remote,
// never by this snapshot -- so the only honest thing L1 Backup can say about
// it is what `git` itself reports about the current branch, as of the last
// fetch. Nothing here fetches, pushes or configures a remote.

/// The current branch's state in a registered scope's repository, as
/// `git` itself answers it -- never a guess, and never "backed up" claimed
/// from anything but an upstream actually being ahead of nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RepositoryState {
    /// The scope's directory does not exist.
    NoDirectory,
    /// The directory exists but `git` does not recognise it as a repository.
    NotARepository,
    /// A repository with no commits yet (an unborn `HEAD`).
    NoCommits,
    /// `HEAD` does not point at a branch.
    DetachedHead,
    /// A repository with commits, but no remote configured at all.
    NoRemote,
    /// The current branch has no upstream, though at least one remote
    /// exists -- `git push -u` has never run for it.
    NoUpstream,
    /// The current branch tracks `upstream` on `remote`, `ahead` commits
    /// not on it. Never re-derived after a fetch this probe never runs:
    /// the count is against the local remote-tracking ref as it already
    /// stood.
    Tracked { remote: String, upstream: String, ahead: u64 },
    /// A `git` probe failed or timed out; this row's state could not be
    /// read. Never treated as a problem or as fine -- only as unknown.
    InspectionFailed { reason: String },
}

/// One registered scope's repository, or the reason it has none. Two scopes
/// whose directories are the same repository (a nested scope sharing the
/// root's checkout, say) are probed once and share this same fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryFact {
    /// Every registered scope name whose directory resolves to this
    /// repository, in config order.
    pub scopes: Vec<String>,
    /// The repository's top level, relative to the instance root (or the
    /// scope's own directory when there is no repository to give one).
    pub path: String,
    pub state: RepositoryState,
    /// The tracked remote's URL with [`redact_remote`] applied. `None`
    /// unless `state` is `Tracked`: an untracked or absent remote has
    /// nothing this probe would know to ask `git` for.
    pub remote_url: Option<String>,
}

/// One macOS Time Machine destination, from `tmutil destinationinfo`. See
/// [`parse_destinationinfo`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TimeMachineFact {
    /// At least one destination is configured, named as `tmutil` names it.
    Configured { destinations: Vec<String> },
    /// `tmutil` ran and plainly said none is configured.
    NotConfigured,
    /// `tmutil` could not be asked: missing, timed out, or an exit this
    /// daemon does not otherwise recognise. Shown as unknown, never as a
    /// problem or as fine.
    Unavailable { reason: String },
    /// Not macOS: Time Machine does not apply to this host.
    Unsupported,
}

/// Strip `user:password@` / `token@` userinfo from an `http(s)` remote URL
/// before it is ever put on the wire -- a personal-access-token remote must
/// never reach `GET /api/backup`. `git@host:owner/repo.git` (the scp-like
/// form) and `ssh://` URLs carry no userinfo worth redacting and are
/// returned unchanged; so is anything that is not `http(s)` at all (a local
/// path, for one).
pub fn redact_remote(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else { return url.to_string() };
    let scheme = &url[..scheme_end + 3];
    if !scheme.eq_ignore_ascii_case("http://") && !scheme.eq_ignore_ascii_case("https://") {
        return url.to_string();
    }
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    match authority.rfind('@') {
        Some(at) => format!("{scheme}{}{}", &authority[at + 1..], &rest[authority_end..]),
        None => url.to_string(),
    }
}

/// `tmutil destinationinfo`'s stdout (or, on a non-zero exit, whatever text
/// there is to explain it) read into a fact. Pure: the daemon is the one
/// that actually runs `tmutil`, under a timeout, and hands this whatever it
/// got back.
pub fn parse_destinationinfo(output: &str, success: bool) -> TimeMachineFact {
    if !success {
        let reason = output
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("tmutil destinationinfo exited with an error")
            .to_string();
        return TimeMachineFact::Unavailable { reason };
    }
    if output.to_ascii_lowercase().contains("no destinations configured") {
        return TimeMachineFact::NotConfigured;
    }
    let destinations: Vec<String> = output
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.trim().eq_ignore_ascii_case("name"))
        .map(|(_, value)| value.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    if destinations.is_empty() {
        TimeMachineFact::Unavailable {
            reason: format!("could not read tmutil destinationinfo's output: {output:?}"),
        }
    } else {
        TimeMachineFact::Configured { destinations }
    }
}

// =========================================================== the L1 fact

/// A verification passing counts for [`resolve_backup_fact`]'s `verified`
/// only within this many days of it -- an old pass proves less each day the
/// archive could have silently rotted since.
pub const VERIFIED_WITHIN_DAYS: i64 = 30;

/// The one L1 fact a policy `daemon` check or a registry metric reads about
/// backups (`#154`) -- computed by one `Engine::backup_fact(now)` in
/// `factory-daemon`'s `backup` module, off the same captured state
/// `backup_report` reads, via [`resolve_backup_fact`]. Plain data: nothing
/// here reads a clock, a store or a disk. `#155`'s repository and Time
/// Machine probes are deliberately not part of this fact -- see the
/// module's own header.
///
/// Moved to the L0 kernel (#193, phase 2: no field's type is owned by
/// another level's module) and re-exported here unchanged, along with
/// [`VerifySummary`], so nothing that builds or reads a `BackupFact`
/// changes -- see `factory_kernel::facts`'s own doc comment.
pub use factory_kernel::BackupFact;

/// [`BackupFact`]'s whole derivation, pure so every row of the triage's
/// semantics table (issue `#154`) is a plain unit test with no filesystem,
/// store or clock of its own. The caller does the I/O and hands in
/// primitives it already has: `age` is its own [`age_level`] against
/// `newest` (`AgeLevel::None` when there is none, from `deadlines`/
/// `age_level` exactly as `backup_report` computes them), and
/// `last_verified` is already filtered to a snapshot still present in the
/// destination -- the same "verifications of archives still present" rule
/// [`warnings`] applies.
///
/// Order matters: not configured beats everything (all three `Some(false)`
/// -- there is no backup to be indeterminate about); an unreachable
/// destination beats a fresh reading (all three `None` -- not even "no" is
/// honest about an archive nobody can currently list); only then do
/// `recent`/`offsite`/`verified` read the age, the device and the newest
/// still-present verification, each independently of whether a snapshot
/// exists at all.
pub fn resolve_backup_fact(
    now: DateTime<Utc>,
    configured: bool,
    destination_exists: bool,
    same_device: Option<bool>,
    newest: Option<DateTime<Utc>>,
    age: AgeLevel,
    last_verified: Option<VerifySummary>,
) -> BackupFact {
    if !configured {
        return BackupFact {
            at: now,
            configured: false,
            newest: None,
            recent: Some(false),
            offsite: Some(false),
            verified: Some(false),
            last_verified: None,
        };
    }
    if !destination_exists {
        return BackupFact {
            at: now,
            configured: true,
            newest: None,
            recent: None,
            offsite: None,
            verified: None,
            last_verified: None,
        };
    }
    let recent = Some(matches!(age, AgeLevel::Fresh));
    let offsite = same_device.map(|same| !same);
    let verified = Some(last_verified.as_ref().is_some_and(|v| {
        v.ok && now.signed_duration_since(v.at) <= Duration::days(VERIFIED_WITHIN_DAYS)
    }));
    BackupFact { at: now, configured: true, newest, recent, offsite, verified, last_verified }
}

// ================================================================ warnings

/// How loudly a warning is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningLevel {
    Warn,
    Bad,
}

/// One honest warning: a fact the daemon read, never a guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupWarning {
    /// A stable id the page and a test can match on: `not_configured`,
    /// `same_device`, ...
    pub kind: String,
    pub level: WarningLevel,
    pub message: String,
}

/// What [`warnings`] is decided from.
#[derive(Debug, Clone, Default)]
pub struct WarningFacts {
    pub configured: bool,
    pub destination: Option<String>,
    pub destination_exists: bool,
    pub same_device: Option<bool>,
    pub scheduled: bool,
    pub newest: Option<DateTime<Utc>>,
    pub age: Option<AgeLevel>,
    /// The newest verification of a snapshot still in the destination, and
    /// whether it passed. A pass on a snapshot retention has since deleted
    /// proves nothing about the ones left.
    pub last_verified: Option<(DateTime<Utc>, bool)>,
    /// The newest failed attempt, when it is newer than the newest success.
    pub failure_since_newest: Option<(DateTime<Utc>, String)>,
    /// Whether the newest snapshot is encrypted (`#152`) -- changes
    /// `never_verified`'s message to name the `--identity` flag verifying
    /// it needs, since a plain "run Verify" would fail without one.
    pub newest_encrypted: bool,
    /// `#155`: every registered scope's repository fact, gathered whether or
    /// not a backup is configured at all -- source code is backed up by
    /// pushing it, not by this report's `configured`.
    pub code: Vec<RepositoryFact>,
    /// `#155`: `None` when it was never asked (not yet wired up); distinct
    /// from [`TimeMachineFact::Unavailable`], which is "asked and failed".
    pub time_machine: Option<TimeMachineFact>,
}

pub fn warnings(f: &WarningFacts) -> Vec<BackupWarning> {
    let mut out = Vec::new();
    let mut push = |kind: &str, level: WarningLevel, message: String| {
        out.push(BackupWarning { kind: kind.into(), level, message });
    };
    if !f.configured {
        push(
            "not_configured",
            WarningLevel::Bad,
            "No backup is configured: the database, the knowledge vault, the policies, goals and scenarios \
             exist on this disk only. Add infrastructure.backup to the root .factory/config.yaml."
                .into(),
        );
        // Source code is backed up by pushing it, not by this snapshot, so
        // the code and Time Machine warnings below still apply -- and
        // still follow `not_configured`, never before it.
    } else {
        let destination = f.destination.clone().unwrap_or_default();
        if let Some((at, reason)) = &f.failure_since_newest {
            push(
                "last_failed",
                WarningLevel::Bad,
                format!("The last backup attempt failed ({}): {reason}", at.format("%Y-%m-%d %H:%M UTC")),
            );
        }
        if !f.destination_exists {
            push(
                "destination_missing",
                WarningLevel::Bad,
                format!("The destination {destination} does not exist or is not mounted."),
            );
        }
        if f.same_device == Some(true) {
            push(
                "same_device",
                WarningLevel::Bad,
                format!(
                    "The destination {destination} is on the same device as the instance, so a disk failure \
                     loses both: this is a copy, not a backup. Point it at an external disk, a NAS mount or a \
                     synced folder."
                ),
            );
        }
        match (f.newest, f.age) {
            (None, _) => push("no_backup", WarningLevel::Bad, "No backup has been taken yet.".into()),
            (Some(_), Some(AgeLevel::Stale)) => push(
                "stale",
                WarningLevel::Warn,
                "The newest backup is older than its schedule: at least one slot passed without a backup.".into(),
            ),
            (Some(_), Some(AgeLevel::Overdue)) => push(
                "overdue",
                WarningLevel::Bad,
                "The newest backup is overdue: more than one scheduled backup has been missed.".into(),
            ),
            _ => {}
        }
        if !f.scheduled {
            push(
                "unscheduled",
                WarningLevel::Warn,
                "No schedule is configured: a backup is taken only when somebody runs one. Add \
                 infrastructure.backup.schedule."
                    .into(),
            );
        }
        match f.last_verified {
            None if f.newest.is_some() && f.newest_encrypted => push(
                "never_verified",
                WarningLevel::Warn,
                "No snapshot in the destination has been verified, so nobody knows whether one would restore. \
                 The newest is encrypted: run `factory backup verify --identity <file>`."
                    .into(),
            ),
            None if f.newest.is_some() => push(
                "never_verified",
                WarningLevel::Warn,
                "No snapshot in the destination has been verified, so nobody knows whether one would restore. Run Verify.".into(),
            ),
            Some((at, false)) => push(
                "verify_failed",
                WarningLevel::Bad,
                format!("The last verification ({}) failed.", at.format("%Y-%m-%d %H:%M UTC")),
            ),
            _ => {}
        }
    }
    // `#155`: honest, not exhaustive -- `inspection_failed` states are
    // unknown, never asserted good or bad, so only `Tracked` (with commits
    // to push), `NoRemote` and `NoUpstream` add a warning.
    for repo in &f.code {
        let label = if repo.scopes.is_empty() { "a scope".to_string() } else { repo.scopes.join(", ") };
        match &repo.state {
            RepositoryState::Tracked { ahead, .. } if *ahead > 0 => push(
                "code_unpushed",
                WarningLevel::Warn,
                format!(
                    "{label} has {ahead} commit{} not on its upstream, as of the last fetch ({}).",
                    if *ahead == 1 { "" } else { "s" },
                    repo.path
                ),
            ),
            RepositoryState::NoRemote => push(
                "code_no_remote",
                WarningLevel::Warn,
                format!("{label} has no git remote configured ({}): its commits exist only on this disk.", repo.path),
            ),
            RepositoryState::NoUpstream => push(
                "code_no_upstream",
                WarningLevel::Warn,
                format!("{label}'s current branch has no upstream ({}): `git push -u` has never run.", repo.path),
            ),
            _ => {}
        }
    }
    if matches!(f.time_machine, Some(TimeMachineFact::NotConfigured)) {
        push(
            "time_machine_off",
            WarningLevel::Warn,
            "Time Machine is not configured on this Mac: nothing outside a pushed git remote or this backup \
             has a second copy anywhere."
                .into(),
        );
    }
    out
}

// ================================================================ the wire

/// `GET /api/backup`: everything the L1 › Backup page draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupReport {
    /// The daemon's clock, so the page reads every age against the same
    /// "now" the levels were decided with.
    pub now: DateTime<Utc>,
    /// The block as configured; `None` when there is none.
    pub config: Option<BackupConfig>,
    pub destination: Option<DestinationFacts>,
    pub age: AgeLevel,
    /// When the newest backup stops being fresh.
    pub due_by: Option<DateTime<Utc>>,
    /// When the schedule next takes one.
    pub next_run: Option<DateTime<Utc>>,
    /// `#156`: when the verification drill's own schedule next runs one --
    /// `None` when no `verify_schedule` is configured (or none fires,
    /// which `validate_config` refuses at start, so only shows up here for
    /// a config edited on disk without a restart). `#[serde(default)]` so a
    /// daemon built before this exists still parses to a CLI built after.
    #[serde(default)]
    pub next_verify: Option<DateTime<Utc>>,
    /// `#156`: why a drill attempted right now would skip -- the newest
    /// snapshot is encrypted, which the drill can never supply an identity
    /// for. `None` when nothing would stop it, including when no drill is
    /// configured at all. `#[serde(default)]` for the same reason as
    /// `next_verify`.
    #[serde(default)]
    pub verify_skipped: Option<String>,
    /// A backup, verification or restore is in progress right now.
    pub running: bool,
    pub last_verified: Option<VerifySummary>,
    pub last_failure: Option<BackupFailure>,
    pub warnings: Vec<BackupWarning>,
    /// Newest first: every archive of this instance in the destination.
    pub snapshots: Vec<SnapshotRow>,
    pub include: Vec<IncludeRow>,
    pub exclude: Vec<ExcludeRow>,
    /// `#155`: every registered scope's repository fact, filled whether or
    /// not a backup is configured. `#[serde(default)]` so a daemon built
    /// before this exists still parses to a CLI built after.
    #[serde(default)]
    pub code: Vec<RepositoryFact>,
    /// `#155`: `None` for a daemon built before this, or one that has not
    /// asked yet -- distinct from `Some(Unavailable { .. })`, which asked
    /// and failed.
    #[serde(default)]
    pub time_machine: Option<TimeMachineFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationFacts {
    pub path: String,
    pub exists: bool,
    /// Same `st_dev` as the instance root. `None` when it cannot be asked.
    pub same_device: Option<bool>,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRow {
    pub name: String,
    pub at: DateTime<Utc>,
    pub size_bytes: u64,
    /// Files in the archive, the manifest aside. `None` for an archive this
    /// daemon has no record of taking (restored database, copied in).
    pub files: Option<u64>,
    pub verified: Option<VerifySummary>,
    pub kept_by: Vec<KeptBy>,
    /// Read from the archive's own bytes (`#152`'s [`looks_encrypted`]),
    /// never from its name's suffix or the live config -- so a renamed file
    /// or a config changed since it was written can never be misreported.
    pub encrypted: bool,
}

/// One verification of a snapshot still present in a backup destination --
/// carried by [`BackupFact::last_verified`] whatever
/// [`BackupFact::verified`] itself decided, so a reader can say why. Moved
/// to the L0 kernel beside [`BackupFact`] (#193, phase 2) and re-exported
/// here unchanged.
pub use factory_kernel::VerifySummary;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupFailure {
    pub at: DateTime<Utc>,
    pub trigger: BackupTrigger,
    pub reason: String,
}

/// Who started a backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupTrigger {
    Schedule,
    Manual,
}

impl BackupTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncludeRow {
    pub path: String,
    pub why: String,
    /// False for the optional directories while `include_logs` is off.
    pub included: bool,
    /// From the newest snapshot's manifest, when this daemon took it.
    pub files: Option<u64>,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludeRow {
    pub path: String,
    pub why: String,
}

/// A backup as taken: the answer to `backup.run`, and what `backup_completed`
/// carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: String,
    pub path: String,
    pub at: DateTime<Utc>,
    pub trigger: BackupTrigger,
    pub by: String,
    pub size_bytes: u64,
    pub files: u64,
    pub database_bytes: u64,
    pub duration_ms: u64,
    /// What retention deleted after it, by name.
    #[serde(default)]
    pub pruned: Vec<String>,
    /// Files and bytes per group, from its manifest -- the include table's
    /// numbers, kept so the page never has to open an archive to draw them.
    #[serde(default)]
    pub groups: Vec<GroupTotal>,
    /// The recipient this snapshot was encrypted to (`#152`), `None` for a
    /// plaintext one. `#[serde(default)]` so a daemon built before this
    /// exists still parses to a CLI built after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_to: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupTotal {
    pub group: Group,
    pub files: u64,
    pub bytes: u64,
}

/// One step of a verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    /// Loaded, but with a finding worth reading -- a file the live instance
    /// most likely cannot parse either. Does not fail the verification.
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

/// The answer to `backup.verify`: every step, in the order it ran.
/// `ok` is "no step failed".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub snapshot: String,
    pub at: DateTime<Utc>,
    pub by: String,
    pub ok: bool,
    pub checks: Vec<VerifyCheck>,
    pub duration_ms: u64,
}

impl Verification {
    pub fn summary(&self) -> VerifySummary {
        VerifySummary { snapshot: self.snapshot.clone(), at: self.at, ok: self.ok }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn the_issue_example_block_parses_with_no_encrypt_to() {
        let c: BackupConfig = serde_yaml_ng::from_str(
            "destination: /Volumes/Backup/factory\nschedule: { cron: \"0 3 * * *\", timezone: Europe/Berlin }\n\
             keep: { daily: 7, weekly: 4, monthly: 6 }\ninclude_logs: false\n",
        )
        .unwrap();
        c.validate().unwrap();
        assert_eq!(c.schedule.as_ref().unwrap().describe(), "0 3 * * * (Europe/Berlin)");
        assert_eq!(c.keep, Keep::default());
        assert_eq!(c.encrypt_to, None);
    }

    #[test]
    fn encrypt_to_parses_and_round_trips_but_is_not_checked_here() {
        // `#152`: this crate stays dependency-free, so it accepts any
        // non-empty string here -- `factory_daemon::backup::validate_config`
        // is where an SSH or plugin recipient is actually refused, with the
        // `age` crate.
        let c: BackupConfig =
            serde_yaml_ng::from_str("destination: /x\nencrypt_to: age1not-a-real-recipient-just-a-string-here\n")
                .unwrap();
        c.validate().unwrap();
        assert_eq!(c.encrypt_to.as_deref(), Some("age1not-a-real-recipient-just-a-string-here"));
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("encrypt_to"), "{json}");
        assert_eq!(serde_json::from_str::<BackupConfig>(&json).unwrap(), c);

        let empty = serde_yaml_ng::from_str::<BackupConfig>("destination: /x\nencrypt_to: \"\"\n").unwrap();
        assert!(empty.validate().unwrap_err().to_string().contains("encrypt_to"));

        let none: BackupConfig = serde_yaml_ng::from_str("destination: /x\n").unwrap();
        assert_eq!(none.encrypt_to, None);
        assert!(!serde_json::to_string(&none).unwrap().contains("encrypt_to"), "omitted, not null, when unset");
    }

    /// `#156`: `verify_schedule` is optional, the same shape as `schedule`,
    /// round-trips, and is validated (empty cron refused) the same way --
    /// the cron expression itself is only parsed for real at daemon load
    /// (`factory_daemon::backup::validate_config`), like `schedule`'s.
    #[test]
    fn verify_schedule_is_optional_like_schedule_and_validated_the_same_way() {
        let none: BackupConfig = serde_yaml_ng::from_str("destination: /b\n").unwrap();
        assert_eq!(none.verify_schedule, None);
        assert!(!serde_json::to_string(&none).unwrap().contains("verify_schedule"), "omitted, not null, when unset");

        let c: BackupConfig =
            serde_yaml_ng::from_str("destination: /b\nverify_schedule: { cron: \"*/2 * * * *\" }\n").unwrap();
        c.validate().unwrap();
        assert_eq!(c.verify_schedule.as_ref().unwrap().describe(), "*/2 * * * * (UTC)");
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("verify_schedule"), "{json}");
        assert_eq!(serde_json::from_str::<BackupConfig>(&json).unwrap(), c);

        let empty_cron: BackupConfig =
            serde_yaml_ng::from_str("destination: /b\nverify_schedule: { cron: \"\" }\n").unwrap();
        assert!(empty_cron.validate().unwrap_err().to_string().contains("verify_schedule"));
    }

    /// `#156`: a `BackupReport` serialized before `next_verify`/
    /// `verify_skipped` existed still deserializes -- `#[serde(default)]`,
    /// the same guarantee `code`/`time_machine` (`#155`) already rely on.
    #[test]
    fn a_report_from_before_156_still_deserializes_with_no_drill_scheduled() {
        let json = r#"{
            "now": "2026-01-01T00:00:00Z", "config": null, "destination": null, "age": "none",
            "due_by": null, "next_run": null, "running": false, "last_verified": null,
            "last_failure": null, "warnings": [], "snapshots": [], "include": [], "exclude": []
        }"#;
        let report: BackupReport = serde_json::from_str(json).unwrap();
        assert_eq!(report.next_verify, None);
        assert_eq!(report.verify_skipped, None);
    }

    #[test]
    fn a_relative_destination_and_a_keep_that_keeps_nothing_are_refused() {
        let relative: BackupConfig = serde_yaml_ng::from_str("destination: backups\n").unwrap();
        assert!(relative.validate().unwrap_err().to_string().contains("absolute"));
        let nothing: BackupConfig =
            serde_yaml_ng::from_str("destination: /b\nkeep: { daily: 0, weekly: 0, monthly: 0 }\n").unwrap();
        assert!(nothing.validate().unwrap_err().to_string().contains("keeps nothing"));
        let partial: BackupConfig = serde_yaml_ng::from_str("destination: /b\nkeep: { daily: 2 }\n").unwrap();
        assert_eq!(partial.keep, Keep { daily: 2, weekly: 4, monthly: 6 });
    }

    #[test]
    fn archive_names_round_trip_and_ignore_everything_else() {
        let at = Utc.with_ymd_and_hms(2026, 9, 25, 3, 0, 7).unwrap();
        let name = archive_name("Business Factory", at, false);
        assert_eq!(name, "factory-backup-business-factory-20260925T030007Z.tar.zst");
        assert_eq!(parse_archive_name("Business Factory", &name), Some(at));
        assert_eq!(parse_archive_name("other", &name), None, "another instance's archive is not ours");
        assert_eq!(parse_archive_name("business-factory", "factory-backup-business-factory-x.tar.zst"), None);
        assert_eq!(parse_archive_name("business-factory", "notes.txt"), None);
        assert_eq!(
            parse_archive_name("business-factory", "factory-backup-business-factory-20260925T030007Z.tar.zst.partial"),
            None,
            "a half-written archive is never listed"
        );
    }

    #[test]
    fn an_encrypted_archive_name_round_trips_alongside_a_plaintext_one() {
        let at = Utc.with_ymd_and_hms(2026, 9, 25, 3, 0, 7).unwrap();
        let name = archive_name("Business Factory", at, true);
        assert_eq!(name, "factory-backup-business-factory-20260925T030007Z.tar.zst.age");
        assert_eq!(parse_archive_name("Business Factory", &name), Some(at));
        assert_eq!(
            parse_archive_name("business-factory", "factory-backup-business-factory-20260925T030007Z.tar.zst.age.partial"),
            None,
            "a half-written encrypted archive is never listed either"
        );
        // The two suffixes never bleed into each other.
        assert_ne!(archive_name("x", at, false), archive_name("x", at, true));
    }

    #[test]
    fn a_snapshot_name_off_the_wire_cannot_leave_the_destination() {
        refuse_bad_snapshot_name("factory-backup-x-20260925T030000Z.tar.zst").unwrap();
        refuse_bad_snapshot_name("factory-backup-x-20260925T030000Z.tar.zst.age").unwrap();
        for bad in [
            "",
            "../factory-backup-x.tar.zst",
            "factory-backup-x/../../etc.tar.zst",
            "notes.txt",
            ".factory-backup-x.tar.zst",
            "factory-backup-x-20260925T030000Z.tar.zst.age.partial",
        ] {
            assert!(refuse_bad_snapshot_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn looks_encrypted_reads_the_age_magic_and_nothing_else() {
        assert!(looks_encrypted(b"age-encryption.org/v1\n-> X25519 ..."));
        assert!(!looks_encrypted(b"\x28\xb5\x2f\xfd"), "a zstd frame is not age");
        assert!(!looks_encrypted(b""));
        assert!(!looks_encrypted(b"age-encryption.org/v"), "too short to be the whole magic");
    }

    #[test]
    fn secrets_and_transient_files_are_excluded_before_they_are_read() {
        assert!(is_excluded(".factory/secrets.yaml").is_some());
        assert!(is_excluded(".factory/knowledge/data/secrets/token.txt").is_some());
        assert!(is_excluded(".factory/knowledge/.env").is_some());
        assert!(is_excluded(".factory/factory.sqlite-wal").is_some());
        assert!(is_excluded(".factory/knowledge/company/README.md").is_none());
        assert!(is_excluded(".factory/policies/secrets-policy.yaml").is_none(), "only the exact names");
    }

    #[test]
    fn retention_keeps_the_newest_of_each_day_week_and_month_up_to_the_limits() {
        // Newest first: two on the 25th, one a day back to the 20th, then
        // the end of August, then July.
        let dates = [
            "2026-09-25", "2026-09-25", "2026-09-24", "2026-09-23", "2026-09-22", "2026-09-21", "2026-09-20",
            "2026-08-31", "2026-07-15",
        ]
        .map(d);
        let kept = retain(&dates, &Keep { daily: 3, weekly: 2, monthly: 2 });
        assert_eq!(kept[0], vec![KeptBy::Newest, KeptBy::Daily, KeptBy::Weekly, KeptBy::Monthly]);
        assert!(kept[1].is_empty(), "the older one on the same day goes: {:?}", kept[1]);
        assert_eq!(kept[2], vec![KeptBy::Daily]);
        assert_eq!(kept[3], vec![KeptBy::Daily]);
        assert!(kept[4].is_empty(), "past the three days");
        // 2026-09-20 is a Sunday: the newest of the ISO week before.
        assert_eq!(kept[6], vec![KeptBy::Weekly]);
        assert_eq!(kept[7], vec![KeptBy::Monthly], "the newest of August");
        assert!(kept[8].is_empty(), "past the two months");
    }

    #[test]
    fn retention_over_nothing_or_one_keeps_what_there_is() {
        assert!(retain(&[], &Keep::default()).is_empty());
        let one = retain(&[d("2026-01-01")], &Keep { daily: 0, weekly: 0, monthly: 1 });
        assert_eq!(one, vec![vec![KeptBy::Newest, KeptBy::Monthly]]);
    }

    #[test]
    fn the_age_level_reads_the_newest_backup_against_its_schedule() {
        let t = |h| Utc.with_ymd_and_hms(2026, 9, 25, h, 0, 0).unwrap();
        let (due, overdue) = (Some(t(5)), Some(t(20)));
        assert_eq!(age_level(t(4), Some(t(1)), due, overdue), AgeLevel::Fresh);
        assert_eq!(age_level(t(6), Some(t(1)), due, overdue), AgeLevel::Stale);
        assert_eq!(age_level(t(21), Some(t(1)), due, overdue), AgeLevel::Overdue);
        assert_eq!(age_level(t(4), None, due, overdue), AgeLevel::None);
    }

    #[test]
    fn warnings_say_what_is_true_and_nothing_else() {
        let none = warnings(&WarningFacts::default());
        assert_eq!(none.iter().map(|w| w.kind.as_str()).collect::<Vec<_>>(), ["not_configured"]);

        let fine = WarningFacts {
            configured: true,
            destination: Some("/Volumes/Backup".into()),
            destination_exists: true,
            same_device: Some(false),
            scheduled: true,
            newest: Some(Utc::now()),
            age: Some(AgeLevel::Fresh),
            last_verified: Some((Utc::now(), true)),
            failure_since_newest: None,
            newest_encrypted: false,
            code: Vec::new(),
            time_machine: None,
        };
        assert!(warnings(&fine).is_empty(), "{:?}", warnings(&fine));

        let worrying = WarningFacts {
            same_device: Some(true),
            scheduled: false,
            age: Some(AgeLevel::Stale),
            last_verified: None,
            failure_since_newest: Some((Utc::now(), "disk full".into())),
            ..fine.clone()
        };
        let kinds: Vec<String> = warnings(&worrying).into_iter().map(|w| w.kind).collect();
        assert_eq!(kinds, ["last_failed", "same_device", "stale", "unscheduled", "never_verified"]);

        let unknown_device = WarningFacts { same_device: None, ..fine.clone() };
        assert!(warnings(&unknown_device).is_empty(), "an unknown device is not claimed to be the same one");

        let never = WarningFacts { newest: None, age: Some(AgeLevel::None), last_verified: None, ..fine };
        let kinds: Vec<String> = warnings(&never).into_iter().map(|w| w.kind).collect();
        assert_eq!(kinds, ["no_backup"], "never_verified says nothing new when there is nothing to verify");
    }

    /// `#152`: an unverified encrypted newest snapshot names the `--identity`
    /// flag, so a person is not told to run a Verify that will just be
    /// refused.
    #[test]
    fn never_verified_names_the_identity_flag_when_the_newest_is_encrypted() {
        let facts = WarningFacts {
            configured: true,
            destination: Some("/Volumes/Backup".into()),
            destination_exists: true,
            same_device: Some(false),
            scheduled: true,
            newest: Some(Utc::now()),
            age: Some(AgeLevel::Fresh),
            last_verified: None,
            failure_since_newest: None,
            newest_encrypted: true,
            code: Vec::new(),
            time_machine: None,
        };
        let found = warnings(&facts);
        assert_eq!(found.iter().map(|w| w.kind.as_str()).collect::<Vec<_>>(), ["never_verified"]);
        assert!(found[0].message.contains("--identity"), "{}", found[0].message);
    }

    fn tracked(scopes: &[&str], path: &str, remote: &str, upstream: &str, ahead: u64) -> RepositoryFact {
        RepositoryFact {
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            path: path.into(),
            state: RepositoryState::Tracked { remote: remote.into(), upstream: upstream.into(), ahead },
            remote_url: Some(format!("https://github.com/o/{path}.git")),
        }
    }

    #[test]
    fn code_and_time_machine_warnings_follow_not_configured_and_add_nothing_for_the_unknown_states() {
        // Even with no backup configured at all, the code and Time Machine
        // facts are still gathered and still warn -- but `not_configured`
        // stays first.
        let unconfigured = WarningFacts {
            code: vec![tracked(&["factory"], "projects/factory", "origin", "origin/main", 2)],
            time_machine: Some(TimeMachineFact::NotConfigured),
            ..WarningFacts::default()
        };
        let kinds: Vec<String> = warnings(&unconfigured).into_iter().map(|w| w.kind).collect();
        assert_eq!(kinds, ["not_configured", "code_unpushed", "time_machine_off"]);

        let base = WarningFacts {
            configured: true,
            destination: Some("/Volumes/Backup".into()),
            destination_exists: true,
            same_device: Some(false),
            scheduled: true,
            newest: Some(Utc::now()),
            age: Some(AgeLevel::Fresh),
            last_verified: Some((Utc::now(), true)),
            failure_since_newest: None,
            newest_encrypted: false,
            code: Vec::new(),
            time_machine: None,
        };

        let ahead = WarningFacts {
            code: vec![tracked(&["factory"], "projects/factory", "origin", "origin/main", 1)],
            ..base.clone()
        };
        let kinds: Vec<String> = warnings(&ahead).into_iter().map(|w| w.kind).collect();
        assert_eq!(kinds, ["code_unpushed"]);

        let up_to_date = WarningFacts {
            code: vec![tracked(&["factory"], "projects/factory", "origin", "origin/main", 0)],
            ..base.clone()
        };
        assert!(warnings(&up_to_date).is_empty(), "ahead == 0 is nothing to warn about");

        let no_remote = WarningFacts {
            code: vec![RepositoryFact {
                scopes: vec!["factory".into()],
                path: "projects/factory".into(),
                state: RepositoryState::NoRemote,
                remote_url: None,
            }],
            ..base.clone()
        };
        assert_eq!(warnings(&no_remote).into_iter().map(|w| w.kind).collect::<Vec<_>>(), ["code_no_remote"]);

        let no_upstream = WarningFacts {
            code: vec![RepositoryFact {
                scopes: vec!["factory".into()],
                path: "projects/factory".into(),
                state: RepositoryState::NoUpstream,
                remote_url: None,
            }],
            ..base.clone()
        };
        assert_eq!(warnings(&no_upstream).into_iter().map(|w| w.kind).collect::<Vec<_>>(), ["code_no_upstream"]);

        // Unknown, failed or unsupported states are never a problem or fine.
        for state in [
            RepositoryState::NoDirectory,
            RepositoryState::NotARepository,
            RepositoryState::NoCommits,
            RepositoryState::DetachedHead,
            RepositoryState::InspectionFailed { reason: "git timed out".into() },
        ] {
            let facts = WarningFacts {
                code: vec![RepositoryFact { scopes: vec!["factory".into()], path: "projects/factory".into(), state, remote_url: None }],
                ..base.clone()
            };
            assert!(warnings(&facts).is_empty(), "{:?}", warnings(&facts));
        }
        for tm in [TimeMachineFact::Unavailable { reason: "missing".into() }, TimeMachineFact::Unsupported] {
            let facts = WarningFacts { time_machine: Some(tm), ..base.clone() };
            assert!(warnings(&facts).is_empty(), "{:?}", warnings(&facts));
        }
        let configured_tm = WarningFacts {
            time_machine: Some(TimeMachineFact::Configured { destinations: vec!["Backup Disk".into()] }),
            ..base
        };
        assert!(warnings(&configured_tm).is_empty());
    }

    #[test]
    fn redact_remote_strips_userinfo_from_http_urls_only() {
        assert_eq!(
            redact_remote("https://x-access-token:SECRET@github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(redact_remote("https://SECRET@host/r"), "https://host/r");
        assert_eq!(redact_remote("http://user:pass@host:8080/path"), "http://host:8080/path");
        // Never touched: no userinfo to strip in the first place.
        assert_eq!(redact_remote("git@github.com:o/r.git"), "git@github.com:o/r.git", "scp-like: not http(s)");
        assert_eq!(redact_remote("ssh://git@host/o/r.git"), "ssh://git@host/o/r.git", "ssh: not http(s)");
        assert_eq!(redact_remote("https://github.com/o/r.git"), "https://github.com/o/r.git", "nothing to redact");
        assert_eq!(redact_remote("/Volumes/Backup/bare.git"), "/Volumes/Backup/bare.git", "a local path");
    }

    // -- resolve_backup_fact (#154): one test per semantics-table row -----

    fn passed(at: DateTime<Utc>) -> VerifySummary {
        VerifySummary { snapshot: "s".into(), at, ok: true }
    }
    fn failed(at: DateTime<Utc>) -> VerifySummary {
        VerifySummary { snapshot: "s".into(), at, ok: false }
    }

    #[test]
    fn not_configured_reads_false_on_every_fact_never_indeterminate() {
        let fact = resolve_backup_fact(Utc::now(), false, false, None, None, AgeLevel::None, None);
        assert!(!fact.configured);
        assert_eq!(fact.newest, None);
        assert_eq!((fact.recent, fact.offsite, fact.verified), (Some(false), Some(false), Some(false)));
        assert_eq!(fact.last_verified, None);
    }

    #[test]
    fn a_missing_or_unmounted_destination_reads_indeterminate_on_every_fact() {
        let now = Utc::now();
        // Even a same_device or last_verified the caller somehow still had
        // is disregarded: an archive nobody can currently list is not
        // "still there" to be true or false about.
        let fact = resolve_backup_fact(now, true, false, Some(true), Some(now), AgeLevel::Fresh, Some(passed(now)));
        assert_eq!(fact.newest, None);
        assert_eq!((fact.recent, fact.offsite, fact.verified), (None, None, None));
        assert_eq!(fact.last_verified, None);
    }

    #[test]
    fn configured_with_no_snapshot_yet_is_not_recent_or_verified_but_offsite_still_reads_the_device() {
        let now = Utc::now();
        let fact = resolve_backup_fact(now, true, true, Some(false), None, AgeLevel::None, None);
        assert_eq!(fact.recent, Some(false));
        assert_eq!(fact.verified, Some(false));
        assert_eq!(fact.offsite, Some(true), "not the same device as the instance root");
    }

    #[test]
    fn same_device_reads_offsite_false_whatever_recent_and_verified_say() {
        let now = Utc::now();
        let fact = resolve_backup_fact(now, true, true, Some(true), Some(now), AgeLevel::Fresh, Some(passed(now)));
        assert_eq!(fact.offsite, Some(false));
    }

    #[test]
    fn an_undeterminable_device_reads_offsite_indeterminate() {
        let now = Utc::now();
        let fact = resolve_backup_fact(now, true, true, None, Some(now), AgeLevel::Fresh, Some(passed(now)));
        assert_eq!(fact.offsite, None);
    }

    #[test]
    fn a_fresh_newest_backup_reads_recent_true() {
        let now = Utc::now();
        let fact = resolve_backup_fact(now, true, true, Some(false), Some(now), AgeLevel::Fresh, None);
        assert_eq!(fact.recent, Some(true));
    }

    #[test]
    fn a_stale_or_overdue_newest_backup_reads_recent_false() {
        let now = Utc::now();
        for age in [AgeLevel::Stale, AgeLevel::Overdue] {
            let fact = resolve_backup_fact(now, true, true, Some(false), Some(now), age, None);
            assert_eq!(fact.recent, Some(false), "{age:?}");
        }
    }

    #[test]
    fn a_present_snapshot_verified_within_thirty_days_reads_verified_true() {
        let now = Utc::now();
        let at = now - Duration::days(29) - Duration::hours(23);
        let fact = resolve_backup_fact(now, true, true, Some(false), Some(now), AgeLevel::Fresh, Some(passed(at)));
        assert_eq!(fact.verified, Some(true));
        assert_eq!(fact.last_verified.as_ref().map(|v| v.ok), Some(true));
    }

    #[test]
    fn verified_reads_false_past_thirty_days_on_a_failure_or_with_nothing_verified() {
        let now = Utc::now();
        let too_old = resolve_backup_fact(
            now, true, true, Some(false), Some(now), AgeLevel::Fresh,
            Some(passed(now - Duration::days(31))),
        );
        assert_eq!(too_old.verified, Some(false), "a pass more than 30 days ago proves nothing today");

        let just_failed = resolve_backup_fact(
            now, true, true, Some(false), Some(now), AgeLevel::Fresh,
            Some(failed(now)),
        );
        assert_eq!(just_failed.verified, Some(false));

        let never = resolve_backup_fact(now, true, true, Some(false), Some(now), AgeLevel::Fresh, None);
        assert_eq!(never.verified, Some(false));
    }

    #[test]
    fn parse_destinationinfo_reads_one_or_two_destinations_and_the_plain_off_answer() {
        // This Mac's own answer (#116): `tmutil: No destinations configured.`,
        // exit 0.
        assert_eq!(parse_destinationinfo("tmutil: No destinations configured.\n", true), TimeMachineFact::NotConfigured);

        let one = "====================================================\n\
                   Name              : Backup Disk\n\
                   Kind              : Local\n\
                   Mount Point       : /Volumes/Backup Disk\n\
                   ID                : 11111111-1111-1111-1111-111111111111\n\
                   ====================================================\n";
        assert_eq!(
            parse_destinationinfo(one, true),
            TimeMachineFact::Configured { destinations: vec!["Backup Disk".into()] }
        );

        let two = format!(
            "{one}====================================================\n\
             Name              : Offsite Disk\n\
             Kind              : Network\n\
             ID                : 22222222-2222-2222-2222-222222222222\n\
             ====================================================\n"
        );
        assert_eq!(
            parse_destinationinfo(&two, true),
            TimeMachineFact::Configured { destinations: vec!["Backup Disk".into(), "Offsite Disk".into()] }
        );

        let TimeMachineFact::Unavailable { reason } = parse_destinationinfo("tmutil: some error\n", false) else {
            panic!("a non-zero exit is unavailable, not unsupported or configured");
        };
        assert!(reason.contains("some error"), "{reason}");
    }

    /// Pinned so a future edit cannot silently change the wire shape a
    /// nested-enum fact serializes to -- the JS side matches this exactly.
    #[test]
    fn a_repository_fact_and_a_time_machine_fact_serialize_the_way_the_ui_expects() {
        let fact = tracked(&["factory"], "projects/factory", "origin", "origin/main", 1);
        let json = serde_json::to_value(&fact).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "scopes": ["factory"],
                "path": "projects/factory",
                "state": { "state": "tracked", "remote": "origin", "upstream": "origin/main", "ahead": 1 },
                "remote_url": "https://github.com/o/projects/factory.git",
            })
        );

        let no_remote = RepositoryFact {
            scopes: vec!["factory".into()],
            path: "projects/factory".into(),
            state: RepositoryState::NoRemote,
            remote_url: None,
        };
        assert_eq!(
            serde_json::to_value(&no_remote).unwrap(),
            serde_json::json!({
                "scopes": ["factory"],
                "path": "projects/factory",
                "state": { "state": "no_remote" },
                "remote_url": null,
            })
        );

        let tm = TimeMachineFact::Configured { destinations: vec!["Backup Disk".into()] };
        assert_eq!(
            serde_json::to_value(&tm).unwrap(),
            serde_json::json!({ "state": "configured", "destinations": ["Backup Disk"] })
        );
        assert_eq!(serde_json::to_value(&TimeMachineFact::NotConfigured).unwrap(), serde_json::json!({ "state": "not_configured" }));
    }
}
