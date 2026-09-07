//! Connection setup: the exact pragmas ADR 0012 decision 3 requires, applied
//! on every open.

use rusqlite::Connection;

use crate::StoreError;

/// Apply WAL, foreign keys, busy timeout, and full-durability synchronous
/// mode, in that order.
///
/// `journal_mode` is set with [`Connection::pragma_update_and_check`] rather
/// than a plain `execute`, because SQLite reports back the mode it actually
/// switched to as a result row — `execute` rejects statements that return
/// rows (`ExecuteReturnedResults`). The other three pragmas return no rows
/// and use [`Connection::pragma_update`].
pub(crate) fn apply(conn: &Connection) -> Result<(), StoreError> {
    // Readers do not block the writer.
    let _mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;

    // Off by default *per connection* in SQLite: skipping this makes every
    // FOREIGN KEY in the schema decorative.
    conn.pragma_update(None, "foreign_keys", "ON")?;

    // Wait rather than fail instantly on contention.
    conn.pragma_update(None, "busy_timeout", 5_000_i64)?;

    // FULL rather than WAL's usual NORMAL: durability over speed. A task is
    // committed `queued` before a prompt reaches a terminal, precisely so a
    // crash cannot lose the record of work that may already have had
    // external effect. NORMAL can lose recent commits on power loss and
    // would undermine the guarantee that commit ordering exists to provide.
    conn.pragma_update(None, "synchronous", "FULL")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::Store;

    /// A pragma that silently failed to apply looks exactly like one that
    /// worked, unless something reads it back after the fact. This is that
    /// read-back, against a freshly opened `Store` rather than against the
    /// pragma-setting code path itself.
    #[test]
    fn all_four_pragmas_are_actually_in_effect() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open");

        let journal_mode: String = store
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal_mode readback");
        assert_eq!(journal_mode.to_lowercase(), "wal");

        let foreign_keys: i64 = store
            .conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("foreign_keys readback");
        assert_eq!(foreign_keys, 1);

        let busy_timeout: i64 = store
            .conn
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .expect("busy_timeout readback");
        assert_eq!(busy_timeout, 5000);

        // FULL is reported back as the integer 2, per the coordinator's
        // verified spike output (`synchronous=2`).
        let synchronous: i64 = store
            .conn
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .expect("synchronous readback");
        assert_eq!(synchronous, 2);
    }
}
