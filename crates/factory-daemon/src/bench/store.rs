//! Compatibility path for the L5-owned live benchmark store.
pub use factory_core::bench_store::*;

#[cfg(test)]
mod integration_tests {
    use super::*;
    use factory_core::bench::{BenchRun, BenchRunStatus};
    use std::collections::BTreeMap;
    fn run(id: &str) -> BenchRun {
        BenchRun {
            id: id.to_string(),
            dataset: "demo".into(),
            dataset_revision: 1,
            cases: Vec::new(),
            case_bases: BTreeMap::new(),
            agents: vec!["builder".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Running,
            attempts: Vec::new(),
            started_at: chrono::Utc::now(),
            ended_at: None,
        }
    }

    /// The acceptance criterion, proved directly: these tables carry their
    /// own `CREATE TABLE IF NOT EXISTS` and no version check, so a bump to
    /// `store_sqlite.rs`'s `SCHEMA_VERSION` -- which drops the four *task*
    /// tables it owns -- must never touch a bench run sitting in the same
    /// database file.
    #[tokio::test]
    async fn bench_tables_survive_a_task_store_schema_version_mismatch() {
        let dir = std::env::temp_dir().join(format!(
            "factory-bench-schema-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("factory.sqlite");

        // Open as the task store first, so its own schema (and its
        // user_version) lands in this database file -- then the bench store
        // opens the very same file the way the daemon does.
        let _task_store = factory_plugins::SqliteStore::open(&db_path).unwrap();
        let bench_store = BenchStore::open(&db_path).unwrap();
        bench_store.put_run(&run("r1")).await.unwrap();
        drop(bench_store);
        drop(_task_store);

        // A "different version" task store: bump `user_version` behind its
        // back, forcing store_sqlite.rs's own mismatch-drop path on its next
        // open, without needing a second real schema version to exist.
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.pragma_update(None, "user_version", 999i64).unwrap();
        }
        let _task_store_again = factory_plugins::SqliteStore::open(&db_path).unwrap();

        let bench_store_again = BenchStore::open(&db_path).unwrap();
        let survived = bench_store_again.get_run("r1").await.unwrap();
        assert!(
            survived.is_some(),
            "the bench run must survive the task store's schema drop"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
