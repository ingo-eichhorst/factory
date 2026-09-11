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
//!   mutators rather than rejecting them. That is the same answer whether
//!   Factory runs as a daemon or as a process per invocation, which is why this
//!   crate could be written while that question was still open. ADR 0014 has
//!   since chosen the daemon, and nothing here changed as a result — the
//!   exclusive lock that keeps a *second daemon* from starting belongs to the
//!   daemon, not to the store;
//! - `synchronous = FULL` rather than WAL's usual `NORMAL`, because Slice 7
//!   commits a task as `queued` *before* a prompt reaches a terminal precisely
//!   so a crash cannot lose the record of work that may already have had
//!   external effect. `NORMAL` can lose recent commits on power loss and would
//!   undermine the guarantee that commit ordering exists to provide.
//!
//! This crate writes only inside a `.factory/` directory (design §4).

mod backup;
pub mod durable;
mod init;
mod migrations;
pub use migrations::latest_schema_version;
mod pragma;
mod schema;
mod snapshot;

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
        migrations::apply(&mut conn, &db_path)?;

        Ok(Self {
            conn,
            path: db_path,
        })
    }

    /// Open an existing database read-only (ADR 0018 decision 1): applies
    /// the pragmas that are safe without write access, reports the schema
    /// version it finds, and never migrates.
    ///
    /// `Store::open_at` is the only door that calls `migrations::apply`, and
    /// it does so unconditionally — so a caller whose whole contract is
    /// "look, don't touch" (`factory doctor`'s read-only pass; the backup
    /// drill's inspection steps, which today migrate the very snapshot they
    /// are inspecting) needs a door where that is true in fact, not by
    /// convention. What makes it true in fact is the SQLite connection
    /// itself: it is opened with `SQLITE_OPEN_READ_ONLY`, so any write —
    /// this module never issuing one, migration code someone adds here
    /// later, a caller's own mistake — fails at the driver rather than
    /// depending on this function's good behaviour. Measured directly: a
    /// `SQLITE_OPEN_READ_ONLY` connection can read a WAL-mode database
    /// (Factory's own) both after a clean close and while a writer still
    /// holds it open, so no pragma here has anything to lose from the
    /// restriction.
    ///
    /// Never creates a database. `db_path` must already exist —
    /// `SQLITE_OPEN_READ_ONLY` reports a missing file as
    /// `StoreError::Sqlite`, the same way `rusqlite` reports it for any
    /// other read-only open.
    ///
    /// `Store::transaction` is still callable on the result — `Store` has no
    /// read-only variant of its own type — but do not expect it to be a
    /// useful way to write: `BEGIN IMMEDIATE` itself succeeds even on a
    /// read-only connection (measured directly), so it is the first actual
    /// write *inside* the transaction that fails with `SQLITE_READONLY`, not
    /// the `transaction()` call. A `commit()` with nothing written then
    /// succeeds trivially. Prefer [`Store::connection`] for reads through
    /// this door.
    pub fn open_read_only(db_path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db_path = db_path.as_ref().to_path_buf();
        let conn = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        pragma::apply_read_only(&conn)?;

        Ok(Self {
            conn,
            path: db_path,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Borrow the connection for reads.
    ///
    /// A read path must not go through [`Store::transaction`], which is
    /// `BEGIN IMMEDIATE` and therefore takes a *write* lock: a reader would then
    /// contend with real writers and, under ADR 0012's `busy_timeout`, fail
    /// after five seconds for a query that changes nothing. Under WAL, readers
    /// never block and are never blocked, so a borrowed connection is the
    /// correct tool.
    ///
    /// This exposes no new dependency — [`Store::transaction`] already returns a
    /// `rusqlite` type. The `&self` receiver is what keeps it honest: a caller
    /// holding one of these cannot start a transaction on the same store.
    #[must_use]
    pub fn connection(&self) -> &rusqlite::Connection {
        &self.conn
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

    /// ADR 0018 decision 2: `rusqlite_migration`'s own
    /// `DatabaseTooFarAhead` names neither number — this does, plus what an
    /// operator can do about it. `built_schema` is the highest schema
    /// version this build's `migrations()` defines; `database_schema` is
    /// the `user_version` the database was actually found at.
    #[error(
        "database schema is too far ahead: this build understands schema {built_schema}, \
         the database is at schema {database_schema}; install a newer build of Factory, \
         or restore a snapshot taken before the upgrade that produced schema {database_schema}"
    )]
    DatabaseTooFarAhead {
        built_schema: i64,
        database_schema: i64,
    },

    #[error("cannot use {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("backup destination already exists: {path}\n  help: {help}")]
    BackupExists { path: PathBuf, help: String },
}
