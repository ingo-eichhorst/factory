//! `VACUUM INTO` snapshot backups: an operator-triggered one (ADR 0012
//! decision 4) and the automatic pre-migration one ADR 0018 decision 3
//! requires. Both go through [`vacuum_into`], the only place this crate
//! issues a `VACUUM INTO` statement.

use std::path::Path;

use rusqlite::Connection;

use crate::{Store, StoreError};

/// Snapshot `store` into `destination` with `VACUUM INTO`.
///
/// Consistent without stopping writers. Copying the file is wrong while WAL
/// is active, and the incremental Online Backup API solves a problem — very
/// large databases — that Factory does not have.
pub(crate) fn backup_to(store: &Store, destination: &Path) -> Result<(), StoreError> {
    vacuum_into(&store.conn, destination)
}

/// The `VACUUM INTO` call itself, shared by [`backup_to`] (an
/// operator-triggered backup against an already-open [`Store`]) and the
/// pre-migration snapshot `snapshot::snapshot_before_migration` takes with
/// the same connection `migrations::apply` is about to migrate, before a
/// `Store` exists to hand back.
///
/// Refuses a destination that already exists rather than overwriting one
/// backup with another; retention and deletion are the operator's business
/// (ADR 0012 decision 4), so this crate never removes a backup itself. This
/// also protects the pre-migration snapshot's timestamped name: ADR 0018's
/// own open item notes that a collision is what would make a retried
/// snapshot unsafe to skip silently, and refusing here is what keeps that
/// true.
pub(crate) fn vacuum_into(conn: &Connection, destination: &Path) -> Result<(), StoreError> {
    if destination.exists() {
        return Err(StoreError::BackupExists {
            path: destination.to_path_buf(),
            help: "remove or rename the existing backup file, or choose a different destination"
                .to_string(),
        });
    }

    let destination_str = destination.to_str().ok_or_else(|| StoreError::Io {
        path: destination.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "backup destination path is not valid UTF-8",
        ),
    })?;

    // `VACUUM INTO` takes its destination as a string literal in the SQL
    // text, not a bindable parameter — the coordinator's verified spike used
    // exactly this literal form. Single quotes in the path are escaped by
    // doubling them, per SQL string-literal rules, since the destination
    // comes from the filesystem and is not guaranteed quote-free.
    let escaped = destination_str.replace('\'', "''");
    conn.execute(&format!("VACUUM INTO '{escaped}'"), [])?;

    Ok(())
}
