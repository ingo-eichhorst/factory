//! Pre-migration snapshots (ADR 0018 decision 3).
//!
//! Migrations are forward-only and append-only (ADR 0012 decision 2): a
//! released migration has no inverse. So before `migrations::apply` changes
//! an *existing* database, this takes a `VACUUM INTO` copy — the only
//! rollback that will ever exist for what is about to happen. If that copy
//! cannot be written, the caller must not run the migration; this module
//! only decides *whether* and *where* to snapshot, `migrations::apply`
//! decides what to do with the `Result`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::{StoreError, backup};

/// Snapshot `conn` — the database at `db_path`, currently at
/// `from_version` — before it is migrated, if and only if it already has a
/// schema and at least one migration is pending.
///
/// Two exemptions, both from ADR 0018 decision 3 directly:
///
/// - `from_version <= 0`: a fresh database. There is nothing to lose, and a
///   snapshot of an empty file is noise in the directory an operator
///   searches during an incident.
/// - `pending <= 0`: nothing is about to change. This also covers the
///   `DatabaseTooFarAhead` case `migrations::apply` is about to reject —
///   `pending` is negative there, and a migration that is refused outright
///   has nothing for a snapshot to protect against.
///
/// `pending` is the caller's `rusqlite_migration::Migrations::pending_migrations`
/// result, taken as a parameter rather than recomputed here so there is
/// exactly one place that calls into `rusqlite_migration` per `apply`.
pub(crate) fn snapshot_before_migration(
    conn: &Connection,
    db_path: &Path,
    from_version: i64,
    pending: i32,
) -> Result<(), StoreError> {
    if from_version <= 0 || pending <= 0 {
        return Ok(());
    }

    let to_version = from_version + i64::from(pending);
    let backups_dir = backups_dir(db_path);

    // `VACUUM INTO` does not create its own parent directory (proven by
    // `tests/no_writes_outside_factory.rs`, which creates `.factory/backups/`
    // itself before calling `Store::backup_to`) — so this must.
    std::fs::create_dir_all(&backups_dir).map_err(|source| StoreError::Io {
        path: backups_dir.clone(),
        source,
    })?;

    let destination = backups_dir.join(format!(
        "pre-migration-{from_version}-to-{to_version}-{}.sqlite",
        timestamp()
    ));

    backup::vacuum_into(conn, &destination)
}

/// `.factory/backups/`: the directory named `backups` beside the database
/// file. For the company-root database (`<root>/.factory/factory.sqlite`),
/// `db_path`'s parent is already `.factory/`, so this lands exactly on
/// `.factory/backups/`, matching the path ADR 0018 decision 3 names.
fn backups_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("backups")
}

/// Nanoseconds since the Unix epoch, as a decimal string.
///
/// Fine-grained enough that two snapshots taken back to back by one process
/// cannot collide, without a date-formatting dependency this crate does not
/// already have. Whether a *retried* upgrade (a second migration attempt
/// later, after an operator intervenes) should reuse the first attempt's
/// snapshot rather than take a second one is explicitly out of scope — ADR
/// 0018's own open item, deferred to an operator watching, not decided here.
fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::backups_dir;

    #[test]
    fn backups_dir_is_a_backups_directory_beside_the_database_file() {
        assert_eq!(
            backups_dir(Path::new("/company/root/.factory/factory.sqlite")),
            Path::new("/company/root/.factory/backups")
        );
    }

    #[test]
    fn backups_dir_falls_back_to_a_relative_backups_directory_for_a_bare_filename() {
        assert_eq!(
            backups_dir(Path::new("factory.sqlite")),
            Path::new("backups")
        );
    }
}
