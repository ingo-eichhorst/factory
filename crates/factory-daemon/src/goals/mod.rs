//! Outside-stack request wiring for the physical L6 Goals service.
//! Current raw identities and actual L5 providers are constructed here;
//! authored catalogue interpretation, scoring and check-ins stay in L6.
mod store;
use crate::{access::Caller, l6_service::L6Service};
#[cfg(test)]
use crate::engine::Engine;
#[cfg(test)]
use std::sync::Arc;
use chrono::Utc;
#[cfg(test)]
use factory_core::{
    config::{Factory, Scope},
    goals,
};
use factory_core::{
    error::Result,
    goals::{CheckIn, KrRef},
    protocol::GoalsReport,
};
pub use store::GoalsStore;

impl L6Service<'_> {
    pub(crate) async fn goals_report(
        &self,
        scope: Option<&str>,
        cycle_id: Option<&str>,
    ) -> Result<GoalsReport> {
        let now = Utc::now();
        let snapshot = self.wiring.snapshot();
        // `#278`: every scope's own declared `reported.*` direction, so
        // the WrongDirection check judges a reported key result the same
        // way it already judges a built-in one. `metrics::resolve` cannot
        // know this itself (it is pure); this is the one live read that
        // hands it in, fresh, the same way `reported_configuration` below
        // feeds the metric values themselves.
        let reported_catalogue = factory_assurance::reported::validate(
            &snapshot.root,
            &crate::metrics::reported_configuration(&snapshot),
        );
        let reported_directions = factory_assurance::reported::directions(&reported_catalogue);
        let service = factory_direction::goals_service::Service::new(
            snapshot.root.clone(),
            snapshot.scope_tree(),
            snapshot.config.scope.as_ref().map(|s| s.name.clone()),
            &self.state.goals,
            reported_directions,
        );
        let plan = service.prepare(scope, cycle_id, now).await?;
        let metric_plan = factory_assurance::metrics_service::Plan::prepare(
            &plan.metric_ids(),
            snapshot.root.clone(),
            &snapshot.scope_tree(),
            &crate::quality::quality_configuration(&snapshot),
            &crate::metrics::reported_configuration(&snapshot),
            None,
        )
        .await?;
        let needs_policy = metric_plan.needs_policy();
        let policy = if needs_policy {
            self.metric_policy_inputs(&snapshot, None).await.map(Some)
        } else {
            Ok(None)
        };
        let has_rows = policy
            .as_ref()
            .ok()
            .and_then(|p| p.as_ref())
            .is_some_and(|p| !p.scopes.is_empty());
        let budgets = self.metric_quality_budgets(&snapshot, &metric_plan).await;
        let query = factory_assurance::metric_values::Read {
            plan: metric_plan,
            policy,
            budgets,
            now,
            window: None,
        };
        let provider = self.wiring.provider::<factory_kernel::MetricValuesFact>();
        let measured = plan.read_metrics(&provider, &query).await;
        // The old request ran these outside-only checks after policy gather
        // and before final metrics. Preserve their priority over a final-read
        // error without passing a callback or a preflight result into L5/L6.
        if provider.policy_was_gathered() {
            self.metric_policy_preflight(&snapshot, None, has_rows)
                .await?;
        }
        service.finish(measured?).await
    }
    pub(crate) async fn goals_checkin(
        &self,
        caller: &Caller,
        kr: KrRef,
        value: f64,
        confidence: u8,
        note: Option<String>,
    ) -> Result<CheckIn> {
        let snapshot = self.wiring.snapshot();
        factory_direction::goals_service::Service::new(
            snapshot.root.clone(),
            snapshot.scope_tree(),
            snapshot.config.scope.as_ref().map(|s| s.name.clone()),
            &self.state.goals,
            // A check-in only needs to find the key result and confirm it
            // is manual -- it never reads WrongDirection, so there is
            // nothing for a live direction map to change here.
            Default::default(),
        )
        .checkin(
            kr,
            value,
            confidence,
            note,
            crate::policies::caller_name(caller),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    //! Engine-level report and check-in paths, on a temporary instance --
    //! not `factory_core::goals` itself (covered on its own), but this
    //! module glued to a real scope tree and a real `GoalsStore` the way a
    //! real request sees them.

    use super::*;
    use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration};
    use factory_plugins::{Registry, SqliteStore};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn scope_at(id: &str, name: &str, path: &str) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(&format!("id: {id}\nname: {name}\n")).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    fn write(dir: &std::path::Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    /// `company` (`.`) is root; `projects` (`projects`) and `demo`
    /// (`projects/demo`) sit under it; `sibling` (`other`) sits outside
    /// `projects` entirely -- the same shape `policies/mod.rs`'s own
    /// `test_engine` uses, for the same reason: a scope query must never
    /// reach sideways.
    fn test_engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-goals-daemon-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let company = scope_at("company-id", "company", ".");
        let projects = scope_at("projects-id", "projects", "projects");
        let demo = scope_at("demo-id", "demo", "projects/demo");
        let sibling = scope_at("sibling-id", "sibling", "other");
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(company.clone()),
            scopes: vec![company, projects, demo, sibling],
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
        let registry = Registry::with_builtins();
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    const CYCLE: &str = "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
         objectives:\n\
         \x20\x20- id: root-obj\n\x20\x20\x20\x20title: Root objective\n\x20\x20\x20\x20key_results:\n\
         \x20\x20\x20\x20\x20\x20- {id: kr, title: KR, kind: committed, manual: true, baseline: 0, target: 1}\n\
         \x20\x20- id: proj-obj\n\x20\x20\x20\x20title: Projects objective\n\x20\x20\x20\x20scope: projects\n\x20\x20\x20\x20key_results:\n\
         \x20\x20\x20\x20\x20\x20- {id: kr, title: KR, kind: committed, manual: true, baseline: 0, target: 1}\n\
         \x20\x20- id: demo-obj\n\x20\x20\x20\x20title: Demo objective\n\x20\x20\x20\x20scope: demo\n\x20\x20\x20\x20key_results:\n\
         \x20\x20\x20\x20\x20\x20- {id: kr, title: KR, kind: committed, manual: true, baseline: 0, target: 1}\n\
         \x20\x20- id: sib-obj\n\x20\x20\x20\x20title: Sibling objective\n\x20\x20\x20\x20scope: sibling\n\x20\x20\x20\x20key_results:\n\
         \x20\x20\x20\x20\x20\x20- {id: kr, title: KR, kind: committed, manual: true, baseline: 0, target: 1}\n";

    /// `demo` (`projects/demo`) sits below `projects`, which sits below the
    /// root `company` -- so a query narrowed to a scope shows objectives
    /// scoped to it *or below* (`scope::ancestors_of`), the same "roll up
    /// the subtree" direction `policy_report`'s own `target_scopes` filter
    /// runs, never the other way: a leaf's query does not also surface its
    /// ancestor's own objectives.
    #[tokio::test]
    async fn scope_filter_includes_the_asked_scope_and_its_descendants_only() {
        let engine = test_engine();
        write(&goals::goals_dir(&engine.factory_snapshot().root), "2026-q4.yaml", CYCLE);

        let whole = engine.goals_report(None, Some("2026-q4")).await.unwrap();
        let ids: BTreeSet<&str> = whole.report.as_ref().unwrap().objectives.iter().map(|o| o.objective.as_str()).collect();
        assert_eq!(ids, BTreeSet::from(["root-obj", "proj-obj", "demo-obj", "sib-obj"]), "no scope named is every scope");

        let narrowed = engine.goals_report(Some("projects"), Some("2026-q4")).await.unwrap();
        let ids: BTreeSet<&str> = narrowed.report.as_ref().unwrap().objectives.iter().map(|o| o.objective.as_str()).collect();
        assert_eq!(
            ids,
            BTreeSet::from(["proj-obj", "demo-obj"]),
            "projects and its descendant demo are both included; root-obj (an ancestor) and \
             sib-obj (a sibling) are not"
        );

        let demo = engine.goals_report(Some("demo"), Some("2026-q4")).await.unwrap();
        let ids: BTreeSet<&str> = demo.report.as_ref().unwrap().objectives.iter().map(|o| o.objective.as_str()).collect();
        assert_eq!(
            ids,
            BTreeSet::from(["demo-obj"]),
            "demo's own query does not also surface projects' (its ancestor's) objective"
        );

        let root = engine.goals_report(Some("company"), Some("2026-q4")).await.unwrap();
        let ids: BTreeSet<&str> = root.report.as_ref().unwrap().objectives.iter().map(|o| o.objective.as_str()).collect();
        assert_eq!(ids, BTreeSet::from(["root-obj", "proj-obj", "demo-obj", "sib-obj"]), "the root scope's own query is everything");
    }

    /// `#278`: a committed key result's `metric:` can name a scope's own
    /// `reported.<source>.<metric>` id the same way it names a built-in
    /// one -- `goals_service` reads whatever `metrics::resolve` and
    /// `metrics_service` hand back, with no special case for this family.
    #[tokio::test]
    async fn a_committed_key_result_can_read_a_reported_metric_the_same_as_a_built_in_one() {
        let root = std::env::temp_dir()
            .join(format!("factory-goals-reported-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut company = scope_at("company-id", "company", ".");
        company.metrics = Some(factory_core::config::ScopeMetricsDeclaration {
            source: Some(factory_core::config::ScopeMetricsSource {
                id: Some("demo".into()),
                file: Some("metrics.json".into()),
            }),
            declare: vec![factory_core::config::ScopeMetricsDeclared {
                id: Some("x".into()),
                title: Some("Demo X".into()),
                unit: Some("ratio".into()),
                better: Some("higher".into()),
            }],
        });
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
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let registry = Registry::with_builtins();
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = Arc::new(Engine::new(
            Factory { root: root.clone(), config },
            registry,
            store,
            PathBuf::from("factory"),
            Vec::new(),
        ));

        write(
            &goals::goals_dir(&engine.factory_snapshot().root),
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: T\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: KR, kind: committed, metric: reported.demo.x, baseline: 0, target: 1}\n",
        );
        std::fs::write(
            root.join("metrics.json"),
            r#"{"as_of":"2026-10-05T09:00:00Z","metrics":[{"id":"x","value":0.75}]}"#,
        )
        .unwrap();

        let report = engine.goals_report(None, Some("2026-q4")).await.unwrap();
        let kr_result = &report.report.as_ref().unwrap().objectives[0].key_results[0];
        assert_eq!(kr_result.value, Some(0.75));
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == goals::FindingKind::WrongDirection),
            "a rising target is right for this higher-declared metric: {:?}",
            report.findings
        );
    }

    /// `#278` fix, end to end through the daemon: `goals_report` threads
    /// the scope's own live declared direction into the WrongDirection
    /// check, not `metrics::resolve`'s fixed `Higher` placeholder -- a
    /// lower-declared reported metric with a *rising* target is flagged,
    /// the same as a built-in lower-is-better metric would be.
    #[tokio::test]
    async fn a_lower_declared_reported_key_result_moving_the_wrong_way_is_flagged_end_to_end() {
        let root = std::env::temp_dir()
            .join(format!("factory-goals-reported-wrong-direction-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut company = scope_at("company-id", "company", ".");
        company.metrics = Some(factory_core::config::ScopeMetricsDeclaration {
            source: Some(factory_core::config::ScopeMetricsSource {
                id: Some("finance".into()),
                file: Some("metrics.json".into()),
            }),
            declare: vec![factory_core::config::ScopeMetricsDeclared {
                id: Some("unresolved_transactions".into()),
                title: Some("Unresolved transactions".into()),
                unit: Some("count".into()),
                better: Some("lower".into()),
            }],
        });
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
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let registry = Registry::with_builtins();
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = Arc::new(Engine::new(
            Factory { root: root.clone(), config },
            registry,
            store,
            PathBuf::from("factory"),
            Vec::new(),
        ));

        // baseline 0 -> target 12 asks unresolved transactions to *rise* --
        // the wrong way for a lower-is-better metric.
        write(
            &goals::goals_dir(&engine.factory_snapshot().root),
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: T\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: KR, kind: committed, metric: reported.finance.unresolved_transactions, baseline: 0, target: 12}\n",
        );

        let report = engine.goals_report(None, Some("2026-q4")).await.unwrap();
        let f = report
            .findings
            .iter()
            .find(|f| f.kind == goals::FindingKind::WrongDirection)
            .unwrap_or_else(|| panic!("{:?}", report.findings));
        assert!(f.detail.contains("obj/kr"), "{}", f.detail);
    }

    /// The reverse of the test above: a higher-declared reported metric
    /// with a *falling* target is flagged end to end -- proving the
    /// daemon's own live direction map actually carries `Higher` through
    /// to `goals_report`, not just that an empty map stays quiet.
    #[tokio::test]
    async fn a_higher_declared_reported_key_result_moving_the_wrong_way_is_flagged_end_to_end() {
        let root = std::env::temp_dir().join(format!(
            "factory-goals-reported-wrong-direction-higher-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut company = scope_at("company-id", "company", ".");
        company.metrics = Some(factory_core::config::ScopeMetricsDeclaration {
            source: Some(factory_core::config::ScopeMetricsSource {
                id: Some("finance".into()),
                file: Some("metrics.json".into()),
            }),
            declare: vec![factory_core::config::ScopeMetricsDeclared {
                id: Some("beleg_coverage".into()),
                title: Some("Beleg coverage".into()),
                unit: Some("ratio".into()),
                better: Some("higher".into()),
            }],
        });
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
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let registry = Registry::with_builtins();
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = Arc::new(Engine::new(
            Factory { root: root.clone(), config },
            registry,
            store,
            PathBuf::from("factory"),
            Vec::new(),
        ));

        // baseline 0.9 -> target 0.5 asks coverage to *fall* -- the wrong
        // way for a higher-is-better metric.
        write(
            &goals::goals_dir(&engine.factory_snapshot().root),
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: T\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: KR, kind: committed, metric: reported.finance.beleg_coverage, baseline: 0.9, target: 0.5}\n",
        );

        let report = engine.goals_report(None, Some("2026-q4")).await.unwrap();
        let f = report
            .findings
            .iter()
            .find(|f| f.kind == goals::FindingKind::WrongDirection)
            .unwrap_or_else(|| panic!("{:?}", report.findings));
        assert!(f.detail.contains("obj/kr"), "{}", f.detail);
    }

    #[tokio::test]
    async fn an_unknown_cycle_id_is_refused() {
        let engine = test_engine();
        write(&goals::goals_dir(&engine.factory_snapshot().root), "2026-q4.yaml", CYCLE);
        let err = engine.goals_report(None, Some("2099-q1")).await.unwrap_err();
        assert!(err.to_string().contains("no such cycle"), "{err}");
    }

    #[tokio::test]
    async fn checkin_refuses_an_unknown_kr_a_computed_kr_bad_confidence_and_a_non_finite_value() {
        let engine = test_engine();
        write(
            &goals::goals_dir(&engine.factory_snapshot().root),
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: T\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: manual-kr, title: M, kind: committed, manual: true, baseline: 0, target: 1}\n\
             \x20\x20\x20\x20\x20\x20- {id: computed-kr, title: C, kind: committed, metric: first_pass_yield, baseline: 0, target: 1}\n",
        );
        let owner = Caller::Owner;

        let err = engine.goals_checkin(&owner, KrRef::new("obj", "nope"), 1.0, 5, None).await.unwrap_err();
        assert!(err.to_string().contains("names no key result"), "{err}");

        let err = engine.goals_checkin(&owner, KrRef::new("obj", "computed-kr"), 1.0, 5, None).await.unwrap_err();
        assert!(err.to_string().contains("computed key result"), "{err}");

        let err = engine.goals_checkin(&owner, KrRef::new("obj", "manual-kr"), 1.0, 11, None).await.unwrap_err();
        assert!(err.to_string().contains("confidence"), "{err}");

        let err = engine.goals_checkin(&owner, KrRef::new("obj", "manual-kr"), f64::NAN, 5, None).await.unwrap_err();
        assert!(err.to_string().contains("finite"), "{err}");

        // Not refused: a real manual key result, a sane confidence, a finite value.
        let ok = engine
            .goals_checkin(&owner, KrRef::new("obj", "manual-kr"), 0.5, 5, Some("note".into()))
            .await
            .unwrap();
        assert_eq!(ok.value, 0.5);
    }

    #[tokio::test]
    async fn checkins_are_append_only_and_the_report_carries_the_whole_history() {
        let engine = test_engine();
        write(
            &goals::goals_dir(&engine.factory_snapshot().root),
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: T\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: M, kind: committed, manual: true, baseline: 0, target: 1}\n",
        );
        let owner = Caller::Owner;
        let kr_ref = KrRef::new("obj", "kr");

        engine.goals_checkin(&owner, kr_ref.clone(), 0.2, 4, None).await.unwrap();
        engine
            .goals_checkin(&owner, kr_ref.clone(), 0.6, 8, Some("closer".to_string()))
            .await
            .unwrap();

        let all = engine.l6.goals.all().await.unwrap();
        assert_eq!(all.len(), 2, "both rows kept -- append-only, never revised or replaced");

        let report = engine.goals_report(None, Some("2026-q4")).await.unwrap();
        let history = report.checkins.get(&kr_ref).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].value, 0.2, "oldest first");
        assert_eq!(history[1].value, 0.6);

        let kr_result = &report.report.as_ref().unwrap().objectives[0].key_results[0];
        assert_eq!(kr_result.value, Some(0.6), "the latest check-in wins the key result's own current value");
        assert_eq!(kr_result.confidence, Some(8));
    }

    /// `#158`: a key result over `conformance_rate.<category>` scores off a
    /// real run through the same registry every computed metric already
    /// goes through -- `goals_report` names no special case for it.
    #[tokio::test]
    async fn a_key_result_over_conformance_rate_scores_from_a_real_held_run() {
        use factory_core::adapter::store::task_from_new;
        use factory_core::control_plan::{
            AttestationVerdict, RequiredStep, StepAttestation, StepKind, GATE_ACTOR,
        };
        use factory_core::run::{NewRun, RunPatch, RunStatus, Trigger};
        use factory_core::task::NewTask;

        let engine = test_engine();
        write(
            &goals::goals_dir(&engine.factory_snapshot().root),
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: T\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: Feature conformance, kind: committed, metric: conformance_rate.feature, baseline: 0, target: 1}\n",
        );

        // A held, conforming `feature` run -- built directly through the
        // store, the same shape `metrics::tests::held_run` builds.
        let task = engine
            .l4.store
            .create(&task_from_new(
                NewTask {
                    title: "t".into(),
                    category: Some("feature".into()),
                    ..Default::default()
                },
                "demo".into(),
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
        let now = Utc::now();
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
                scope: "demo".into(),
                category: "feature".into(),
                step: "tests".into(),
                kind: StepKind::Gate,
                actor: GATE_ACTOR.to_string(),
                verdict: AttestationVerdict::Pass,
                required_by: Vec::new(),
                command: Some("true".into()),
                exit_code: Some(0),
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
        let report = engine.goals_report(None, Some("2026-q4")).await.unwrap();
        let kr_result = &report.report.as_ref().unwrap().objectives[0].key_results[0];
        assert_eq!(kr_result.value, Some(1.0), "the one held run conforms");
        assert_eq!(
            kr_result.score,
            Some(1.0),
            "baseline 0, target 1 -- a value of 1.0 scores 1.0"
        );
    }
}
