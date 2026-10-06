//! Outside request wiring for the live L6 Scenarios owner. Only fresh raw
//! declarations, physical providers, legacy metric error preflights and
//! final task payload hydration remain beside the ladder.
use crate::l6_service::L6Service;
#[cfg(test)]
use crate::engine::Engine;
#[cfg(test)]
use std::sync::Arc;
use chrono::{DateTime, Utc};
#[cfg(test)]
use factory_core::protocol::ScenarioResult;
use factory_core::{
    config::Factory,
    error::Result,
    metrics::MetricId,
    protocol::{
        PromotedControl, ScenarioPromoteResult, ScenarioWhatIfResult, ScenariosReport,
        SkippedControl,
    },
    scenario::DriverId,
};
#[cfg(test)]
use factory_core::{config::Scope, metrics::MetricValue, scenario, task::NewTask};
#[cfg(test)]
use factory_direction::scenarios_service::{
    driver_baseline, driver_result, measured_overrides, open_backlog, open_goal_task_count,
};
#[cfg(test)]
use factory_kernel::TaskInventoryFact;
#[cfg(test)]
use std::collections::BTreeSet;
use std::collections::BTreeMap;

impl<'s> L6Service<'s> {
    fn scenarios_service(
        &self,
        snapshot: &Factory,
    ) -> factory_direction::scenarios_service::Service<'s> {
        factory_direction::scenarios_service::Service::new(
            self.policy_intent_service(snapshot),
            &self.state.goals,
        )
    }

    /// Raw authored query preparation beside the request. L6 never receives
    /// computed metric values; its own plan reads the actual L5 fact port.
    async fn scenario_metric_query(
        &self,
        snapshot: &Factory,
        ids: &[MetricId],
        now: DateTime<Utc>,
        scope: Option<&str>,
    ) -> Result<(factory_assurance::metric_values::Read, bool)> {
        let plan = factory_assurance::metrics_service::Plan::prepare(
            ids,
            snapshot.root.clone(),
            &snapshot.scope_tree(),
            &crate::quality::quality_configuration(snapshot),
            &crate::metrics::reported_configuration(snapshot),
            scope,
        )
        .await?;
        let policy = if plan.needs_policy() {
            self.metric_policy_inputs(snapshot, plan.scope())
                .await
                .map(Some)
        } else {
            Ok(None)
        };
        let has_rows = policy
            .as_ref()
            .ok()
            .and_then(|p| p.as_ref())
            .is_some_and(|p| !p.scopes.is_empty());
        let budgets = self.metric_quality_budgets(snapshot, &plan).await;
        Ok((
            factory_assurance::metric_values::Read {
                plan,
                policy,
                budgets,
                now,
                window: None,
            },
            has_rows,
        ))
    }

    pub(crate) async fn scenarios_report(
        &self,
        scope: Option<&str>,
    ) -> Result<ScenariosReport> {
        let now = Utc::now();
        let snapshot = self.wiring.snapshot();
        let service = self.scenarios_service(&snapshot);
        let knowledge = self.wiring.provider::<factory_kernel::KnowledgeTags>();
        let inventory = self.wiring.provider::<factory_kernel::TaskInventoryFact>();
        let plan = service
            .prepare_report(scope, &knowledge, &inventory, now)
            .await?;
        let selected_scope = plan.scope().map(str::to_string);
        let (query, has_rows) = self
            .scenario_metric_query(&snapshot, plan.metric_ids(), now, plan.scope())
            .await?;
        let provider = self.wiring.provider::<factory_kernel::MetricValuesFact>();
        let measured = plan.read_metrics(&provider, &query).await;
        if provider.policy_was_gathered() {
            self.metric_policy_preflight(&snapshot, selected_scope.as_deref(), has_rows)
                .await?;
        }
        let checks = self.wiring.provider::<factory_kernel::CheckEvaluationFact>();
        let production = self.wiring.provider::<factory_kernel::ProductionFact>();
        service.finish_report(measured?, &checks, &production).await
    }

    pub(crate) async fn scenario_whatif(
        &self,
        scenario_name: Option<String>,
        raw_drivers: BTreeMap<DriverId, String>,
        scope: Option<&str>,
    ) -> Result<ScenarioWhatIfResult> {
        let now = Utc::now();
        let snapshot = self.wiring.snapshot();
        let service = self.scenarios_service(&snapshot);
        let knowledge = self.wiring.provider::<factory_kernel::KnowledgeTags>();
        let checks = self.wiring.provider::<factory_kernel::CheckEvaluationFact>();
        let inventory = self.wiring.provider::<factory_kernel::TaskInventoryFact>();
        let plan = service
            .prepare_whatif(
                scenario_name,
                raw_drivers,
                scope,
                &knowledge,
                &checks,
                &inventory,
                now,
            )
            .await?;
        let selected_scope = plan.scope().map(str::to_string);
        let (query, has_rows) = self
            .scenario_metric_query(&snapshot, plan.metric_ids(), now, plan.scope())
            .await?;
        let provider = self.wiring.provider::<factory_kernel::MetricValuesFact>();
        let measured = plan.read_metrics(&provider, &query).await;
        if provider.policy_was_gathered() {
            self.metric_policy_preflight(&snapshot, selected_scope.as_deref(), has_rows)
                .await?;
        }
        let production = self.wiring.provider::<factory_kernel::ProductionFact>();
        service.finish_whatif(measured?, &production).await
    }

    pub(crate) async fn scenario_promote(
        &self,
        scenario_name: String,
        scope: String,
        agent: Option<String>,
    ) -> Result<ScenarioPromoteResult> {
        let now = Utc::now();
        let snapshot = self.wiring.snapshot();
        let knowledge = self.wiring.provider::<factory_kernel::KnowledgeTags>();
        let comparisons = self.wiring.provider::<factory_kernel::CheckComparisonFact>();
        let observer = crate::commands::CreationObserver(self.wiring.bus().clone());
        let commands = self.wiring.direction(&observer);
        let receipt = self
            .scenarios_service(&snapshot)
            .promote(
                scenario_name,
                scope,
                agent,
                &knowledge,
                &comparisons,
                &commands,
                now,
            )
            .await?;
        let mut created = Vec::new();
        for entry in receipt.created {
            created.push(PromotedControl {
                control: entry.control,
                task: self.wiring.task_snapshot(entry.task.id).await?,
            });
        }
        Ok(ScenarioPromoteResult {
            scenario: receipt.scenario,
            scope: receipt.scope,
            created,
            skipped: receipt
                .skipped
                .into_iter()
                .map(|entry| SkippedControl {
                    control: entry.control,
                    existing_task: entry.existing_task,
                })
                .collect(),
        })
    }

    #[cfg(test)]
    pub(crate) async fn subtree_daily(
        &self,
        asked: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Vec<factory_core::protocol::ProductionBucket>> {
        let snapshot = self.wiring.snapshot();
        let production = self.wiring.provider::<factory_kernel::ProductionFact>();
        self.scenarios_service(&snapshot)
            .subtree_daily(&production, asked, now)
            .await
    }
}
#[cfg(test)]
mod tests {
    //! Engine-level scenario reports and writes, on a temporary instance --
    //! not `factory_core::scenario` itself (covered on its own), but this
    //! module glued to a real scope tree, real policy catalogues (one real,
    //! one draft), a real goals cycle, and the store behind `Engine`, the
    //! way a real request sees them.

    use super::*;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration};
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;

    /// Since #106 `rework_rate` is the registry metric of that name, not a
    /// hardcoded `0.0`: its baseline is the metric's value, and absent --
    /// nothing to be relative to -- when the metric has none.
    #[test]
    fn the_rework_rate_driver_takes_its_baseline_from_the_registry_metric() {
        let id = MetricId::new("rework_rate").unwrap();
        let mut values = BTreeMap::new();
        values.insert(
            id.clone(),
            MetricValue { id: id.clone(), value: Some(0.2), as_of: chrono::Utc::now(), reason: None },
        );
        assert_eq!(driver_baseline(&values).get("rework_rate"), Some(&0.2));
        assert_eq!(driver_baseline(&BTreeMap::new()).get("rework_rate"), None);
        assert_eq!(driver_baseline(&BTreeMap::new()).get("capacity_factor"), Some(&1.0));
    }

    #[test]
    fn measured_subset_or_absent_cost_baselines_cannot_be_overridden_into_a_free_forecast() {
        let now = Utc::now();
        let id = MetricId::new("unit_cost").unwrap();
        let values = BTreeMap::from([(id.clone(), MetricValue { id, value: Some(5.0), as_of: now, reason: Some("one unmeasured finished run".into()) })]);
        let baseline = driver_baseline(&values);
        assert!(!baseline.contains_key("unit_cost"));
        let overridden = measured_overrides(&baseline, &BTreeMap::from([("unit_cost".into(), scenario::Override::Set(0.0))]));
        let drivers = driver_result(&baseline, overridden, &values);
        assert!(!drivers.outcomes_after.contains_key("weekly_cost"));
        assert!(!drivers.tornados.contains_key("weekly_cost"));
        assert!(drivers.outcome_reasons_after["weekly_cost"].contains("unmeasured"));
    }

    fn scope_at(id: &str, name: &str, path: &str) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(&format!("id: {id}\nname: {name}\n")).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    const CRA_YAML: &str = "framework: cra\n\
         title: Cyber Resilience Act\n\
         kind: regulation\n\
         controls:\n\
         \x20\x20- id: a\n\x20\x20\x20\x20title: Control A\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: knowledge\n\
         \x20\x20- id: b\n\x20\x20\x20\x20title: Control B\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: attestation\n";

    const AI_ACT_DRAFT_YAML: &str = "framework: ai-act\n\
         title: EU AI Act (draft)\n\
         kind: regulation\n\
         controls:\n\
         \x20\x20- id: oversight\n\x20\x20\x20\x20title: Human oversight\n\x20\x20\x20\x20remediation: |\n\x20\x20\x20\x20\x20\x20Record an attestation.\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: attestation\n";

    const SCENARIO_YAML: &str = "name: ai-act-2027\n\
         title: EU AI Act applies from 2027\n\
         kind: [policy, drivers, goals]\n\
         horizon: 13w\n\
         policy:\n\
         \x20\x20add_frameworks: [ai-act]\n\
         drivers:\n\
         \x20\x20capacity_factor: \"-20%\"\n\
         goals:\n\
         \x20\x20- { kr: obj/kr-ratio, by: 2027-01-01 }\n\
         signposts:\n\
         \x20\x20- { metric: throughput_week, below: 999 }\n";

    const GOALS_CYCLE_YAML: &str = "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
         objectives:\n\
         \x20\x20- id: obj\n\x20\x20\x20\x20title: Objective\n\x20\x20\x20\x20key_results:\n\
         \x20\x20\x20\x20\x20\x20- { id: kr-ratio, title: KR, kind: committed, metric: first_pass_yield, baseline: 0, target: 1 }\n";

    /// `company` (`.`) is root and commits the instance to `cra`, exactly
    /// like `policies::tests::test_engine`. `projects` and, below it,
    /// `demo-app` sit under it; `sibling` sits outside `projects` entirely,
    /// to prove a scope query never reaches sideways (the same shape every
    /// other module's own subtree test uses). One scenario,
    /// `ai-act-2027.yaml`, overlays the `ai-act` draft catalogue
    /// (`policies/drafts/ai-act.yaml`), a driver override, one `goals:`
    /// entry naming a real (ratio-shaped) key result, and one signpost that
    /// is always triggered (`throughput_week` starts at `0`, `below: 999`).
    fn test_engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-scenarios-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory/policies/drafts")).unwrap();
        std::fs::write(root.join(".factory/policies/cra.yaml"), CRA_YAML).unwrap();
        std::fs::write(root.join(".factory/policies/drafts/ai-act.yaml"), AI_ACT_DRAFT_YAML).unwrap();

        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::write(root.join(".factory/knowledge/page.md"), "---\ntags: [control/cra/a]\n---\n# Page\n").unwrap();

        std::fs::create_dir_all(root.join(".factory/scenarios")).unwrap();
        std::fs::write(root.join(".factory/scenarios/ai-act-2027.yaml"), SCENARIO_YAML).unwrap();

        std::fs::create_dir_all(root.join(".factory/goals")).unwrap();
        std::fs::write(root.join(".factory/goals/2026-q4.yaml"), GOALS_CYCLE_YAML).unwrap();

        let mut config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: None,
            scopes: Vec::new(),
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration { frameworks: vec!["cra".to_string()], ..Default::default() },
            quality: Default::default(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        config.scopes = vec![
            scope_at("company-id", "company", "."),
            scope_at("projects-id", "projects", "projects"),
            scope_at("demo-app-id", "demo-app", "projects/demo"),
            scope_at("sibling-id", "sibling", "other"),
        ];

        let factory = Factory { root, config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    fn find<'a>(report: &'a ScenariosReport, name: &str) -> &'a ScenarioResult {
        report.scenarios.iter().find(|s| s.scenario.name == name).unwrap_or_else(|| panic!("no scenario {name:?} in {report:#?}"))
    }

    // -- policy delta: real catalogue + draft --------------------------------

    #[tokio::test]
    async fn policy_delta_is_exact_against_a_real_catalogue_and_a_draft() {
        let engine = test_engine();
        let report = engine.scenarios_report(Some("company")).await.unwrap();
        let scenario = find(&report, "ai-act-2027");

        // The real catalogue's own controls are unaffected: `a` stays
        // satisfied (the knowledge tag), `b` stays open (no attestation) --
        // neither shows up as newly anything.
        let newly_open: Vec<String> = scenario.policy_subtree.newly_open.iter().map(|c| c.to_string()).collect();
        assert_eq!(newly_open, vec!["ai-act/oversight".to_string()], "{:#?}", scenario.policy_subtree);
        assert!(scenario.policy_subtree.newly_applicable_but_covered.is_empty());

        // Every scope in the subtree gets its own delta row, since the
        // draft framework applies everywhere the overlay was evaluated.
        let scopes: std::collections::BTreeSet<&str> = scenario.policy.iter().map(|d| d.scope.as_str()).collect();
        assert_eq!(scopes, std::collections::BTreeSet::from(["company", "projects", "demo-app", "sibling"]));
        for row in &scenario.policy {
            assert_eq!(
                row.delta.newly_open.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
                vec!["ai-act/oversight".to_string()],
                "scope {} should see the same newly-open control",
                row.scope
            );
        }

        // The baseline itself never sees the draft framework at all.
        assert!(report.baseline.policy.iter().all(|r| r.framework != "ai-act"), "{:#?}", report.baseline.policy);
        assert!(report.baseline.policy.iter().any(|r| r.framework == "cra"));

        // A draft that never collided with a real catalogue is not a
        // finding; report-level findings are all about this scenario file
        // or the loader, never a phantom collision.
        assert!(report.findings.iter().all(|f| f.kind != factory_core::scenario::FindingKind::DraftCollidesWithReal));
    }

    #[tokio::test]
    async fn a_draft_that_collides_with_a_real_framework_is_dropped_and_reported() {
        let engine = test_engine();
        // A second draft, alongside `drafts/ai-act.yaml`, whose own
        // `framework` collides with the real `cra` catalogue -- its file
        // stem has to match its own `framework` (`policy::load_all`'s own
        // rule, which `load_drafts` shares) for it to load at all.
        let snapshot = engine.factory_snapshot();
        std::fs::write(
            snapshot.root.join(".factory/policies/drafts/cra.yaml"),
            "framework: cra\ntitle: Fake CRA\nkind: regulation\ncontrols: []\n",
        )
        .unwrap();

        let report = engine.scenarios_report(None).await.unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == factory_core::scenario::FindingKind::DraftCollidesWithReal && f.subject == "cra.yaml"),
            "{:#?}",
            report.findings
        );
        // The colliding draft never reaches the merged set at all, but the
        // unrelated `ai-act` draft the scenario actually names still does --
        // one bad draft never stops another from loading, the same
        // "one file never takes the rest down" rule every loader here holds.
        let scenario = find(&report, "ai-act-2027");
        assert_eq!(
            scenario.policy_subtree.newly_open.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
            vec!["ai-act/oversight".to_string()],
            "{:#?}",
            scenario.policy_subtree
        );
    }

    // -- determinism ----------------------------------------------------------

    #[tokio::test]
    async fn the_report_is_deterministic_for_the_same_inputs() {
        let engine = test_engine();
        let a = engine.scenarios_report(Some("company")).await.unwrap();
        let b = engine.scenarios_report(Some("company")).await.unwrap();

        let sa = find(&a, "ai-act-2027");
        let sb = find(&b, "ai-act-2027");
        assert_eq!(sa.forecast.per_week, sb.forecast.per_week);
        assert_eq!(sa.forecast.completion_week, sb.forecast.completion_week);
        assert_eq!(sa.forecast.seed, sb.forecast.seed);
        assert_eq!(sa.drivers.tornado, sb.drivers.tornado);
        assert_eq!(sa.drivers.outcomes_before, sb.drivers.outcomes_before);
        assert_eq!(sa.drivers.outcomes_after, sb.drivers.outcomes_after);
        assert_eq!(sa.goals.len(), sb.goals.len());
        for (ga, gb) in sa.goals.iter().zip(&sb.goals) {
            assert_eq!(ga.probability, gb.probability);
        }
        assert_eq!(sa.backlog.total, sb.backlog.total);
        assert_eq!(a.baseline.forecast.per_week, b.baseline.forecast.per_week);
    }

    // -- goal scenarios: ratio KRs never fabricate a probability -------------

    #[tokio::test]
    async fn a_ratio_key_result_re_scores_to_none_with_a_reason_never_a_number() {
        let engine = test_engine();
        let report = engine.scenarios_report(None).await.unwrap();
        let scenario = find(&report, "ai-act-2027");
        assert_eq!(scenario.goals.len(), 1);
        let g = &scenario.goals[0];
        assert_eq!(g.kr.to_string(), "obj/kr-ratio");
        assert_eq!(g.probability.probability, None);
        assert!(g.probability.reason.is_some(), "{:?}", g.probability);
    }

    // -- signposts --------------------------------------------------------------

    #[tokio::test]
    async fn a_triggered_signpost_is_surfaced_at_the_top_level() {
        let engine = test_engine();
        let report = engine.scenarios_report(None).await.unwrap();
        let scenario = find(&report, "ai-act-2027");
        assert_eq!(scenario.signposts[0].state, scenario::SignpostState::Triggered, "{:?}", scenario.signposts);

        assert!(
            report.triggered.iter().any(|t| t.scenario == "ai-act-2027" && t.metric.as_str() == "throughput_week"),
            "{:#?}",
            report.triggered
        );
    }

    // -- scope filter -----------------------------------------------------------

    #[tokio::test]
    async fn scope_filter_matches_the_asked_scope_and_its_descendants_only() {
        let engine = test_engine();
        let report = engine.scenarios_report(Some("projects")).await.unwrap();
        let scenario = find(&report, "ai-act-2027");
        let scopes: std::collections::BTreeSet<&str> = scenario.policy.iter().map(|d| d.scope.as_str()).collect();
        assert_eq!(
            scopes,
            std::collections::BTreeSet::from(["projects", "demo-app"]),
            "company (an ancestor) and sibling (unrelated) must not appear"
        );
    }

    // -- promote ------------------------------------------------------------

    #[tokio::test]
    async fn promote_creates_a_task_per_newly_open_control_and_skips_it_next_time() {
        let engine = test_engine();
        let result = engine.scenario_promote("ai-act-2027".to_string(), "company".to_string(), None).await.unwrap();

        assert_eq!(result.created.len(), 1, "{:#?}", result.created);
        assert!(result.skipped.is_empty());
        let created = &result.created[0];
        assert_eq!(created.control.to_string(), "ai-act/oversight");
        assert_eq!(created.task.title, "Prepare ai-act/oversight for scenario ai-act-2027: Human oversight");
        assert_eq!(created.task.labels.get("policy").map(String::as_str), Some("ai-act/oversight"));
        assert_eq!(created.task.labels.get("scenario").map(String::as_str), Some("ai-act-2027"));
        assert!(created.task.instructions.contains("Record an attestation"), "{:?}", created.task.instructions);
        assert!(created.task.instructions.contains("Missing evidence"), "{:?}", created.task.instructions);

        // A second promote finds the same open task rather than creating a
        // duplicate.
        let again = engine.scenario_promote("ai-act-2027".to_string(), "company".to_string(), None).await.unwrap();
        assert!(again.created.is_empty(), "{:#?}", again.created);
        assert_eq!(again.skipped.len(), 1);
        assert_eq!(again.skipped[0].control.to_string(), "ai-act/oversight");
        assert_eq!(again.skipped[0].existing_task, created.task.id);
    }

    /// `#122`: a promoted task whose run failed is blocked, not closed, so
    /// the next promote still skips it rather than making a duplicate.
    #[tokio::test]
    async fn promote_skips_a_failed_task_rather_than_creating_a_duplicate() {
        let engine = test_engine();
        let result = engine.scenario_promote("ai-act-2027".to_string(), "company".to_string(), None).await.unwrap();
        let created = &result.created[0];
        engine.fail_task_for_test(&created.task.id, factory_core::run::FailKind::RunTimeout).await;

        let again = engine.scenario_promote("ai-act-2027".to_string(), "company".to_string(), None).await.unwrap();
        assert!(again.created.is_empty(), "{:#?}", again.created);
        assert_eq!(again.skipped.len(), 1);
        assert_eq!(again.skipped[0].existing_task, created.task.id);
    }

    /// `#122`: the goal-task count and the baseline backlog both count a
    /// task blocked by a failure as open work -- it is -- and a closed one
    /// as not.
    #[test]
    fn open_work_counts_a_failed_task_and_not_a_closed_one() {
        use factory_core::task::TaskStatus;
        let task = |id: &str, status: TaskStatus, failed: bool| {
            let mut t = factory_core::adapter::store::task_from_new(
                factory_core::task::NewTask { title: id.into(), ..Default::default() },
                "demo".into(),
                "shell".into(),
                "herdr".into(),
            );
            t.id = id.into();
            t.status = status;
            t.labels.insert("goal".into(), "ship/kr1".into());
            if failed {
                t.failure = Some(factory_core::task::TaskFailure {
                    kind: Some(factory_core::run::FailKind::AgentFailed),
                    run_id: Some("r".into()),
                    attempt: Some(1),
                    at: chrono::Utc::now(),
                });
            }
            TaskInventoryFact {
                open: !t.status.is_terminal(), id: t.id, title: t.title,
                scope: t.scope, labels: t.labels,
            }
        };
        let tasks = vec![
            task("failed", TaskStatus::Blocked, true),
            task("done", TaskStatus::Done, false),
            task("wont", TaskStatus::Cancelled, false),
            task("pending", TaskStatus::Pending, false),
        ];
        let change = scenario::GoalChange { kr: "ship/kr1".parse().unwrap(), target: None, by: None };
        assert_eq!(open_goal_task_count(&tasks, &[change]), 2);
        let targets = BTreeSet::from(["demo"]);
        assert_eq!(open_backlog(&tasks, &targets), 2.0);
    }

    #[tokio::test]
    async fn promote_refuses_an_unknown_scenario() {
        let engine = test_engine();
        let err = engine.scenario_promote("nope".to_string(), "company".to_string(), None).await.unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
    }

    // -- subtree_daily: throughput from a descendant reaches an ancestor's own report --

    async fn finished_run_in(engine: &Arc<Engine>, scope: &str, label: &str) {
        let new_task = factory_core::adapter::store::task_from_new(
            NewTask { title: label.to_string(), ..Default::default() },
            scope.to_string(),
            "assistant".to_string(),
            "shell".to_string(),
        );
        let task = engine.l4.store.create(&new_task).await.unwrap();
        let run = engine
            .l4.store
            .create_run(&factory_core::run::NewRun {
                task_id: task.id.clone(),
                trigger: factory_core::run::Trigger::Manual,
                agent: "assistant".to_string(),
                adapter: "shell".to_string(),
                runtime: "shell".to_string(),
                token: "tok".to_string(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .l4.store
            .update_run(
                &run.id,
                &factory_core::run::RunPatch {
                    status: Some(factory_core::run::RunStatus::Done),
                    ended_at: Some(Utc::now() - chrono::Duration::hours(1)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    /// `Engine::production`'s own scope filter is exact-match only; a
    /// request scoped to `projects` (an ancestor of `demo-app`, the scope
    /// that actually did the work) must still see that throughput in its
    /// own baseline forecast -- see `Engine::subtree_daily`'s own doc
    /// comment for why summing per target scope is what makes that true,
    /// unlike a single `production(scope: "projects")` call.
    #[tokio::test]
    async fn a_scope_scoped_report_sees_a_descendants_own_throughput() {
        let engine = test_engine();
        for i in 0..3 {
            finished_run_in(&engine, "demo-app", &format!("run-{i}")).await;
        }

        let daily = engine.subtree_daily(Some("projects"), chrono::Utc::now()).await.unwrap();
        let total: u32 = daily.iter().map(|b| b.finished).sum();
        assert_eq!(total, 3, "the ancestor's own subtree_daily must include its descendant's throughput");

        // Pin the bug this method exists to avoid regressing: an exact-match
        // call at the ancestor alone sees none of it.
        let exact_only = engine.production(None, None, Some("projects".to_string())).await.unwrap().daily;
        let exact_total: u32 = exact_only.iter().map(|b| b.finished).sum();
        assert_eq!(exact_total, 0, "an exact-match production call at the ancestor alone sees none of the descendant's work");

        // And it shows up in the real report's own baseline forecast, not
        // just in the grid this test reaches into directly.
        let report = engine.scenarios_report(Some("projects")).await.unwrap();
        assert!(report.baseline.forecast.reason.is_none(), "{:?}", report.baseline.forecast.reason);
    }

    // -- whatif ---------------------------------------------------------------

    #[tokio::test]
    async fn whatif_layers_request_drivers_over_the_named_scenarios_own() {
        let engine = test_engine();

        // With no request-level override, the scenario's own "-20%" holds.
        let named = engine.scenario_whatif(Some("ai-act-2027".to_string()), Default::default(), None).await.unwrap();
        let baseline_capacity = 1.0; // capacity_factor's own neutral baseline
        assert!((named.drivers.overridden["capacity_factor"] - baseline_capacity * 0.8).abs() < 1e-9, "{:#?}", named.drivers);

        // A request-level override for the same driver wins outright.
        let mut overrides = BTreeMap::new();
        overrides.insert("capacity_factor".to_string(), "=2.0".to_string());
        let overridden = engine.scenario_whatif(Some("ai-act-2027".to_string()), overrides, None).await.unwrap();
        assert_eq!(overridden.drivers.overridden["capacity_factor"], 2.0);

        // With no scenario named at all, only the request's own overrides
        // apply, and the forecast has nothing to clear.
        let mut bare = BTreeMap::new();
        bare.insert("capacity_factor".to_string(), "=3.0".to_string());
        let none_named = engine.scenario_whatif(None, bare, None).await.unwrap();
        assert_eq!(none_named.drivers.overridden["capacity_factor"], 3.0);
        assert_eq!(none_named.forecast.completion_week.p50, Some(0), "nothing to clear with no scenario named");
    }

    #[tokio::test]
    async fn whatif_refuses_a_bad_override_syntax() {
        let engine = test_engine();
        let mut overrides = BTreeMap::new();
        overrides.insert("capacity_factor".to_string(), "banana".to_string());
        let err = engine.scenario_whatif(None, overrides, None).await.unwrap_err();
        assert!(err.to_string().contains("capacity_factor"), "{err}");
    }

    // -- #156: backup metrics as signposts ---------------------------------
    //
    // `backup_age_hours`/`backup_verified_age_days` (`#154`) are registry
    // metrics like any other -- no scenario code changed to support them.
    // These are lock tests proving the wiring holds: a scenario naming one
    // loads with no `UnknownMetric` finding, and `evaluate_signpost` (tested
    // on its own in `factory_core::scenario`) reads a real value through it.

    /// A throwaway instance with `infrastructure.backup` configured and one
    /// scenario, `backup-watch`, whose only signpost is
    /// `{ metric: backup_age_hours, above: 30 }`.
    fn backup_signpost_test_engine(destination: &str) -> Arc<Engine> {
        let base = std::env::temp_dir().join(format!("factory-scenarios-backup-signpost-{}", uuid::Uuid::new_v4()));
        let root = base.join("instance");
        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::write(root.join(".factory/config.yaml"), "version: 1\ninstance:\n  id: test\n  name: test\n").unwrap();
        std::fs::write(root.join(".factory/knowledge/page.md"), "# A page\n").unwrap();
        std::fs::create_dir_all(root.join(".factory/scenarios")).unwrap();
        std::fs::write(
            root.join(".factory/scenarios/backup-watch.yaml"),
            "name: backup-watch\ntitle: Backup age\nsignposts:\n  - { metric: backup_age_hours, above: 30 }\n",
        )
        .unwrap();
        let database = root.join(".factory/factory.sqlite");
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&database).unwrap());
        let mut company: Scope = serde_yaml_ng::from_str("id: company-id\nname: company\n").unwrap();
        company.path = PathBuf::from(".");
        let destination = base.join(destination);
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(company.clone()),
            scopes: vec![company],
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration::default(),
            quality: Default::default(),
            infrastructure: serde_yaml_ng::from_str(&format!(
                "backup:\n  destination: {}\n  keep: {{ daily: 7 }}\n",
                destination.display()
            ))
            .unwrap(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let engine = Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new())
            .with_backup_store(crate::backup::BackupStore::open(&database).unwrap());
        Arc::new(engine)
    }

    #[tokio::test]
    async fn a_signpost_on_backup_age_hours_loads_with_no_unknown_metric_finding_and_is_quiet_for_a_fresh_backup() {
        let engine = backup_signpost_test_engine("destination");
        engine.backup_run(factory_core::backup::BackupTrigger::Manual, "owner".into()).await.unwrap();

        let report = engine.scenarios_report(None).await.unwrap();
        assert!(
            !report.findings.iter().any(|f| f.kind == scenario::FindingKind::UnknownMetric),
            "{:?}",
            report.findings
        );
        let result = find(&report, "backup-watch");
        let sp = result.signposts.iter().find(|s| s.metric.as_str() == "backup_age_hours").unwrap();
        assert_eq!(sp.state, scenario::SignpostState::Quiet, "{sp:?}");
        assert!(!report.triggered.iter().any(|t| t.scenario == "backup-watch"), "{:?}", report.triggered);
    }

    #[tokio::test]
    async fn a_signpost_on_backup_age_hours_triggers_once_the_newest_snapshot_is_old_enough() {
        let engine = backup_signpost_test_engine("destination");
        let snapshot = engine.backup_run(factory_core::backup::BackupTrigger::Manual, "owner".into()).await.unwrap();

        // `list_archives` reads a snapshot's age off its file name, never
        // its mtime, so renaming it back 31 hours -- past the signpost's
        // `above: 30` -- is enough, the same way `#152`'s own
        // `retention_prunes_across_a_mixed_plaintext_and_encrypted_history`
        // fixture plants an aged archive.
        let old_name = factory_core::backup::archive_name("test", snapshot.at - chrono::Duration::hours(31), false);
        let destination = snapshot.path.rsplit_once('/').unwrap().0.to_string();
        std::fs::rename(&snapshot.path, format!("{destination}/{old_name}")).unwrap();

        let report = engine.scenarios_report(None).await.unwrap();
        let result = find(&report, "backup-watch");
        let sp = result.signposts.iter().find(|s| s.metric.as_str() == "backup_age_hours").unwrap();
        assert_eq!(sp.state, scenario::SignpostState::Triggered, "{sp:?}");
        assert!(report.triggered.iter().any(|t| t.scenario == "backup-watch"), "{:?}", report.triggered);

        let live = engine.triggered_signposts(chrono::Utc::now()).await.unwrap();
        assert!(live.iter().any(|t| t.scenario == "backup-watch"), "{live:?}");
    }

    // -- #158: gate_fail_rate as a signpost ----------------------------------
    //
    // `gate_fail_rate` is a registry metric like any other -- no scenario
    // code changed to support it. A lock test proving the wiring holds, the
    // same shape the backup signpost tests above use.

    /// A one-scope instance with one scenario, `gate-watch`, whose only
    /// signpost is `{ metric: gate_fail_rate, above: 0.5 }`.
    fn gate_fail_rate_signpost_test_engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!(
            "factory-scenarios-gate-signpost-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join(".factory/scenarios")).unwrap();
        std::fs::write(
            root.join(".factory/scenarios/gate-watch.yaml"),
            "name: gate-watch\ntitle: Gate failures\nsignposts:\n  - { metric: gate_fail_rate, above: 0.5 }\n",
        )
        .unwrap();
        let mut company: Scope =
            serde_yaml_ng::from_str("id: company-id\nname: company\n").unwrap();
        company.path = PathBuf::from(".");
        let config = Config {
            version: 1,
            instance: Instance {
                id: "test".into(),
                name: "test".into(),
            },
            daemon: DaemonConfig::default(),
            scope: Some(company.clone()),
            scopes: vec![company],
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(
            factory,
            Registry::with_builtins(),
            store,
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    #[tokio::test]
    async fn a_signpost_on_gate_fail_rate_is_quiet_with_no_runs_and_triggers_once_a_gate_fails() {
        use factory_core::adapter::store::task_from_new;
        use factory_core::control_plan::{
            AttestationVerdict, RequiredStep, StepAttestation, StepKind, GATE_ACTOR,
        };
        use factory_core::run::{NewRun, RunPatch, RunStatus, Trigger};
        use factory_core::task::NewTask;

        let engine = gate_fail_rate_signpost_test_engine();

        // No runs at all yet: `gate_fail_rate` has no value, so the
        // signpost reads `no_data`, never `triggered`.
        let report = engine.scenarios_report(None).await.unwrap();
        let result = find(&report, "gate-watch");
        let sp = result
            .signposts
            .iter()
            .find(|s| s.metric.as_str() == "gate_fail_rate")
            .unwrap();
        assert_eq!(sp.state, scenario::SignpostState::NoData, "{sp:?}");
        assert!(
            !report.triggered.iter().any(|t| t.scenario == "gate-watch"),
            "{:?}",
            report.triggered
        );

        // One held run whose gate failed: `gate_fail_rate` is 1.0.
        let task = engine
            .l4.store
            .create(&task_from_new(
                NewTask {
                    title: "t".into(),
                    category: Some("feature".into()),
                    ..Default::default()
                },
                "company".into(),
                "worker".into(),
                "shell".into(),
            ))
            .await
            .unwrap();
        let run = engine
            .l4.store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "worker".into(),
                adapter: "shell".into(),
                runtime: "shell".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        let required = vec![RequiredStep {
            step: "tests".into(),
            kind: StepKind::Gate,
            command: Some("true".into()),
            timeout_seconds: None,
            required_by: Vec::new(),
            node_id: None,
            actor: None,
            by: None,
        }];
        let now = chrono::Utc::now();
        let run = engine
            .l4.store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Done),
                    ended_at: Some(now),
                    required_steps: Some(required),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        engine
            .l4.run_evidence
            .append_step_attestation(&StepAttestation {
                id: uuid::Uuid::new_v4().to_string(),
                run_id: run.id.clone(),
                task_id: task.id.clone(),
                scope: "company".into(),
                category: "feature".into(),
                step: "tests".into(),
                kind: StepKind::Gate,
                actor: GATE_ACTOR.to_string(),
                verdict: AttestationVerdict::Fail,
                required_by: Vec::new(),
                command: Some("true".into()),
                exit_code: Some(1),
                output: None,
                dir: "/tmp".into(),
                commit: None,
                dirty: None,
                node_id: None,
                at: now,
                findings: None,
                round: 0,
                worktree_digest: None,
            })
            .await
            .unwrap();

        let report = engine.scenarios_report(None).await.unwrap();
        let result = find(&report, "gate-watch");
        let sp = result
            .signposts
            .iter()
            .find(|s| s.metric.as_str() == "gate_fail_rate")
            .unwrap();
        assert_eq!(sp.state, scenario::SignpostState::Triggered, "{sp:?}");
        assert!(
            report.triggered.iter().any(|t| t.scenario == "gate-watch"),
            "{:?}",
            report.triggered
        );

        let live = engine
            .triggered_signposts(chrono::Utc::now())
            .await
            .unwrap();
        assert!(live.iter().any(|t| t.scenario == "gate-watch"), "{live:?}");
    }
}
