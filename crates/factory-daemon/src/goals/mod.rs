//! Where goals requests are served. Like `policies/mod.rs`: the authored
//! catalogue under `<root>/.factory/goals/` (`factory_core::goals`, pure and
//! tested on its own) is the source of truth for *what* the direction and a
//! cycle's objectives are, re-read on every request; this module assembles
//! the evidence that already lives elsewhere in the daemon -- computed
//! metrics (`crate::metrics`) and check-ins -- and folds it against them.
//!
//! The one piece of state this module owns is the check-ins themselves,
//! kept in `GoalsStore` (`store.rs`), append-only.
//!
//! ## Goals enforce nothing
//!
//! Nothing here starts, stops, or gates any work, same as policies (design
//! §8). `goals_checkin` validates a check-in's own shape (a real key result,
//! `manual: true`, a sane confidence and value) but never asks whether the
//! value itself is plausible -- that judgement belongs to whoever reads the
//! L6 tab, not to this module.

mod store;
pub use store::GoalsStore;

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use factory_core::config::{Factory, Scope};
use factory_core::error::{FactoryError, Result};
use factory_core::goals::{self, CheckIn, Cycle, KrRef};
use factory_core::metrics::MetricValue;
use factory_core::protocol::{CycleSummary, GoalsReport, InputView, NorthStarView};

use crate::access::Caller;
use crate::engine::Engine;

/// Whether `obj_scope` (an objective's or roadmap item's own optional
/// `scope:`, `None` meaning the root) is `asked` itself or a descendant of
/// it, by `Scope::path` -- the same test `policy_report`'s own
/// `target_scopes` filter runs, just aimed the other way: there it asks
/// "which scopes are under the one requested", here "does this one thing's
/// scope sit under the one requested". A name the live config does not
/// currently define excludes the objective it is on -- the config may have
/// changed since the catalogue was authored, and a filtered request has
/// nothing to place it under.
fn scope_matches(snapshot: &Factory, asked: &Scope, obj_scope: Option<&str>) -> bool {
    let root_name = snapshot.config.scope.as_ref().map(|s| s.name.as_str());
    let Some(name) = obj_scope.or(root_name) else {
        return false;
    };
    match snapshot.scope(name) {
        Ok(s) => {
            s.name == asked.name
                || snapshot
                    .config
                    .ancestors_of(s)
                    .iter()
                    .any(|ancestor| ancestor.name == asked.name)
        }
        Err(_) => false,
    }
}

/// `cycle`, with its objectives and roadmap items narrowed to the ones
/// `scope_matches` -- unchanged (a clone) when `asked` is `None`, the same
/// "no scope named, the whole instance" rule `policy_report` follows.
fn filter_cycle_for_scope(cycle: &Cycle, snapshot: &Factory, asked: Option<&Scope>) -> Cycle {
    let Some(asked) = asked else {
        return cycle.clone();
    };
    let mut filtered = cycle.clone();
    filtered.objectives.retain(|o| scope_matches(snapshot, asked, o.scope.as_deref()));
    filtered.roadmap.retain(|r| scope_matches(snapshot, asked, r.scope.as_deref()));
    filtered
}

fn mean_score(report: &goals::CycleReport) -> Option<f64> {
    let scored: Vec<f64> = report.objectives.iter().filter_map(|o| o.score).collect();
    if scored.is_empty() {
        None
    } else {
        Some(scored.iter().sum::<f64>() / scored.len() as f64)
    }
}

impl Engine {
    /// The L6 Goals tab: `Request::Goals`.
    pub(crate) async fn goals_report(self: &Arc<Self>, scope: Option<&str>, cycle_id: Option<&str>) -> Result<GoalsReport> {
        let now = Utc::now();
        let snapshot = self.factory_snapshot();
        let goals_path = goals::goals_dir(&snapshot.root);
        let catalogue = tokio::task::spawn_blocking(move || goals::load(&goals_path))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("goals catalogue walk: {e}")))?;

        let asked = scope.map(|name| snapshot.scope(name)).transpose()?.cloned();

        if let Some(id) = cycle_id {
            if !catalogue.cycles.iter().any(|c| c.id() == id) {
                return Err(FactoryError::BadRequest(format!("no such cycle: {id:?}")));
            }
        }
        let target_cycle_id: Option<String> = match cycle_id {
            Some(id) => Some(id.to_string()),
            None => goals::current_cycle(&catalogue.cycles, now).map(|c| c.id().to_string()),
        };

        // Every metric the catalogue itself names, computed once -- not
        // narrowed to the asked scope, since a scope filter only decides
        // what the *report* shows, not what is worth computing; the same
        // value would be fetched again for a neighbouring scope's request a
        // moment later.
        let ids = crate::metrics::goals_metric_ids(&catalogue);
        let computed = self.metrics(&ids, now).await?;
        let values: BTreeMap<_, MetricValue> = computed.values.into_iter().map(|v| (v.id.clone(), v)).collect();

        let checkins = self.goals.all().await?;

        let mut cycles = Vec::new();
        let mut target_report = None;
        let mut roadmap = Vec::new();
        for original in &catalogue.cycles {
            let filtered = filter_cycle_for_scope(original, &snapshot, asked.as_ref());
            let report = goals::evaluate(&filtered, &values, &checkins, now);
            cycles.push(CycleSummary {
                id: original.id().to_string(),
                from: original.starts_on(),
                to: original.ends_on(),
                status: goals::cycle_status(original, now),
                score: mean_score(&report),
            });
            if target_cycle_id.as_deref() == Some(original.id()) {
                roadmap = filtered.roadmap.clone();
                target_report = Some(report);
            }
        }

        let north_star = catalogue.direction.as_ref().and_then(|d| d.north_star.as_ref()).map(|ns| NorthStarView {
            metric: ns.metric.clone(),
            why: ns.why.clone(),
            value: values.get(&ns.metric).cloned().unwrap_or_else(|| not_computed(&ns.metric, now)),
        });
        let inputs = catalogue
            .direction
            .as_ref()
            .map(|d| d.inputs.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|metric| InputView {
                value: values.get(&metric).cloned().unwrap_or_else(|| not_computed(&metric, now)),
                metric,
            })
            .collect();

        // Every manual key result's whole check-in history, oldest first,
        // across every cycle -- not narrowed by the scope filter or by
        // `target_cycle_id`, since a key result's history outlives whichever
        // cycle happens to be asked about.
        let mut checkin_history: BTreeMap<KrRef, Vec<CheckIn>> = BTreeMap::new();
        for cycle in &catalogue.cycles {
            for objective in &cycle.objectives {
                for kr in &objective.key_results {
                    if kr.manual {
                        let kr_ref = KrRef::new(objective.id.clone(), kr.id.clone());
                        let mut history: Vec<CheckIn> = checkins.iter().filter(|c| c.kr == kr_ref).cloned().collect();
                        history.sort_by_key(|c| c.at);
                        checkin_history.insert(kr_ref, history);
                    }
                }
            }
        }

        Ok(GoalsReport {
            scope: asked.as_ref().map(|s| s.name.clone()),
            direction: catalogue.direction.clone(),
            cycles,
            report: target_report,
            findings: catalogue.findings.clone(),
            north_star,
            inputs,
            roadmap,
            checkins: checkin_history,
        })
    }

    /// Record a check-in against a manual key result: `Request::GoalsCheckIn`.
    /// Refuses a `kr` no loaded cycle defines, one that is not
    /// `manual: true` (a computed key result's value comes from its metric,
    /// never a check-in), a `confidence` outside `0..=10`, or a non-finite
    /// `value`.
    pub(crate) async fn goals_checkin(
        self: &Arc<Self>,
        caller: &Caller,
        kr: KrRef,
        value: f64,
        confidence: u8,
        note: Option<String>,
    ) -> Result<CheckIn> {
        if !value.is_finite() {
            return Err(FactoryError::BadRequest("value must be a finite number".into()));
        }
        if confidence > 10 {
            return Err(FactoryError::BadRequest(format!("confidence must be 0..=10, got {confidence}")));
        }

        let snapshot = self.factory_snapshot();
        let goals_path = goals::goals_dir(&snapshot.root);
        let catalogue = tokio::task::spawn_blocking(move || goals::load(&goals_path))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("goals catalogue walk: {e}")))?;

        let found_kr = catalogue
            .cycles
            .iter()
            .flat_map(|c| &c.objectives)
            .find(|o| o.id == kr.objective)
            .and_then(|o| o.key_results.iter().find(|k| k.id == kr.kr));
        let Some(found_kr) = found_kr else {
            return Err(FactoryError::BadRequest(format!("{kr} names no key result in any loaded cycle")));
        };
        if !found_kr.manual {
            return Err(FactoryError::BadRequest(format!(
                "{kr} is a computed key result, backed by a metric -- it can't be checked in"
            )));
        }

        let checkin = CheckIn {
            id: uuid::Uuid::new_v4().to_string(),
            kr,
            value,
            confidence,
            note,
            by: crate::policies::caller_name(caller),
            at: Utc::now(),
        };
        self.goals.append(&checkin).await?;
        Ok(checkin)
    }

    /// The `GoalContext` a task's `goal=<objective>/<kr>` label resolves to,
    /// read fresh off `.factory/goals/` -- called once per dispatch, the
    /// same as `policy_frameworks`. `None` for a task with no such label, or
    /// one naming a pair the catalogue does not currently define -- never an
    /// error: an unknown label should not stop a task from dispatching.
    pub(crate) async fn goal_context(&self, root: std::path::PathBuf, label: Option<String>) -> Option<factory_core::adapter::agent::GoalContext> {
        let label = label?;
        tokio::task::spawn_blocking(move || {
            let catalogue = goals::load(&goals::goals_dir(&root));
            goals::resolve_label(&catalogue.cycles, &label).map(|(objective_id, objective_title, kr_id, kr_title)| {
                factory_core::adapter::agent::GoalContext {
                    objective_id,
                    objective_title,
                    kr_id,
                    kr_title,
                }
            })
        })
        .await
        .ok()
        .flatten()
    }
}

fn not_computed(metric: &factory_core::metrics::MetricId, now: chrono::DateTime<Utc>) -> MetricValue {
    MetricValue {
        id: metric.clone(),
        value: None,
        as_of: now,
        reason: Some("metric not computed for this report".to_string()),
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

        let all = engine.goals.all().await.unwrap();
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

    #[tokio::test]
    async fn goal_context_resolves_a_known_label_and_is_none_for_an_unknown_one() {
        let engine = test_engine();
        write(
            &goals::goals_dir(&engine.factory_snapshot().root),
            "2026-q4.yaml",
            "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
             objectives:\n\
             \x20\x20- id: obj\n\x20\x20\x20\x20title: Objective Title\n\x20\x20\x20\x20key_results:\n\
             \x20\x20\x20\x20\x20\x20- {id: kr, title: KR Title, kind: committed, manual: true, baseline: 0, target: 1}\n",
        );
        let root = engine.factory_snapshot().root.clone();

        let found = engine.goal_context(root.clone(), Some("obj/kr".to_string())).await.unwrap();
        assert_eq!(found.objective_title, "Objective Title");
        assert_eq!(found.kr_title, "KR Title");

        assert!(engine.goal_context(root.clone(), Some("obj/nope".to_string())).await.is_none());
        assert!(engine.goal_context(root, None).await.is_none());
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
            .store
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
            .store
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
            .store
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
            .policies
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
