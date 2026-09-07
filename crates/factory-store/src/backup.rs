//! `VACUUM INTO` snapshot backups (ADR 0012 decision 4).

use std::path::Path;

use crate::{Store, StoreError};

/// Snapshot `store` into `destination` with `VACUUM INTO`.
///
/// Refuses a destination that already exists rather than overwriting one
/// backup with another; retention and deletion are the operator's business
/// (ADR 0012 decision 4), so this crate never removes a backup itself.
pub(crate) fn backup_to(store: &Store, destination: &Path) -> Result<(), StoreError> {
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
    store
        .conn
        .execute(&format!("VACUUM INTO '{escaped}'"), [])?;

    Ok(())
}
