//! Schema versioning via `PRAGMA user_version` (ADR 0012 decision 2).
//!
//! `rusqlite_migration` tracks schema state in SQLite's own `user_version`
//! rather than a bookkeeping table of its own, so an operator can read the
//! schema version with the `sqlite3` CLI during an incident, with no Factory
//! code and no knowledge of a table layout (Slice 10's `factory doctor`).

use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

use crate::{StoreError, schema};

/// The full set of released migrations, in order.
///
/// Forward-only and append-only: once released, an entry here is never
/// edited. A schema change ships as a new entry appended after the last one,
/// never as an edit to an existing one. Version 1 is a single migration, so
/// a fresh database's `user_version` is `1`.
fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(schema::V1_SCHEMA)])
}

/// Bring `conn` to the latest schema.
///
/// Safe to call against a fresh database or one already at the latest
/// version: `rusqlite_migration` compares `user_version` against the
/// migration list and applies only what is missing, which is what makes
/// repeated `Store::open` calls idempotent (Slice 2's acceptance criterion).
pub(crate) fn apply(conn: &mut Connection) -> Result<(), StoreError> {
    migrations().to_latest(conn)?;
    Ok(())
}
