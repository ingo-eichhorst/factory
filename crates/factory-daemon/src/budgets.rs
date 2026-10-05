//! L6 monthly budget views over the single L4 spend fact port. Authored
//! intent is re-read, not copied into a mutable aggregate or provider plan.
use crate::engine::Engine;
#[cfg(test)]
use crate::facts::Facts;
use chrono::{DateTime, Utc};
use factory_core::{budget, config::Scope, error::Result};
use factory_kernel::{CostGroupBy, CostReport};
#[cfg(test)]
use factory_kernel::{SpendQuery, L6};

/// Outside-stack projection of the current scope chain. L6 compiles raw
/// limits; assurance gathers spend and judges them through its own ports.
pub(crate) fn check_budget_intent(
    snapshot: &factory_core::config::Factory,
    scope: &Scope,
    config: &budget::PolicyConfig,
) -> factory_assurance::evidence::BudgetIntent {
    let mut chain = snapshot.config.ancestors_of(scope);
    chain.push(scope);
    budget::check_intent(
        config,
        chain.into_iter().map(|s| (s.id.clone(), s.name.clone())),
    )
}

impl Engine {
    pub(crate) async fn budget_report(
        &self,
        scope: Option<&str>,
        group_by: CostGroupBy,
        now: DateTime<Utc>,
    ) -> Result<budget::Report> {
        let snapshot = self.factory_snapshot();
        let provider = <CostReport as crate::facts::Port>::provider(self);
        factory_direction::budget_service::Service::new(
            snapshot.root.clone(),
            snapshot.config.scopes.iter().map(budget_scope).collect(),
            snapshot.config.scope.as_ref().map(budget_scope),
            &provider,
        )
        .report(scope, group_by, now)
        .await
    }
}
fn budget_scope(scope: &Scope) -> factory_direction::budget_service::Scope {
    factory_direction::budget_service::Scope {
        id: scope.id.clone(),
        name: scope.name.clone(),
        path: scope.path.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::{
        adapter::store::task_from_new,
        config::{Config, Factory},
        run::{NewRun, RunStatus, Trigger},
        task::NewTask,
        usage::{RunUsage, TokenCounts, UsageState},
    };
    use factory_plugins::{Registry, SqliteStore};
    use std::{path::PathBuf, sync::Arc};

    struct Instance {
        engine: Arc<Engine>,
        root: PathBuf,
        database: PathBuf,
    }
    impl Instance {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("factory-budget-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(root.join(".factory/budgets")).unwrap();
            let mut config: Config =
                serde_yaml_ng::from_str("version: 1\ninstance: {id: test, name: test}\n").unwrap();
            config.daemon.power_assertion = false;
            config.scopes = [
                ("root-id", "root", ""),
                ("work-id", "work", "projects/work"),
                ("nested-id", "nested", "projects/work/nested"),
                ("side-id", "work-extra", "projects/work-extra"),
            ]
            .into_iter()
            .map(|(id, name, path)| {
                let mut s: Scope =
                    serde_yaml_ng::from_str(&format!("id: {id}\nname: {name}\n")).unwrap();
                s.path = path.into();
                s
            })
            .collect();
            config.scope = Some(config.scopes[0].clone());
            let database = root.join(".factory/test.sqlite");
            let engine = Arc::new(Engine::new(
                Factory {
                    root: root.clone(),
                    config,
                },
                Registry::with_builtins(),
                Arc::new(SqliteStore::open(&database).unwrap()),
                "factory".into(),
                Vec::new(),
            ));
            Self {
                engine,
                root,
                database,
            }
        }
        fn limits(&self, raw: &str) {
            std::fs::write(budget::catalogue_path(&self.root), raw).unwrap();
        }
        async fn run(&self, scope: &str, at: &str, usage: Option<RunUsage>) -> String {
            let task = self
                .engine
                .store
                .create(&task_from_new(
                    NewTask {
                        title: "measured".into(),
                        ..Default::default()
                    },
                    scope.into(),
                    "worker".into(),
                    "shell".into(),
                ))
                .await
                .unwrap();
            let mut run = self
                .engine
                .store
                .create_run(&NewRun {
                    task_id: task.id.clone(),
                    trigger: Trigger::Manual,
                    agent: "worker".into(),
                    adapter: "shell".into(),
                    runtime: "shell".into(),
                    token: "test".into(),
                    queued_at: None,
                    scheduled_for: None,
                })
                .await
                .unwrap();
            run.started_at = time(at);
            run.ended_at = Some(time("2026-10-15T00:00:00Z"));
            run.status = RunStatus::Done;
            run.usage = usage;
            rusqlite::Connection::open(&self.database).unwrap().execute(
                "UPDATE runs SET status = ?2, started_at = ?3, ended_at = ?4, data = ?5 WHERE id = ?1",
                rusqlite::params![run.id, run.status.as_str(), run.started_at.to_rfc3339(), run.ended_at.unwrap().to_rfc3339(), serde_json::to_string(&run).unwrap()]).unwrap();
            task.id
        }
        async fn report(&self, scope: Option<&str>) -> budget::Report {
            self.engine
                .budget_report(scope, CostGroupBy::Scope, time("2026-10-16T12:00:00Z"))
                .await
                .unwrap()
        }
    }
    impl Drop for Instance {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn time(raw: &str) -> DateTime<Utc> {
        raw.parse().unwrap()
    }
    fn measured(usd: f64) -> RunUsage {
        RunUsage {
            state: UsageState::Known,
            reason: None,
            cost_usd: Some(usd),
            tokens: TokenCounts {
                input: Some(100),
                output: Some(0),
                cache_read: Some(0),
                cache_write: Some(0),
            },
            ..RunUsage::unknown("", 2)
        }
    }
    fn card<'a>(report: &'a budget::Report, id: &str) -> &'a budget::ScopeBudget {
        report.budgets.iter().find(|c| c.id == id).unwrap()
    }

    #[tokio::test]
    async fn budget_service_constructor_uses_fresh_scope_identity_and_stable_limit_ids() {
        let i = Instance::new();
        i.limits("version: 1\nscopes: {work-id: {monthly_usd: 50}}\n");
        let now = time("2026-10-16T12:00:00Z");
        let first = i
            .engine
            .budget_report(Some("work"), CostGroupBy::Scope, now)
            .await
            .unwrap();
        assert_eq!(card(&first, "work-id").scope, "work");
        let mut scope = i.engine.factory_snapshot().scope("work").unwrap().clone();
        let id = scope.id.clone();
        scope.name = "renamed".into();
        scope.path = "projects/new-home".into();
        i.engine.replace_scope(&id, scope);
        let next = i
            .engine
            .budget_report(Some("projects/new-home"), CostGroupBy::Scope, now)
            .await
            .unwrap();
        let target = card(&next, "work-id");
        assert_eq!(target.scope, "renamed");
        assert_eq!(target.path, "projects/new-home");
        assert_eq!(target.monthly_usd, Some(50.0));
        assert_eq!(target.relation, "selected");
        assert!(
            !next.budgets.iter().any(|b| b.id == "nested-id"),
            "old path child no longer belongs below the moved scope"
        );
        assert!(i
            .engine
            .budget_report(Some("work"), CostGroupBy::Scope, now)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn subtree_caps_are_independent_and_never_sum_overlapping_cards() {
        let i = Instance::new();
        i.limits("version: 1\nscopes:\n  root-id: {monthly_usd: 50}\n  work-id: {monthly_usd: 10}\n  nested-id: {monthly_usd: 20}\n");
        i.run("work", "2026-10-02T00:00:00Z", Some(measured(7.0)))
            .await;
        i.run("nested", "2026-10-03T00:00:00Z", Some(measured(5.0)))
            .await;
        i.run("work-extra", "2026-10-04T00:00:00Z", Some(measured(20.0)))
            .await;
        // Starts before the month, ends in it: not this month's charge.
        i.run("work", "2026-09-30T23:59:59Z", Some(measured(1000.0)))
            .await;
        let r = i.report(Some("work")).await;
        assert_eq!(r.spend.total.cost_usd, 12.0);
        assert_eq!(r.spend.total.runs, 2);
        assert_eq!(card(&r, "work-id").assessment.state, budget::State::Over);
        assert_eq!(
            card(&r, "nested-id").assessment.state,
            budget::State::Within
        );
        assert_eq!(card(&r, "nested-id").monthly_usd, Some(20.0));
        assert_eq!(card(&r, "root-id").spent.cost_usd, 32.0);
        assert_eq!(card(&r, "root-id").relation, "ancestor");
        assert!(!r.budgets.iter().any(|c| c.id == "side-id"));
        assert_eq!(
            r.spend.daily.iter().map(|d| d.spent.cost_usd).sum::<f64>(),
            12.0
        );
        assert_eq!(r.spend.daily[0].day.to_string(), "2026-10-02");
        assert_eq!(i.report(None).await.spend.total.cost_usd, 32.0);
        let nested = i.report(Some("nested")).await;
        assert_eq!(nested.spend.total.cost_usd, 5.0);
        assert_eq!(card(&nested, "work-id").spent.cost_usd, 12.0);
        assert_eq!(card(&nested, "work-id").relation, "ancestor");
    }

    #[tokio::test]
    async fn unmeasured_partial_and_removed_scope_runs_prevent_a_safe_verdict() {
        let i = Instance::new();
        i.limits("version: 1\nscopes: {work-id: {monthly_usd: 50}}\n");
        i.run("work", "2026-10-02T00:00:00Z", Some(measured(10.0)))
            .await;
        i.run("nested", "2026-10-03T00:00:00Z", None).await;
        let mut partial = measured(1.0);
        partial.partial = true;
        i.run("work", "2026-10-04T00:00:00Z", Some(partial)).await;
        let r = i.report(Some("work")).await;
        let c = card(&r, "work-id");
        assert_eq!(c.assessment.state, budget::State::Unknown);
        assert_eq!((c.spent.runs_unknown, c.spent.runs_partial), (1, 1));
        assert!(c.assessment.remaining_usd.is_none());
        i.limits("version: 1\nscopes: {work-id: {monthly_usd: 5}}\n");
        assert_eq!(
            card(&i.report(Some("work")).await, "work-id")
                .assessment
                .state,
            budget::State::Over
        );
        let j = Instance::new();
        j.limits("version: 1\nscopes: {work-id: {monthly_usd: 50}}\n");
        j.run(
            "no-longer-configured",
            "2026-10-05T00:00:00Z",
            Some(measured(30.0)),
        )
        .await;
        let r = j.report(Some("work")).await;
        assert_eq!(r.spend.total.cost_usd, 0.0);
        assert_eq!(r.spend.unattributed_runs, 1);
        assert_eq!(r.spend.daily[0].unattributed_runs, 1);
        assert_eq!(card(&r, "work-id").assessment.state, budget::State::Unknown);
        assert_eq!(j.report(None).await.spend.total.cost_usd, 30.0);
        let id = "cost_week"
            .parse::<factory_core::metrics::MetricId>()
            .unwrap();
        let metric = j
            .engine
            .metrics_for(
                &[id.clone()],
                time("2026-10-06T00:00:00Z"),
                Some("work"),
                None,
            )
            .await
            .unwrap();
        assert!(metric.values[0].value.is_none());
        assert!(metric.values[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("1 unattributed"));
        let metric = j
            .engine
            .metrics(&[id], time("2026-10-06T00:00:00Z"))
            .await
            .unwrap();
        assert_eq!(
            metric.values[0].value,
            Some(30.0),
            "all-instance spend is measured even without scope attribution"
        );
    }

    #[tokio::test]
    async fn intent_is_reread_by_stable_id_and_bad_files_are_not_absent_defaults() {
        let i = Instance::new();
        let r = i.report(None).await;
        assert_eq!(
            card(&r, "work-id").assessment.state,
            budget::State::Unconfigured
        );
        i.limits(
            "version: 1\nscopes:\n  work-id: {monthly_usd: 50}\n  deleted-id: {monthly_usd: 90}\n",
        );
        let r = i.report(Some("work")).await;
        assert_eq!(card(&r, "work-id").monthly_usd, Some(50.0));
        assert!(r.findings[0].contains("deleted-id"));
        i.limits("version: 1\nscopes: {work-id: {monthly_usd: 80}}\n");
        assert_eq!(
            card(&i.report(Some("work")).await, "work-id").monthly_usd,
            Some(80.0)
        );
        i.limits("version: 1\nscopes: {work-id: {monthly_usd: -1}}\n");
        assert!(i
            .engine
            .budget_report(None, CostGroupBy::Scope, Utc::now())
            .await
            .unwrap_err()
            .to_string()
            .contains("nonnegative"));
        assert!(i
            .engine
            .budget_report(Some("nope"), CostGroupBy::Scope, Utc::now())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn exact_month_start_is_empty_and_each_group_reads_the_same_spend_fact() {
        let i = Instance::new();
        i.limits("version: 1\nscopes: {work-id: {monthly_usd: 0}}\n");
        i.run("work", "2026-10-02T00:00:00Z", Some(measured(5.0)))
            .await;
        let now = time("2026-10-01T00:00:00Z");
        let r = i
            .engine
            .budget_report(Some("work"), CostGroupBy::Scope, now)
            .await
            .unwrap();
        assert_eq!(r.spend.total.runs, 0);
        assert_eq!(card(&r, "work-id").assessment.state, budget::State::Within);
        for group_by in [
            CostGroupBy::Scope,
            CostGroupBy::Agent,
            CostGroupBy::Issue,
            CostGroupBy::Workflow,
            CostGroupBy::Provider,
        ] {
            let now = time("2026-10-16T12:00:00Z");
            let r = i
                .engine
                .budget_report(Some("work"), group_by, now)
                .await
                .unwrap();
            let fact = Facts::<L6>::new(&i.engine)
                .get::<CostReport>(&SpendQuery {
                    scope: Some("work".into()),
                    group_by,
                    from: Some(r.month.from),
                    to: Some(now),
                    ..Default::default()
                })
                .await
                .unwrap();
            assert_eq!(r.spend, fact);
        }
    }
}
