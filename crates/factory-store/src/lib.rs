//! The one operational database at the company root.
//!
//! Design §4: `<company>/.factory/factory.sqlite` is canonical for registered
//! paths, session mappings, workspace leases, and tasks. Scope `config.yaml`
//! and `AGENTS.md` files remain canonical for configuration and context — this
//! store never duplicates them.
//!
//! Every mechanism here is fixed by ADR 0012 and was verified against the real
//! crates before implementation began:
//!
//! - `rusqlite` with `bundled`, so the SQLite engine is pinned into the binary
//!   and a host upgrade cannot change Factory's behaviour underneath it;
//! - `rusqlite_migration` tracking schema state in `PRAGMA user_version`, so an
//!   operator can read the schema version with the `sqlite3` CLI during an
//!   incident, without Factory and without knowing a bookkeeping table's shape;
//! - WAL, `busy_timeout`, and `BEGIN IMMEDIATE`, which *serialize* concurrent
//!   mutators rather than rejecting them — the same answer whether Factory runs
//!   as a daemon or as a process per invocation, which is why the parked
//!   daemon decision does not reach this slice;
//! - `synchronous = FULL` rather than WAL's usual `NORMAL`, because Slice 7
//!   commits a task as `queued` *before* a prompt reaches a terminal precisely
//!   so a crash cannot lose the record of work that may already have had
//!   external effect. `NORMAL` can lose recent commits on power loss and would
//!   undermine the guarantee that commit ordering exists to provide.
//!
//! This crate writes only inside a `.factory/` directory (design §4).

mod backup;
mod init;
mod migrations;
mod pragma;
mod schema;

use std::path::{Path, PathBuf};

/// An open connection to the company-root database.
pub struct Store {
    conn: rusqlite::Connection,
    path: PathBuf,
}

impl Store {
    /// Open — creating if absent — the database beneath `company_root`.
    ///
    /// Creates `<company_root>/.factory/` if needed and nothing outside it.
    /// Idempotent: running it against a valid existing database changes no
    /// configuration and loses no data, per the Slice 2 acceptance criterion.
    pub fn open(company_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let factory_dir = company_root.as_ref().join(".factory");
        std::fs::create_dir_all(&factory_dir).map_err(|source| StoreError::Io {
            path: factory_dir.clone(),
            source,
        })?;
        Self::open_at(factory_dir.join("factory.sqlite"))
    }

    /// Open a database file directly. Used by tests and by the backup drill.
    pub fn open_at(db_path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db_path = db_path.as_ref().to_path_buf();

        // Restrictive permissions are set on the file before SQLite ever
        // opens it, so the `-wal`/`-shm` sidecars SQLite creates under WAL
        // inherit a restrictive mode too. See `init` for why the order
        // matters.
        init::create_with_restrictive_permissions(&db_path)?;

        let mut conn = rusqlite::Connection::open(&db_path)?;
        pragma::apply(&conn)?;
        migrations::apply(&mut conn)?;

        Ok(Self {
            conn,
            path: db_path,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The applied schema version, read from `PRAGMA user_version`.
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(version)
    }

    /// `PRAGMA integrity_check`, returning its verdict verbatim.
    ///
    /// Returns the string rather than a bool because a corrupt database
    /// reports *what* is wrong, and discarding that is throwing away the only
    /// useful part of the answer.
    pub fn integrity_check(&self) -> Result<String, StoreError> {
        let verdict: String = self
            .conn
            .pragma_query_value(None, "integrity_check", |row| row.get(0))?;
        Ok(verdict)
    }

    /// Snapshot the live database with `VACUUM INTO`.
    ///
    /// Consistent without stopping writers. Copying the file is wrong while WAL
    /// is active, and the incremental Online Backup API solves a problem —
    /// very large databases — that Factory does not have.
    ///
    /// Refuses a destination that already exists rather than overwriting one
    /// backup with another.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<(), StoreError> {
        backup::backup_to(self, destination.as_ref())
    }

    /// Begin a write transaction with `BEGIN IMMEDIATE`.
    ///
    /// Immediate rather than deferred so a writer takes its lock up front,
    /// instead of doing work, discovering the conflict at first write, and
    /// failing after it has already read.
    pub fn transaction(&mut self) -> Result<rusqlite::Transaction<'_>, StoreError> {
        self.conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(StoreError::from)
    }
}

/// Everything that can go wrong opening or operating the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration failed: {0}")]
    Migration(#[from] rusqlite_migration::Error),

    #[error("cannot use {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("backup destination already exists: {path}\n  help: {help}")]
    BackupExists { path: PathBuf, help: String },
}
