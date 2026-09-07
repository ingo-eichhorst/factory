//! Shared helpers for factory-store's integration tests.
//!
//! This module is compiled once per test binary (each file under `tests/`
//! is its own crate), and no single test file uses every helper — hence the
//! blanket `dead_code` allow rather than one per unused function per binary.
//!
//! Only the crate's public API (`Store::open`, `Store::open_at`,
//! `Store::transaction`, ...) is used here plus raw SQL reachable through
//! the `rusqlite::Transaction` handed out by `transaction()` — this crate's
//! skeleton intentionally exposes no scope/session/task insertion methods of
//! its own (that belongs to a later slice), so tests exercise the schema
//! directly.

#![allow(dead_code)]

use std::path::Path;

use rusqlite::Connection;

/// Insert a scope row. Takes `&Connection` so it also accepts
/// `&rusqlite::Transaction` via deref coercion.
pub fn insert_scope(
    conn: &Connection,
    id: &str,
    name: &str,
    canonical_path: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO scopes (id, name, canonical_path) VALUES (?1, ?2, ?3)",
        (id, name, canonical_path),
    )?;
    Ok(())
}

/// Insert a session row with the given workspace path and state.
pub fn insert_session(
    conn: &Connection,
    id: &str,
    scope_id: &str,
    agent_name: &str,
    workspace_path: &str,
    state: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        (id, scope_id, agent_name, workspace_path, state),
    )?;
    Ok(())
}

/// Insert a minimal task row.
pub fn insert_task(
    conn: &Connection,
    id: &str,
    target_scope_id: &str,
    prompt: &str,
    status: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO tasks (id, target_scope_id, prompt, status) VALUES (?1, ?2, ?3, ?4)",
        (id, target_scope_id, prompt, status),
    )?;
    Ok(())
}

/// Count rows in `table`. Test-only, so a formatted (not parameterized)
/// table name is acceptable — every caller passes a literal.
pub fn row_count(conn: &Connection, table: &str) -> rusqlite::Result<i64> {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
}

/// Recursively list every file and directory beneath `root`, as paths
/// relative to `root`. Used to assert that factory-store never writes
/// outside `.factory/`.
pub fn recursive_listing(root: &Path) -> Vec<std::path::PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            out.push(
                path.strip_prefix(root)
                    .expect("entry under root")
                    .to_path_buf(),
            );
            if path.is_dir() {
                walk(&path, root, out);
            }
        }
    }

    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}
