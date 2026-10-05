//! The live L6 Goals owner: authored catalogues, scope selection, scoring
//! and append-only manual check-ins. Outside wiring supplies raw identities
//! and the actual L5 metric fact capability, never computed metric values.
use crate::{
    goals::{self, CheckIn, Cycle, KrRef},
    goals_store::GoalsStore,
    goals_view::{CycleSummary, GoalsReport, InputView, NorthStarView},
};
use chrono::{DateTime, Utc};
use factory_assurance::metrics::{self, MetricError};
use factory_kernel::{
    FactoryError, Facts, MetricId, MetricValue, MetricValuesFact, Provide, Result, ScopeNode,
    ScopeTree, L6,
};
use std::{collections::BTreeMap, path::PathBuf};

pub struct Service<'a> {
    root: PathBuf,
    scopes: ScopeTree,
    root_name: Option<String>,
    store: &'a GoalsStore,
    /// `#278`: every scope's own declared `reported.<source>.<metric>`
    /// direction, keyed the same way `reported::directions` returns it --
    /// a plain value the outside router builds fresh from the live config
    /// snapshot, the same shape `quality_configuration`/`reported_
    /// configuration` already use elsewhere. Empty for a caller that does
    /// not have (or does not need) live data -- see `goals::
    /// load_with_reported_directions`'s own doc comment for what that
    /// means for the wrong-direction check.
    reported_directions: BTreeMap<String, metrics::Better>,
}
pub struct Plan {
    catalogue: goals::GoalsCatalogue,
    asked: Option<ScopeNode>,
    target_cycle_id: Option<String>,
    scopes: ScopeTree,
    root_name: Option<String>,
    now: DateTime<Utc>,
}
impl Plan {
    pub fn metric_ids(&self) -> Vec<MetricId> {
        metric_ids(&self.catalogue)
    }
    pub async fn read_metrics<P>(self, provider: &P, query: &P::Query) -> Result<Measured>
    where
        P: Provide<MetricValuesFact, Value = MetricValuesFact, Error = FactoryError>,
    {
        let fact = Facts::<L6>::new()
            .get::<MetricValuesFact, _>(provider, query)
            .await?;
        Ok(Measured { plan: self, fact })
    }
}
/// Evidence received through this owner's own fact read. The private fields
/// cannot be populated with computed values by the outside router.
pub struct Measured {
    plan: Plan,
    fact: MetricValuesFact,
}
impl<'a> Service<'a> {
    pub fn new(
        root: PathBuf,
        scopes: ScopeTree,
        root_name: Option<String>,
        store: &'a GoalsStore,
        reported_directions: BTreeMap<String, metrics::Better>,
    ) -> Self {
        Self {
            root,
            scopes,
            root_name,
            store,
            reported_directions,
        }
    }
    pub async fn prepare(
        &self,
        scope: Option<&str>,
        cycle_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Plan> {
        let goals_path = goals::goals_dir(&self.root);
        let reported_directions = self.reported_directions.clone();
        let catalogue = tokio::task::spawn_blocking(move || {
            goals::load_with_reported_directions(&goals_path, &reported_directions)
        })
        .await
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("goals catalogue walk: {e}")))?;
        let asked = scope
            .map(|name| self.scopes.scope(name))
            .transpose()?
            .cloned();
        if let Some(id) = cycle_id {
            if !catalogue.cycles.iter().any(|c| c.id() == id) {
                return Err(FactoryError::BadRequest(format!("no such cycle: {id:?}")));
            }
        }
        let target_cycle_id = match cycle_id {
            Some(id) => Some(id.to_string()),
            None => goals::current_cycle(&catalogue.cycles, now).map(|c| c.id().to_string()),
        };
        Ok(Plan {
            catalogue,
            asked,
            target_cycle_id,
            scopes: self.scopes.clone(),
            root_name: self.root_name.clone(),
            now,
        })
    }
    pub async fn finish(&self, measured: Measured) -> Result<GoalsReport> {
        let Measured { plan, fact } = measured;
        let Plan {
            catalogue,
            asked,
            target_cycle_id,
            scopes,
            root_name,
            now,
        } = plan;
        let values: BTreeMap<_, MetricValue> =
            fact.values.into_iter().map(|v| (v.id.clone(), v)).collect();
        let checkins = self.store.all().await?;

        let mut cycles = Vec::new();
        let mut target_report = None;
        let mut roadmap = Vec::new();
        for original in &catalogue.cycles {
            let filtered =
                filter_cycle_for_scope(original, &scopes, root_name.as_deref(), asked.as_ref());
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

        let north_star = catalogue
            .direction
            .as_ref()
            .and_then(|d| d.north_star.as_ref())
            .map(|ns| NorthStarView {
                metric: ns.metric.clone(),
                why: ns.why.clone(),
                value: values
                    .get(&ns.metric)
                    .cloned()
                    .unwrap_or_else(|| not_computed(&ns.metric, now)),
            });
        let inputs = catalogue
            .direction
            .as_ref()
            .map(|d| d.inputs.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|metric| InputView {
                value: values
                    .get(&metric)
                    .cloned()
                    .unwrap_or_else(|| not_computed(&metric, now)),
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
                        let mut history: Vec<CheckIn> = checkins
                            .iter()
                            .filter(|c| c.kr == kr_ref)
                            .cloned()
                            .collect();
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

    pub async fn checkin(
        &self,
        kr: KrRef,
        value: f64,
        confidence: u8,
        note: Option<String>,
        by: String,
    ) -> Result<CheckIn> {
        if !value.is_finite() {
            return Err(FactoryError::BadRequest(
                "value must be a finite number".into(),
            ));
        }
        if confidence > 10 {
            return Err(FactoryError::BadRequest(format!(
                "confidence must be 0..=10, got {confidence}"
            )));
        }

        let goals_path = goals::goals_dir(&self.root);
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
            return Err(FactoryError::BadRequest(format!(
                "{kr} names no key result in any loaded cycle"
            )));
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
            by,
            at: Utc::now(),
        };
        self.store.append(&checkin).await?;
        Ok(checkin)
    }
}

fn scope_matches(
    scopes: &ScopeTree,
    root_name: Option<&str>,
    asked: &ScopeNode,
    obj_scope: Option<&str>,
) -> bool {
    let Some(name) = obj_scope.or(root_name) else {
        return false;
    };
    match scopes.scope(name) {
        Ok(s) => {
            s.name == asked.name
                || scopes
                    .ancestors_of(s)
                    .iter()
                    .any(|ancestor| ancestor.name == asked.name)
        }
        Err(_) => false,
    }
}

/// Raw label context from the authored catalogue. Runtime projection remains
/// outside L6; execution of the label still belongs to the command migration.
pub async fn context(
    root: PathBuf,
    label: Option<String>,
) -> Option<(String, String, String, String)> {
    let label = label?;
    tokio::task::spawn_blocking(move || {
        let catalogue = goals::load(&goals::goals_dir(&root));
        goals::resolve_label(&catalogue.cycles, &label)
    })
    .await
    .ok()
    .flatten()
}
fn filter_cycle_for_scope(
    cycle: &Cycle,
    scopes: &ScopeTree,
    root_name: Option<&str>,
    asked: Option<&ScopeNode>,
) -> Cycle {
    let Some(asked) = asked else {
        return cycle.clone();
    };
    let mut filtered = cycle.clone();
    filtered
        .objectives
        .retain(|o| scope_matches(scopes, root_name, asked, o.scope.as_deref()));
    filtered
        .roadmap
        .retain(|r| scope_matches(scopes, root_name, asked, r.scope.as_deref()));
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
fn not_computed(metric: &MetricId, now: chrono::DateTime<Utc>) -> MetricValue {
    MetricValue {
        id: metric.clone(),
        value: None,
        as_of: now,
        reason: Some("metric not computed for this report".to_string()),
    }
}

/// Every metric id `catalogue` itself names -- `direction.yaml`'s
/// `north_star`/`inputs` and every key result's own bound metric -- filtered
/// to ids `metrics::resolve` has at least heard of (`Unknown` ones are
/// dropped; `Unavailable` ones are kept, so they still come back with their
/// reason rather than silently vanishing from a report).
pub fn metric_ids(catalogue: &goals::GoalsCatalogue) -> Vec<MetricId> {
    let mut ids = Vec::new();
    if let Some(direction) = &catalogue.direction {
        if let Some(north_star) = &direction.north_star {
            push_if_known(&mut ids, &north_star.metric);
        }
        for input in &direction.inputs {
            push_if_known(&mut ids, input);
        }
    }
    for cycle in &catalogue.cycles {
        for objective in &cycle.objectives {
            for kr in &objective.key_results {
                if let Some(bound) = kr.bound_metric(&objective.id) {
                    push_if_known(&mut ids, &bound);
                }
            }
        }
    }
    ids
}

fn push_if_known(ids: &mut Vec<MetricId>, id: &MetricId) {
    if !matches!(metrics::resolve(id), Err(MetricError::Unknown(_))) {
        ids.push(id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_kernel::{FactProvider, L5};
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    };
    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("factory-goals-owner-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(goals::goals_dir(&root)).unwrap();
            Self(root)
        }
        fn write(&self, name: &str, text: &str) {
            std::fs::write(goals::goals_dir(&self.0).join(name), text).unwrap();
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[derive(Default)]
    struct Metrics {
        calls: Mutex<Vec<Vec<MetricId>>>,
        value: AtomicUsize,
        fail: AtomicBool,
    }
    impl FactProvider for Metrics {
        type Level = L5;
    }
    #[async_trait::async_trait]
    impl Provide<MetricValuesFact> for Metrics {
        type Query = Vec<MetricId>;
        type Value = MetricValuesFact;
        type Error = FactoryError;
        async fn get(&self, ids: &Vec<MetricId>) -> Result<MetricValuesFact> {
            self.calls.lock().unwrap().push(ids.clone());
            if self.fail.load(Ordering::SeqCst) {
                return Err(FactoryError::adapter("metrics", "unavailable"));
            }
            Ok(MetricValuesFact {
                values: ids
                    .iter()
                    .map(|id| MetricValue {
                        id: id.clone(),
                        value: Some(self.value.load(Ordering::SeqCst) as f64),
                        as_of: now(),
                        reason: None,
                    })
                    .collect(),
            })
        }
    }
    fn now() -> DateTime<Utc> {
        "2026-10-16T12:00:00Z".parse().unwrap()
    }
    fn scopes() -> ScopeTree {
        ScopeTree {
            scopes: [
                ("company", "."),
                ("work", "projects/work"),
                ("child", "projects/work/deep"),
                ("side", "projects/work-other"),
            ]
            .into_iter()
            .map(|(name, path)| ScopeNode {
                name: name.into(),
                path: path.into(),
            })
            .collect(),
        }
    }
    fn service<'a>(root: &Root, store: &'a GoalsStore) -> Service<'a> {
        Service::new(
            root.0.clone(),
            scopes(),
            Some("company".into()),
            store,
            BTreeMap::new(),
        )
    }
    const CYCLE: &str = "cycle: { id: current, from: 2026-10-01, to: 2026-12-31 }\nobjectives:\n  - id: ship\n    title: Ship\n    scope: work\n    key_results:\n      - {id: manual, title: Manual, kind: committed, manual: true, baseline: 0, target: 10}\n      - {id: computed, title: Computed, kind: committed, metric: first_pass_yield, baseline: 0, target: 10}\n";
    async fn report(
        service: &Service<'_>,
        metrics: &Metrics,
        scope: Option<&str>,
    ) -> Result<GoalsReport> {
        let plan = service.prepare(scope, None, now()).await?;
        let query = plan.metric_ids();
        let measured = plan.read_metrics(metrics, &query).await?;
        service.finish(measured).await
    }
    #[tokio::test]
    async fn catalogue_and_metric_fact_are_live_and_failed_reads_never_leave_a_status_cache() {
        let root = Root::new();
        root.write("current.yaml", CYCLE);
        let store = GoalsStore::in_memory().unwrap();
        let metrics = Metrics::default();
        let owner = service(&root, &store);
        metrics.value.store(2, Ordering::SeqCst);
        let first = report(&owner, &metrics, Some("work")).await.unwrap();
        assert_eq!(
            first.report.as_ref().unwrap().objectives[0].key_results[1].value,
            Some(2.0)
        );
        root.write(
            "current.yaml",
            &CYCLE.replace("title: Ship", "title: Updated"),
        );
        metrics.value.store(5, Ordering::SeqCst);
        let next = report(&owner, &metrics, Some("work")).await.unwrap();
        assert_eq!(
            next.report.as_ref().unwrap().objectives[0].key_results[1].value,
            Some(5.0)
        );
        assert_eq!(next.report.as_ref().unwrap().objectives[0].title, "Updated");
        metrics.fail.store(true, Ordering::SeqCst);
        assert!(report(&owner, &metrics, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("unavailable"));
        metrics.fail.store(false, Ordering::SeqCst);
        assert!(report(&owner, &metrics, None).await.is_ok());
        assert_eq!(metrics.calls.lock().unwrap().len(), 4);
    }
    #[tokio::test]
    async fn metric_selection_remains_whole_catalogue_even_for_an_unrelated_scope() {
        let root = Root::new();
        root.write("current.yaml", CYCLE);
        root.write("direction.yaml", "vision: V\nmission: M\nnorth_star: {metric: throughput_week, why: Count}\ninputs: [first_pass_yield, unregistered_metric]\n");
        let store = GoalsStore::in_memory().unwrap();
        let metrics = Metrics::default();
        let owner = service(&root, &store);
        let selected = report(&owner, &metrics, Some("side")).await.unwrap();
        assert!(selected.report.unwrap().objectives.is_empty());
        let calls = metrics.calls.lock().unwrap();
        assert_eq!(
            calls[0].iter().map(MetricId::as_str).collect::<Vec<_>>(),
            ["throughput_week", "first_pass_yield", "first_pass_yield"]
        );
        assert!(selected.inputs[1].value.value.is_none());
        assert_eq!(
            selected.inputs[1].value.reason.as_deref(),
            Some("metric not computed for this report")
        );
    }
    #[tokio::test]
    async fn aliases_follow_path_ancestry_and_unknown_cycles_fail_before_a_lower_read() {
        let root = Root::new();
        root.write("current.yaml", CYCLE);
        let store = GoalsStore::in_memory().unwrap();
        let metrics = Metrics::default();
        let owner = service(&root, &store);
        assert!(owner.prepare(None, Some("missing"), now()).await.is_err());
        assert!(owner.prepare(Some("missing"), None, now()).await.is_err());
        assert!(metrics.calls.lock().unwrap().is_empty());
        assert_eq!(
            report(&owner, &metrics, Some("projects/work"))
                .await
                .unwrap()
                .report
                .unwrap()
                .objectives
                .len(),
            1
        );
        assert!(report(&owner, &metrics, Some("side"))
            .await
            .unwrap()
            .report
            .unwrap()
            .objectives
            .is_empty());
        assert!(
            report(&owner, &metrics, Some("child"))
                .await
                .unwrap()
                .report
                .unwrap()
                .objectives
                .is_empty(),
            "a child never inherits an ancestor's objective into its own filtered report"
        );
    }
    #[tokio::test]
    async fn manual_checkins_validate_current_intent_and_preserve_unfiltered_history() {
        let root = Root::new();
        root.write("current.yaml", CYCLE);
        let store = GoalsStore::in_memory().unwrap();
        let metrics = Metrics::default();
        let owner = service(&root, &store);
        let kr = KrRef::new("ship", "manual");
        for (value, confidence) in [(f64::NAN, 5), (1.0, 11)] {
            assert!(owner
                .checkin(kr.clone(), value, confidence, None, "owner".into())
                .await
                .is_err());
        }
        assert!(owner
            .checkin(KrRef::new("ship", "computed"), 1.0, 5, None, "owner".into())
            .await
            .is_err());
        assert!(store.all().await.unwrap().is_empty());
        owner
            .checkin(kr.clone(), 2.0, 4, None, "owner".into())
            .await
            .unwrap();
        owner
            .checkin(kr.clone(), 7.0, 8, Some("progress".into()), "owner".into())
            .await
            .unwrap();
        let selected = report(&owner, &metrics, Some("side")).await.unwrap();
        assert_eq!(
            selected.checkins[&kr]
                .iter()
                .map(|c| c.value)
                .collect::<Vec<_>>(),
            [2.0, 7.0]
        );
        root.write(
            "current.yaml",
            &CYCLE.replace("manual: true", "metric: throughput_week"),
        );
        assert!(owner
            .checkin(kr, 8.0, 8, None, "owner".into())
            .await
            .is_err());
        assert_eq!(store.all().await.unwrap().len(), 2);
    }
    #[tokio::test]
    async fn label_context_is_reread_and_absence_stays_absent() {
        let root = Root::new();
        root.write("current.yaml", CYCLE);
        assert!(context(root.0.clone(), None).await.is_none());
        assert!(context(root.0.clone(), Some("missing/manual".into()))
            .await
            .is_none());
        let first = context(root.0.clone(), Some("ship/manual".into()))
            .await
            .unwrap();
        assert_eq!(
            first,
            (
                "ship".into(),
                "Ship".into(),
                "manual".into(),
                "Manual".into()
            )
        );
        root.write(
            "current.yaml",
            &CYCLE.replace("title: Ship", "title: Updated"),
        );
        assert_eq!(
            context(root.0.clone(), Some("ship/manual".into()))
                .await
                .unwrap()
                .1,
            "Updated"
        );
    }
}
