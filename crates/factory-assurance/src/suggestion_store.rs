//! Where suggestions (`#275`) live: one row per suggestion, keyed by id,
//! with `scope`, `state` and `filed_at` broken out as their own columns so a
//! restart can still list and filter cheaply; `data` is the whole
//! [`crate::suggestion::Suggestion`], history included. Follows
//! `bench_store.rs` exactly -- its own `CREATE TABLE IF NOT EXISTS`, no
//! `SCHEMA_VERSION` check, nothing ever deleted, a full row replace on every
//! state change rather than a second table of deltas.

use crate::suggestion::Suggestion;
use factory_kernel::{FactoryError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS suggestions (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    state TEXT NOT NULL,
    filed_at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS suggestions_scope ON suggestions(scope, filed_at);
CREATE INDEX IF NOT EXISTS suggestions_state ON suggestions(state, filed_at);
"#;

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("suggestion sqlite", error.to_string())
}

fn decode(json: String) -> Result<Suggestion> {
    serde_json::from_str(&json).map_err(error)
}

#[derive(Clone)]
pub struct SuggestionStore {
    conn: Arc<Mutex<Connection>>,
}

impl SuggestionStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(error)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(error)?;
        conn.execute_batch(SCHEMA).map_err(error)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(error)?;
        conn.execute_batch(SCHEMA).map_err(error)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|_| error("connection mutex poisoned"))?;
            f(&mut guard)
        })
        .await
        .map_err(error)?
    }

    /// Insert or fully replace one suggestion -- filing, and every later
    /// state change, write the row this way. Nothing is ever deleted.
    pub async fn put(&self, suggestion: &Suggestion) -> Result<()> {
        let suggestion = suggestion.clone();
        self.with_conn(move |conn| {
            let data = serde_json::to_string(&suggestion).map_err(error)?;
            conn.execute(
                "INSERT INTO suggestions (id, scope, state, filed_at, data) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET scope=excluded.scope, state=excluded.state, data=excluded.data",
                params![
                    suggestion.id,
                    suggestion.scope,
                    suggestion.state.as_str(),
                    suggestion.filed_at.to_rfc3339(),
                    data,
                ],
            )
            .map_err(error)?;
            Ok(())
        })
        .await
    }

    pub async fn get(&self, id: &str) -> Result<Option<Suggestion>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            let json: Option<String> = conn
                .query_row("SELECT data FROM suggestions WHERE id = ?1", [&id], |row| row.get(0))
                .optional()
                .map_err(error)?;
            json.map(decode).transpose()
        })
        .await
    }

    /// Every suggestion, newest filed first. Filtering (scope subtree, kind,
    /// target, state) is the caller's job -- `crate::suggestion::Filter` --
    /// over this: suggestion volume is nowhere near where a second query
    /// shape per filter combination would pay for itself.
    pub async fn all(&self) -> Result<Vec<Suggestion>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT data FROM suggestions ORDER BY filed_at DESC")
                .map_err(error)?;
            let rows: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            rows.into_iter()
                .filter_map(|json| match decode(json) {
                    Ok(s) => Some(Ok(s)),
                    Err(err) => {
                        tracing::warn!("skipping malformed suggestion row: {err}");
                        None
                    }
                })
                .collect()
        })
        .await
    }

    /// The suggestion whose pending ask started this run, if any -- used to
    /// settle the answer once that continuation run reports (`#275`'s "ask
    /// the agent"). A linear scan: asks in flight are rare and short-lived.
    pub async fn find_by_ask_run(&self, run_id: &str) -> Result<Option<Suggestion>> {
        let all = self.all().await?;
        Ok(all.into_iter().find(|s| {
            s.ask.as_ref().is_some_and(|ask| ask.run_id == run_id && ask.answer.is_none())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suggestion::{Filing, SuggestionKind};
    use chrono::{DateTime, Utc};

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn suggestion(id: &str, scope: &str, filed_at: i64) -> Suggestion {
        Filing {
            run_id: "r1".into(),
            task_id: "t1".into(),
            scope: scope.into(),
            agent: "worker".into(),
            harness: "claude-code".into(),
            session_id: Some("sess".into()),
            usage: None,
            kind: SuggestionKind::Capability,
            target: "L2 secret x".into(),
            summary: "blocked".into(),
            detail: None,
            wasted_tokens: None,
        }
        .file(id.into(), at(filed_at))
    }

    #[tokio::test]
    async fn put_then_get_round_trips_the_whole_record() {
        let store = SuggestionStore::in_memory().unwrap();
        let s = suggestion("s1", "demo", 1);
        store.put(&s).await.unwrap();
        let loaded = store.get("s1").await.unwrap().unwrap();
        assert_eq!(loaded.id, "s1");
        assert_eq!(loaded.scope, "demo");
        assert_eq!(loaded.history.len(), 1);
        assert!(store.get("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn put_replaces_the_row_in_place_rather_than_duplicating_it() {
        let store = SuggestionStore::in_memory().unwrap();
        let mut s = suggestion("s1", "demo", 1);
        store.put(&s).await.unwrap();
        s.dismiss("not worth it".into(), "owner", at(2)).unwrap();
        store.put(&s).await.unwrap();

        let all = store.all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].state, crate::suggestion::SuggestionState::Dismissed);
        assert_eq!(all[0].history.len(), 2);
    }

    #[tokio::test]
    async fn all_orders_newest_filed_first() {
        let store = SuggestionStore::in_memory().unwrap();
        store.put(&suggestion("older", "demo", 1)).await.unwrap();
        store.put(&suggestion("newer", "demo", 10)).await.unwrap();
        let all = store.all().await.unwrap();
        assert_eq!(all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["newer", "older"]);
    }

    #[tokio::test]
    async fn find_by_ask_run_matches_only_the_unanswered_pending_ask() {
        let store = SuggestionStore::in_memory().unwrap();
        let mut s = suggestion("s1", "demo", 1);
        s.ask("why?".into(), "owner", "ask-run", at(2));
        store.put(&s).await.unwrap();

        let found = store.find_by_ask_run("ask-run").await.unwrap().unwrap();
        assert_eq!(found.id, "s1");
        assert!(store.find_by_ask_run("no-such-run").await.unwrap().is_none());

        s.answer("ask-run", "because".into(), at(3));
        store.put(&s).await.unwrap();
        assert!(store.find_by_ask_run("ask-run").await.unwrap().is_none(), "already answered");
    }
}
