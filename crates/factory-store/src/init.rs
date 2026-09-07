//! Creating the database file with restrictive permissions before SQLite
//! ever touches it.

use std::fs::OpenOptions;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::StoreError;

/// Create `db_path` if it does not exist yet, and set its mode to `0600`,
/// before any `rusqlite::Connection` is opened against it.
///
/// Order matters: SQLite derives the mode of the `-wal` and `-shm` sidecar
/// files it creates from the database file's mode *at the time those
/// sidecars are created*. Opening a connection first (which, under WAL,
/// creates the sidecars immediately) and `chmod`-ing the database file
/// afterwards can leave a `-wal` file more permissive than the database
/// beside it, which defeats the point of restricting the database file's
/// permissions in the first place.
///
/// Idempotent: called again against an existing file, this only re-asserts
/// the mode and does not truncate or otherwise disturb its contents.
pub(crate) fn create_with_restrictive_permissions(db_path: &Path) -> Result<(), StoreError> {
    let io_err = |source: std::io::Error| StoreError::Io {
        path: db_path.to_path_buf(),
        source,
    };

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(db_path)
        .map_err(io_err)?;

    let mut permissions = file.metadata().map_err(io_err)?.permissions();
    permissions.set_mode(0o600);
    file.set_permissions(permissions).map_err(io_err)?;

    Ok(())
}
