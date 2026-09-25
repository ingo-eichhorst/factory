//! Cost per run (#117): taking usage snapshots, and reading them back.
//!
//! Usage comes from the agent runtime and nowhere else --
//! `AgentRuntime::usage`, which herdr answers through a plugin. A run is
//! read three times: once its session is up and before it is handed the
//! task (the baseline), at each turn the harness says ended, and as it ends,
//! before its session is closed. Each reading is appended to the store,
//! answered or not; `factory_core::usage::run_usage` turns the lot into the
//! run's `usage`, which is written back onto the run so every reader of a
//! run -- `run show`, the UI, `/api/costs`, the metrics -- sees it without
//! knowing snapshots exist.
//!
//! Nothing here can fail a run. A snapshot that cannot be taken is recorded
//! as unknown with the reason, and a store that cannot keep it is logged
//! and passed over: usage is an observation, and a run's outcome never
//! depends on it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use factory_core::error::Result;
use factory_core::event::Event;
use factory_core::run::{Run, RunPatch};
use factory_core::task::Task;
use factory_core::usage::{
    run_usage, CostGroupBy, CostReport, CostRow, RunUsage, RunUsageEntry, SnapshotPoint, TaskUsage,
    UsageSnapshot,
};

use crate::engine::Engine;

/// What `factory cost` reads when no window is given.
const DEFAULT_WINDOW_DAYS: i64 = 30;

/// A run's `issue` when its task has no `issue=<n>` label.
const NO_ISSUE: &str = "(no issue)";

impl Engine {
    /// Ask the run's runtime what its session has used, keep the answer (or
    /// why there was none), and bring the run's derived `usage` up to date.
    /// A run with no session has nothing to ask about and gets no snapshot.
    pub(crate) async fn snapshot_usage(&self, run: &Run, point: SnapshotPoint) {
        let Some(session) = &run.session else { return };
        let at = Utc::now();
        let (usage, unknown) = match self.registry.runtime(&session.runtime) {
            Err(e) => (None, Some(e.to_string())),
            Ok(runtime) => match runtime.usage(session).await {
                Ok(Some(u)) => (Some(u), None),
                Ok(None) => (
                    None,
                    Some(format!("the {} runtime has no source for usage", runtime.name())),
                ),
                Err(e) => (None, Some(e.to_string())),
            },
        };
        let snapshot = UsageSnapshot {
            run_id: run.id.clone(),
            task_id: run.task_id.clone(),
            point,
            at,
            runtime: session.runtime.clone(),
            usage,
            unknown,
        };
        // Held across append, read and write-back, never across the runtime
        // call above: two snapshots of one run finishing together must not
        // leave the older sum written last.
        let _guard = self.usage_edit.lock().await;
        if let Err(e) = self.store.append_usage(&snapshot).await {
            tracing::warn!(run = %run.id, point = point.as_str(), error = %e, "could not keep a usage snapshot");
            return;
        }
        let snapshots = match self.store.usage_snapshots(&run.id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(run = %run.id, error = %e, "could not read usage snapshots back");
                return;
            }
        };
        let patch = RunPatch {
            usage: Some(run_usage(&snapshots)),
            ..Default::default()
        };
        match self.store.update_run(&run.id, &patch).await {
            Ok(updated) => self.bus.publish(Event::RunUpdated { run: updated.redacted() }),
            Err(e) => tracing::warn!(run = %run.id, error = %e, "could not record a run's usage"),
        }
    }

    /// `Request::TaskUsage`: every run's usage and their sum.
    pub(crate) async fn task_usage(&self, task_id: &str) -> Result<TaskUsage> {
        let task = self.require(task_id).await?;
        let runs = self.store.runs(&task.id, u32::MAX).await?;
        let now = Utc::now();
        let mut total = CostRow::new(task.id.clone(), Some(task.title.clone()));
        let mut entries = Vec::with_capacity(runs.len());
        for run in runs {
            total.add(run.usage.as_ref());
            entries.push(RunUsageEntry {
                wall_seconds: (run.ended_at.unwrap_or(now) - run.started_at).num_seconds(),
                usage: run
                    .usage
                    .clone()
                    .unwrap_or_else(|| RunUsage::unknown("no usage was recorded for this run", 0)),
                run_id: run.id,
                attempt: run.attempt,
                status: run.status,
            });
        }
        Ok(TaskUsage {
            task_id: task.id,
            total,
            runs: entries,
        })
    }

    /// `Request::Costs`: runs that started in `[from, to)`, summed per
    /// group. Read on request from the runs themselves -- there is no second
    /// store of costs to drift from them.
    pub(crate) async fn costs_report(
        &self,
        group_by: CostGroupBy,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        scope: Option<&str>,
    ) -> Result<CostReport> {
        let to = to.unwrap_or_else(Utc::now);
        let from = from.unwrap_or(to - Duration::days(DEFAULT_WINDOW_DAYS));
        if from >= to {
            return Err(factory_core::FactoryError::BadRequest(format!(
                "the window is empty: {from} is not before {to}"
            )));
        }
        let snapshot = self.factory_snapshot();
        // A scope means its whole subtree, the reading Operations, Policy
        // and Scenarios give it; a name that resolves to nothing is refused.
        let (scope_name, members) = match scope {
            None => (None, None),
            Some(name) => {
                let (asked, subtree) = crate::policies::subtree_scopes(&snapshot, Some(name))?;
                let asked = asked.expect("a named scope resolves or errors");
                let mut members: BTreeSet<String> = subtree.into_iter().map(|s| s.name).collect();
                members.insert(asked.name.clone());
                (Some(asked.name), Some(members))
            }
        };

        let runs: Vec<Run> = self
            .store
            .runs_between(from, to)
            .await?
            .into_iter()
            .filter(|r| r.started_at >= from && r.started_at < to)
            .collect();
        let mut tasks: BTreeMap<String, Option<Task>> = BTreeMap::new();
        for run in &runs {
            if !tasks.contains_key(&run.task_id) {
                let task = self.store.get(&run.task_id).await.ok().flatten();
                tasks.insert(run.task_id.clone(), task);
            }
        }

        let mut rows: BTreeMap<String, CostRow> = BTreeMap::new();
        let mut total = CostRow::new("total", None);
        for run in &runs {
            let task = tasks.get(&run.task_id).and_then(Option::as_ref);
            if let Some(members) = &members {
                // A deleted task's scope is unknown, so it is in no scope.
                if !task.is_some_and(|t| members.contains(&snapshot.canonical_scope_name(&t.scope))) {
                    continue;
                }
            }
            let (key, label) = group_key(group_by, run, task, |s| snapshot.canonical_scope_name(s));
            rows.entry(key.clone())
                .or_insert_with(|| CostRow::new(key, label))
                .add(run.usage.as_ref());
            total.add(run.usage.as_ref());
        }
        let mut rows: Vec<CostRow> = rows.into_values().collect();
        CostReport::sort_rows(&mut rows);
        Ok(CostReport {
            group_by,
            from,
            to,
            scope: scope_name,
            rows,
            total,
        })
    }
}

/// Which group a run falls in, and a readable label when the key is an id.
fn group_key(
    group_by: CostGroupBy,
    run: &Run,
    task: Option<&Task>,
    canonical: impl Fn(&str) -> String,
) -> (String, Option<String>) {
    match group_by {
        CostGroupBy::Task => (
            run.task_id.clone(),
            Some(task.map(|t| t.title.clone()).unwrap_or_else(|| "(deleted task)".into())),
        ),
        CostGroupBy::Issue => (
            task.and_then(|t| t.labels.get("issue"))
                .map(|n| format!("issue={n}"))
                .unwrap_or_else(|| NO_ISSUE.into()),
            None,
        ),
        CostGroupBy::Scope => (
            task.map(|t| canonical(&t.scope)).unwrap_or_else(|| "(deleted task)".into()),
            None,
        ),
        CostGroupBy::Agent => (
            format!(
                "{}/{}",
                task.map(|t| canonical(&t.scope)).unwrap_or_else(|| "?".into()),
                run.agent
            ),
            None,
        ),
    }
}

/// `Engine::snapshot_usage` needs `&self` only; the turn-ended path wants it
/// off the hook's own request, so it is spawned from an `Arc`.
pub(crate) fn spawn_snapshot(engine: &Arc<Engine>, run: Run, point: SnapshotPoint) {
    let engine = engine.clone();
    tokio::spawn(async move { engine.snapshot_usage(&run, point).await });
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::adapter::{AgentRuntime, RuntimeStatus, StartRequest};
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::error::FactoryError;
    use factory_core::run::{RunStatus, Trigger};
    use factory_core::task::{NewTask, SessionRef, TaskReport};
    use factory_core::usage::{HarnessUsage, SessionUsage, TokenCounts, UsageCost, UsageState};
    use factory_plugins::{Registry, SqliteStore};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    type Answer = std::result::Result<Option<SessionUsage>, String>;

    /// A runtime that answers `usage` from a script, one answer per call,
    /// and `None` once the script runs out.
    struct MeteredRuntime {
        answers: Mutex<VecDeque<Answer>>,
    }

    #[async_trait::async_trait]
    impl AgentRuntime for MeteredRuntime {
        fn name(&self) -> &str {
            "metered"
        }
        async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
            Ok(SessionRef {
                runtime: "metered".into(),
                handle: req.id.clone(),
                meta: Default::default(),
            })
        }
        async fn submit(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _: &SessionRef) -> Result<RuntimeStatus> {
            Ok(RuntimeStatus::Working)
        }
        async fn send_text(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn send_keys(&self, _: &SessionRef, _: &[String]) -> Result<()> {
            Ok(())
        }
        async fn read(&self, _: &SessionRef, _: u32) -> Result<String> {
            Ok(String::new())
        }
        async fn stop(&self, _: &SessionRef) -> Result<()> {
            Ok(())
        }
        async fn usage(&self, _: &SessionRef) -> Result<Option<SessionUsage>> {
            match self.answers.lock().unwrap().pop_front() {
                Some(Ok(u)) => Ok(u),
                Some(Err(e)) => Err(FactoryError::adapter("metered", e)),
                None => Ok(None),
            }
        }
    }

    fn usage(input: u64, usd: f64) -> SessionUsage {
        SessionUsage {
            schema: 1,
            handle: None,
            sampled_at: None,
            sessions: vec![HarnessUsage {
                session_id: "s1".into(),
                adapter: Some("claude-code".into()),
                model: Some("claude-opus-5".into()),
                tokens: TokenCounts {
                    input: Some(input),
                    output: Some(input / 10),
                    cache_read: Some(0),
                    cache_write: Some(0),
                },
                cost: UsageCost {
                    usd: Some(usd),
                    pricing_source: Some("litellm@test".into()),
                },
                elapsed_seconds: None,
                active_seconds: None,
                subagents: Vec::new(),
                rate_limit: None,
                unavailable: Default::default(),
            }],
        }
    }

    fn engine(answers: Vec<Answer>) -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-costs-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            version: 1,
            instance: Instance {
                id: "test".into(),
                name: "test".into(),
            },
            // Off: these tests dispatch real runs, and the default would fork
            // a real `caffeinate` on whatever machine runs them.
            daemon: DaemonConfig {
                power_assertion: false,
                ..DaemonConfig::default()
            },
            roles: Default::default(),
            policies: Default::default(),
            quality: Default::default(),
            scope: None,
            scopes: vec![Scope {
                id: "demo-id".into(),
                name: "demo".into(),
                path: PathBuf::new(),
                agent: None,
                agents: Vec::new(),
                runtime: Some("metered".into()),
                git: None,
                task_store: None,
                roles: Default::default(),
                policies: Default::default(),
                quality: Default::default(),
            }],
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        let mut registry = Registry::with_builtins();
        registry.add_runtime(
            Arc::new(MeteredRuntime {
                answers: Mutex::new(answers.into()),
            }),
            "test",
        );
        Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    async fn dispatched(engine: &Arc<Engine>, issue: Option<&str>) -> (Task, Run) {
        let task = engine
            .create(NewTask {
                title: "costly".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                labels: issue.map(|n| [("issue".to_string(), n.to_string())].into()).unwrap_or_default(),
                ..Default::default()
            })
            .await
            .unwrap();
        engine.start_run(&task.id, Trigger::Manual).await;
        let run = engine.store.active_run(&task.id).await.unwrap().expect("dispatched");
        (task, run)
    }

    async fn done(engine: &Arc<Engine>, task: &Task, run: &Run) -> Run {
        engine
            .report(
                &task.id,
                TaskReport {
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("ok".into()),
                    error: None,
                    token: run.token.clone(),
                },
            )
            .await
            .unwrap();
        engine.require_run(&run.id).await.unwrap()
    }

    #[tokio::test]
    async fn a_run_is_read_at_dispatch_and_at_its_end_and_carries_the_difference() {
        let engine = engine(vec![Ok(Some(usage(1_000, 0.10))), Ok(Some(usage(5_000, 0.50)))]);
        let (task, run) = dispatched(&engine, Some("117")).await;
        let baseline = engine.store.usage_snapshots(&run.id).await.unwrap();
        assert_eq!(baseline.len(), 1);
        assert_eq!(baseline[0].point, SnapshotPoint::Dispatch);

        let ended = done(&engine, &task, &run).await;
        let u = ended.usage.expect("the run carries its usage");
        assert_eq!(u.state, UsageState::Known, "{u:?}");
        assert_eq!(u.tokens.input, Some(4_000));
        assert_eq!(u.tokens.output, Some(400));
        assert!((u.cost_usd.unwrap() - 0.40).abs() < 1e-9);
        assert_eq!(u.as_of_point, Some(SnapshotPoint::RunEnd));
        assert_eq!(u.pricing_sources, vec!["litellm@test".to_string()]);

        let tu = engine.task_usage(&task.id).await.unwrap();
        assert_eq!(tu.total.runs, 1);
        assert!((tu.total.cost_usd - 0.40).abs() < 1e-9);
        assert_eq!(tu.runs[0].attempt, 1);
    }

    #[tokio::test]
    async fn a_turn_end_takes_a_reading_off_the_hooks_own_path() {
        let engine = engine(vec![Ok(Some(usage(0, 0.0))), Ok(Some(usage(2_000, 0.20)))]);
        let (task, run) = dispatched(&engine, None).await;
        engine
            .turn_ended(
                &task.id,
                factory_core::task::TurnEnded {
                    event: factory_core::task::TurnEndEvent::Stop,
                    pending_background: 0,
                    error: None,
                    error_details: None,
                    last_message: None,
                    token: run.token.clone(),
                },
            )
            .await
            .unwrap();
        // Written back after the snapshot is stored, so wait on the run.
        let mut u = None;
        for _ in 0..100 {
            u = engine.require_run(&run.id).await.unwrap().usage;
            if u.as_ref().and_then(|u| u.as_of_point) == Some(SnapshotPoint::TurnEnded) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let u = u.unwrap();
        assert_eq!(u.as_of_point, Some(SnapshotPoint::TurnEnded), "{u:?}");
        assert_eq!(engine.store.usage_snapshots(&run.id).await.unwrap().len(), 2);
        assert_eq!(u.tokens.input, Some(2_000));
    }

    #[tokio::test]
    async fn a_runtime_with_no_usage_leaves_the_run_unknown_with_the_reason() {
        let engine = engine(vec![]);
        let (task, run) = dispatched(&engine, None).await;
        let ended = done(&engine, &task, &run).await;
        assert_eq!(ended.status, RunStatus::Done, "no usage never fails a run");
        let u = ended.usage.unwrap();
        assert_eq!(u.state, UsageState::Unknown);
        assert!(u.reason.unwrap().contains("metered runtime has no source for usage"));
        assert_eq!(u.snapshots, 2);
    }

    #[tokio::test]
    async fn a_failed_baseline_is_kept_and_named() {
        let engine = engine(vec![Err("plugin exploded".into()), Ok(Some(usage(10, 0.01)))]);
        let (task, run) = dispatched(&engine, None).await;
        let ended = done(&engine, &task, &run).await;
        let u = ended.usage.unwrap();
        assert_eq!(u.state, UsageState::Unknown);
        assert!(u.reason.unwrap().contains("plugin exploded"));
    }

    #[tokio::test]
    async fn costs_group_by_issue_scope_agent_and_task_and_count_the_unknown() {
        let engine = engine(vec![
            Ok(Some(usage(0, 0.0))),
            Ok(Some(usage(1_000, 1.25))),
            // The second run's runtime has nothing to say at all.
        ]);
        let (t1, r1) = dispatched(&engine, Some("117")).await;
        done(&engine, &t1, &r1).await;
        let (t2, r2) = dispatched(&engine, None).await;
        done(&engine, &t2, &r2).await;

        let by_issue = engine.costs_report(CostGroupBy::Issue, None, None, None).await.unwrap();
        assert_eq!(by_issue.rows.len(), 2);
        assert_eq!(by_issue.rows[0].key, "issue=117", "most expensive first");
        assert!((by_issue.rows[0].cost_usd - 1.25).abs() < 1e-9);
        assert_eq!(by_issue.rows[1].key, NO_ISSUE);
        assert_eq!(by_issue.rows[1].runs_unknown, 1);
        assert_eq!(by_issue.total.runs, 2);
        assert_eq!(by_issue.total.runs_unknown, 1, "counted, never dropped");
        assert_eq!(by_issue.total.tokens.input, 1_000);

        let by_agent = engine.costs_report(CostGroupBy::Agent, None, None, None).await.unwrap();
        assert_eq!(by_agent.rows.len(), 1);
        assert_eq!(by_agent.rows[0].key, "demo/shell");
        assert_eq!(by_agent.rows[0].runs, 2);

        let by_scope = engine.costs_report(CostGroupBy::Scope, None, None, Some("demo")).await.unwrap();
        assert_eq!(by_scope.rows[0].key, "demo");
        assert_eq!(by_scope.scope.as_deref(), Some("demo"));

        let by_task = engine.costs_report(CostGroupBy::Task, None, None, None).await.unwrap();
        assert_eq!(by_task.rows[0].key, t1.id);
        assert_eq!(by_task.rows[0].label.as_deref(), Some("costly"));

        // A window before any of it holds nothing; an empty one is refused.
        let past = Utc::now() - Duration::days(400);
        let none = engine
            .costs_report(CostGroupBy::Task, Some(past), Some(past + Duration::days(1)), None)
            .await
            .unwrap();
        assert!(none.rows.is_empty());
        assert!(engine.costs_report(CostGroupBy::Task, Some(past), Some(past), None).await.is_err());
        assert!(engine.costs_report(CostGroupBy::Task, None, None, Some("nope")).await.is_err());
    }
}
