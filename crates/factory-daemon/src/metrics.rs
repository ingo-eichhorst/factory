//! Metric computation: `factory_core::metrics`'s registry names the v1
//! vocabulary; this turns a `MetricId` into a `MetricValue` (and, for the
//! three production-based fixed metrics, a `MetricSeries`) off data the
//! daemon already has -- lazy, like a policy fact: only the ids actually
//! asked for are computed, and nothing here is a query language (design
//! §8), the same restraint `policies/mod.rs` holds toward evidence.
//!
//! ## Reuse, not reimplementation
//!
//! `throughput_week`/`first_pass_yield`/`scrap_rate` read `production.rs`'s
//! own daily grid (`Engine::production`) rather than re-deriving "finished"/
//! "scrapped"/"reworked" a second time -- that module's own doc comment is
//! the one place those words are defined, and this one only sums buckets it
//! already produced. `compliance.<fw>`/`open_controls.<fw>` read
//! `Engine::policy_report(None)`'s subtree rollup. `bench.resolve_rate.<dataset>`
//! reads the newest settled bench run through `bench::aggregate`, the same
//! function `bench show`'s own results table uses.
//! `goal_tasks_done.<objective>.<kr>` is the one metric with no existing
//! aggregate to lean on: it counts tasks itself, off the `TaskStore` trait
//! (`self.store`) so `ScopedStores` still shards correctly by scope --
//! unscoped, the same way `production.rs` reads every run before narrowing.
//!
//! ## Series
//!
//! Only the three production-based metrics carry a history: one point per
//! day over `production.rs`'s own 53-week daily grid, each point the
//! metric's own trailing-window definition evaluated as of that day (a
//! rolling 7-day sum for `throughput_week`, a rolling 28-day ratio for the
//! other two). The last point always equals the metric's own current value.
//! Every other metric is a single number with no time axis of its own to
//! draw yet.
//!
//! ## Unknown vs. unavailable
//!
//! An id `metrics::resolve` has never heard of refuses the whole call --
//! `Engine::metrics` is the one place a typo becomes a `BadRequest` rather
//! than a quiet `None`. An id it knows but cannot compute yet (`unit_cost`,
//! `tokens_per_run`) comes back as `value: None` with the registry's own
//! reason -- the issue's own last bullet. `default_metric_ids`, used only
//! when a caller's own `ids` was empty, filters the other way: it never
//! offers an *unknown* id (nothing built it), but does offer an
//! *unavailable* one, since "every non-parameterised metric" names both.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use factory_core::error::{FactoryError, Result};
use factory_core::goals::GoalsCatalogue;
use factory_core::metrics::{self, MetricDef, MetricError, MetricId, MetricSeries, MetricValue};
use factory_core::protocol::{MetricDefView, PolicyReport, ProductionBin};
use factory_core::task::{TaskFilter, TaskStatus};

use crate::engine::Engine;

/// `Request::Metrics`'s answer, and the same shape `#100`'s Scenarios tab
/// calls `Engine::metrics` for directly -- a plain struct rather than
/// `Payload`, so an in-process caller never has to pattern-match its own
/// dispatch's wire type back apart.
#[derive(Debug)]
pub struct Metrics {
    pub values: Vec<MetricValue>,
    pub series: Vec<MetricSeries>,
    pub registry: Vec<MetricDefView>,
}

fn is_production_metric(id: &str) -> bool {
    matches!(id, "throughput_week" | "first_pass_yield" | "scrap_rate")
}

fn is_policy_metric(id: &str) -> bool {
    id.starts_with("compliance.") || id.starts_with("open_controls.")
}

impl Engine {
    /// Compute every id in `ids` (deduplicated), lazily: nothing not asked
    /// for is ever touched, and `production`/`policy_report`/the bench store
    /// are each read at most once per call regardless of how many ids ask
    /// for something behind them.
    pub(crate) async fn metrics(self: &Arc<Self>, ids: &[MetricId], now: DateTime<Utc>) -> Result<Metrics> {
        let mut wanted: Vec<MetricId> = Vec::new();
        for id in ids {
            if !wanted.contains(id) {
                wanted.push(id.clone());
            }
        }

        // Resolved up front: an `Unknown` id refuses the whole call before
        // anything is computed, rather than a partial answer with a gap in
        // it nobody asked to reason about.
        let mut resolved: Vec<(MetricId, std::result::Result<MetricDef, &'static str>)> = Vec::new();
        for id in &wanted {
            match metrics::resolve(id) {
                Ok(def) => resolved.push((id.clone(), Ok(def))),
                Err(MetricError::Unavailable { reason, .. }) => resolved.push((id.clone(), Err(reason))),
                Err(MetricError::Unknown(_)) => {
                    return Err(FactoryError::BadRequest(format!("{id} is not a known metric")));
                }
            }
        }

        let needs_production = resolved.iter().any(|(id, r)| r.is_ok() && is_production_metric(id.as_str()));
        let needs_policy = resolved.iter().any(|(id, r)| r.is_ok() && is_policy_metric(id.as_str()));

        let production = if needs_production {
            Some(self.production(Some(5), Some(ProductionBin::Day), None).await?)
        } else {
            None
        };
        let policy_report = if needs_policy {
            Some(self.policy_report(None).await?)
        } else {
            None
        };

        let mut values = Vec::new();
        let mut series = Vec::new();
        let mut registry = Vec::new();

        for (id, def_result) in resolved {
            let def = match def_result {
                Ok(def) => def,
                Err(reason) => {
                    values.push(MetricValue {
                        id: id.clone(),
                        value: None,
                        as_of: now,
                        reason: Some(reason.to_string()),
                    });
                    // `metrics::registry()` lists the fixed, unbound family
                    // for a metric this module can name but not compute --
                    // v1 has no *parameterised* unavailable family, so an
                    // exact-id lookup always finds it for today's two.
                    if let Some(found) = metrics::registry().into_iter().find(|d| d.id == id.as_str()) {
                        registry.push(found.into());
                    }
                    continue;
                }
            };

            let value = if id.as_str() == "throughput_week" {
                let daily = &production.as_ref().expect("needs_production set").daily;
                let s = throughput_week_series(daily);
                let v = value_from_series(&s, now, "no finished runs recorded yet");
                series.push(s);
                v
            } else if id.as_str() == "first_pass_yield" {
                let daily = &production.as_ref().expect("needs_production set").daily;
                let s = first_pass_yield_series(daily);
                let v = value_from_series(&s, now, "no finished runs in the trailing 28 days");
                series.push(s);
                v
            } else if id.as_str() == "scrap_rate" {
                let daily = &production.as_ref().expect("needs_production set").daily;
                let s = scrap_rate_series(daily);
                let v = value_from_series(&s, now, "no finished runs in the trailing 28 days");
                series.push(s);
                v
            } else if let Some(framework) = id.as_str().strip_prefix("compliance.") {
                compliance_value(&id, policy_report.as_ref().expect("needs_policy set"), framework, now)
            } else if let Some(framework) = id.as_str().strip_prefix("open_controls.") {
                open_controls_value(&id, policy_report.as_ref().expect("needs_policy set"), framework, now)
            } else if let Some(dataset) = id.as_str().strip_prefix("bench.resolve_rate.") {
                self.bench_resolve_rate_value(&id, dataset, now).await?
            } else if let Some(rest) = id.as_str().strip_prefix("goal_tasks_done.") {
                let (objective, kr) = rest.split_once('.').ok_or_else(|| {
                    FactoryError::Other(anyhow::anyhow!("malformed goal_tasks_done id {id}"))
                })?;
                self.goal_tasks_done_value(&id, objective, kr, now).await?
            } else {
                // Every family `metrics::resolve` returns `Ok` for today has
                // a branch above; a future metric added to the registry
                // without one here comes back honestly unresolved rather
                // than panicking a request that merely asked for it --
                // `factory-core` and `factory-daemon` are separate crates,
                // so the compiler cannot force the two to be added together.
                MetricValue {
                    id: id.clone(),
                    value: None,
                    as_of: now,
                    reason: Some("no computation wired for this metric yet".to_string()),
                }
            };
            values.push(value);
            registry.push(def.into());
        }

        Ok(Metrics { values, series, registry })
    }

    /// The default `ids` for `Request::Metrics` when a caller's own list is
    /// empty: every non-parameterised metric (available or not) plus, for
    /// every loaded policy catalogue, its `compliance`/`open_controls`
    /// pair, plus every metric id the goals catalogue itself names --
    /// `direction.yaml`'s `north_star`/`inputs` and every key result's
    /// bound metric. An id the catalogues name that `metrics::resolve` has
    /// never heard of is silently left out here (its own `UnknownMetric`
    /// finding already says so at `factory goals`), never turned into a
    /// refusal the way an explicitly typed unknown id is.
    pub(crate) async fn default_metric_ids(self: &Arc<Self>) -> Vec<MetricId> {
        let mut ids: Vec<MetricId> = metrics::registry()
            .into_iter()
            .filter(|d| !d.id.contains('<'))
            .filter_map(|d| MetricId::new(d.id).ok())
            .collect();

        let snapshot = self.factory_snapshot();
        let policies_dir = snapshot.policies_dir();
        let goals_dir = factory_core::goals::goals_dir(&snapshot.root);
        let (frameworks, catalogue): (Vec<String>, GoalsCatalogue) = tokio::task::spawn_blocking(move || {
            let (catalogues, _findings) = factory_core::policy::load_all(&policies_dir);
            let frameworks = catalogues.into_iter().map(|c| c.framework).collect();
            let catalogue = factory_core::goals::load(&goals_dir);
            (frameworks, catalogue)
        })
        .await
        .unwrap_or_default();

        for framework in &frameworks {
            if let Ok(id) = MetricId::new(format!("compliance.{framework}")) {
                ids.push(id);
            }
            if let Ok(id) = MetricId::new(format!("open_controls.{framework}")) {
                ids.push(id);
            }
        }

        ids.extend(goals_metric_ids(&catalogue));

        let mut seen = BTreeSet::new();
        ids.retain(|id| seen.insert(id.clone()));
        ids
    }

    async fn bench_resolve_rate_value(&self, id: &MetricId, dataset: &str, now: DateTime<Utc>) -> Result<MetricValue> {
        let runs = self.bench.runs(Some(dataset), 200).await?;
        let Some(run) = runs.into_iter().find(|r| r.settled()) else {
            return Ok(MetricValue {
                id: id.clone(),
                value: None,
                as_of: now,
                reason: Some(format!("no settled bench run for dataset {dataset:?}")),
            });
        };
        let results = factory_core::bench::aggregate(&run.attempts);
        let (pass, fail) = results.iter().fold((0u32, 0u32), |(p, f), r| (p + r.pass, f + r.fail));
        if pass + fail == 0 {
            return Ok(MetricValue {
                id: id.clone(),
                value: None,
                as_of: now,
                reason: Some(format!("nothing gated in the newest settled run of {dataset:?}")),
            });
        }
        Ok(MetricValue {
            id: id.clone(),
            value: Some(f64::from(pass) / f64::from(pass + fail)),
            as_of: now,
            reason: None,
        })
    }

    /// Unscoped, like `production.rs`'s own read: a goal label names an
    /// objective/key-result pair, not a scope, and the objective it belongs
    /// to may itself carry any scope (or none). `ScopedStores::list` with no
    /// `scope` in the filter fans out over every configured store and
    /// merges the result, so this counts correctly regardless of where the
    /// labelled tasks actually live.
    async fn goal_tasks_done_value(&self, id: &MetricId, objective: &str, kr: &str, now: DateTime<Utc>) -> Result<MetricValue> {
        let label = format!("{objective}/{kr}");
        let tasks = self.store.list(&TaskFilter::default()).await?;
        let count = tasks
            .iter()
            .filter(|t| t.labels.get("goal").map(String::as_str) == Some(label.as_str()) && t.status == TaskStatus::Done)
            .count();
        Ok(MetricValue {
            id: id.clone(),
            value: Some(count as f64),
            as_of: now,
            reason: None,
        })
    }
}

/// Every metric id `catalogue` itself names -- `direction.yaml`'s
/// `north_star`/`inputs` and every key result's own bound metric -- filtered
/// to ids `metrics::resolve` has at least heard of (`Unknown` ones are
/// dropped; `Unavailable` ones are kept, so they still come back with their
/// reason rather than silently vanishing from a report).
pub(crate) fn goals_metric_ids(catalogue: &GoalsCatalogue) -> Vec<MetricId> {
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

/// Sum `finished`/`scrapped`/`reworked` over a `window`-day trailing window
/// ending at each day in `daily` (clipped at the start of the grid, so the
/// earliest few points are over a shorter window than `window`), and hand
/// each sum to `calc` -- `None` skips that day's point entirely (used for a
/// ratio with nothing finished yet to divide by) rather than fabricating a
/// number.
fn rolling_series<F>(daily: &[factory_core::protocol::ProductionBucket], window: usize, calc: F) -> Vec<(NaiveDate, f64)>
where
    F: Fn(u32, u32, u32) -> Option<f64>,
{
    let mut points = Vec::new();
    for i in 0..daily.len() {
        let start = i.saturating_sub(window.saturating_sub(1));
        let (mut finished, mut scrapped, mut reworked) = (0u32, 0u32, 0u32);
        for bucket in &daily[start..=i] {
            finished += bucket.finished;
            scrapped += bucket.scrapped;
            reworked += bucket.reworked;
        }
        if let Some(value) = calc(finished, scrapped, reworked) {
            points.push((daily[i].from.date_naive(), value));
        }
    }
    points
}

fn throughput_week_series(daily: &[factory_core::protocol::ProductionBucket]) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("throughput_week").expect("fixed id"),
        points: rolling_series(daily, 7, |finished, _, _| Some(f64::from(finished))),
    }
}

fn first_pass_yield_series(daily: &[factory_core::protocol::ProductionBucket]) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("first_pass_yield").expect("fixed id"),
        points: rolling_series(daily, 28, |finished, _, reworked| {
            if finished == 0 {
                None
            } else {
                Some(1.0 - f64::from(reworked) / f64::from(finished))
            }
        }),
    }
}

fn scrap_rate_series(daily: &[factory_core::protocol::ProductionBucket]) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("scrap_rate").expect("fixed id"),
        points: rolling_series(daily, 28, |finished, scrapped, _| {
            if finished == 0 {
                None
            } else {
                Some(f64::from(scrapped) / f64::from(finished))
            }
        }),
    }
}

/// A series' own last point is always the metric's current value -- this is
/// the one place that invariant is enforced, so `throughput_week`/
/// `first_pass_yield`/`scrap_rate` can never disagree with their own
/// sparkline.
fn value_from_series(series: &MetricSeries, now: DateTime<Utc>, empty_reason: &str) -> MetricValue {
    let value = series.points.last().map(|(_, v)| *v);
    MetricValue {
        id: series.id.clone(),
        value,
        as_of: now,
        reason: if value.is_none() { Some(empty_reason.to_string()) } else { None },
    }
}

fn compliance_value(id: &MetricId, report: &PolicyReport, framework: &str, now: DateTime<Utc>) -> MetricValue {
    let Some(rollup) = report.rollup.iter().find(|r| r.framework == framework) else {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!("no catalogue loaded for framework {framework:?}")),
        };
    };
    let c = &rollup.counts;
    let counted = c.satisfied + c.attested + c.stale + c.open + c.not_applicable;
    if counted == 0 {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!("framework {framework:?} has no regulation/standard controls counted")),
        };
    }
    let value = (c.satisfied + c.attested + c.not_applicable) as f64 / counted as f64;
    MetricValue { id: id.clone(), value: Some(value), as_of: now, reason: None }
}

fn open_controls_value(id: &MetricId, report: &PolicyReport, framework: &str, now: DateTime<Utc>) -> MetricValue {
    let Some(rollup) = report.rollup.iter().find(|r| r.framework == framework) else {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!("no catalogue loaded for framework {framework:?}")),
        };
    };
    let c = &rollup.counts;
    MetricValue {
        id: id.clone(),
        value: Some((c.open + c.stale) as f64),
        as_of: now,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    //! Engine-level computation, on a temporary instance -- not
    //! `factory_core::metrics` itself (covered on its own), but this module
    //! glued to real data the way a real request sees it: real runs and
    //! tasks in the store, and a real policy catalogue on disk.

    use super::*;
    use factory_core::adapter::store::task_from_new;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, PolicyDeclaration, Scope};
    use factory_core::run::{NewRun, RunPatch, RunStatus, Trigger};
    use factory_core::task::{NewTask, TaskPatch};
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;

    fn scope_at(id: &str, name: &str, path: &str) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(&format!("id: {id}\nname: {name}\n")).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    /// A one-scope instance, with `frameworks` (if any) committed at the
    /// root the same way `policies:` at the instance root always does. The
    /// root scope is declared (`config.scope`), so `goals.checkin`-shaped
    /// authorization checks in other modules' tests can reuse this too.
    fn test_engine(frameworks: Vec<String>) -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-metrics-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let root_scope = scope_at("root-id", "root", ".");
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(root_scope.clone()),
            scopes: vec![root_scope],
            roles: Default::default(),
            policies: PolicyDeclaration { frameworks, ..Default::default() },
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        let factory = Factory { root, config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    /// A finished run for a fresh task in `root`: `attempt`-th attempt (so a
    /// second call for the same `label` is the rework signal), `status`,
    /// ended `ended_ago` before now. Returns the task so a caller can patch
    /// its own status afterward (`goal_tasks_done`'s own tests).
    async fn finished_run(engine: &Arc<Engine>, label: &str, status: RunStatus, ended_ago: chrono::Duration) -> factory_core::task::Task {
        let existing = engine
            .store
            .list(&Default::default())
            .await
            .unwrap()
            .into_iter()
            .find(|t: &factory_core::task::Task| t.title == label);
        let task = match existing {
            Some(t) => t,
            None => {
                let new_task = task_from_new(
                    NewTask { title: label.to_string(), ..Default::default() },
                    "root".to_string(),
                    "assistant".to_string(),
                    "shell".to_string(),
                );
                engine.store.create(&new_task).await.unwrap()
            }
        };
        let run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "assistant".to_string(),
                adapter: "shell".to_string(),
                runtime: "shell".to_string(),
                token: "tok".to_string(),
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(status),
                    ended_at: Some(Utc::now() - ended_ago),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        task
    }

    // ------------------------------------------------------- production

    #[tokio::test]
    async fn throughput_yield_and_scrap_are_computed_from_real_runs() {
        let engine = test_engine(Vec::new());
        // One clean finish, one reworked finish (a second attempt on the
        // same task), one scrapped finish (failed) -- three finished runs
        // total, one reworked, one scrapped.
        finished_run(&engine, "clean", RunStatus::Done, chrono::Duration::hours(1)).await;
        finished_run(&engine, "reworked", RunStatus::Done, chrono::Duration::hours(2)).await;
        finished_run(&engine, "reworked", RunStatus::Done, chrono::Duration::hours(1)).await;
        finished_run(&engine, "scrapped", RunStatus::Failed, chrono::Duration::hours(1)).await;

        let now = Utc::now();
        let ids = vec![
            MetricId::new("throughput_week").unwrap(),
            MetricId::new("first_pass_yield").unwrap(),
            MetricId::new("scrap_rate").unwrap(),
        ];
        let computed = engine.metrics(&ids, now).await.unwrap();

        let get = |id: &str| computed.values.iter().find(|v| v.id.as_str() == id).unwrap().clone();
        assert_eq!(get("throughput_week").value, Some(4.0), "four finished runs in the trailing week");
        assert_eq!(get("first_pass_yield").value, Some(1.0 - 1.0 / 4.0), "one of four finished was reworked");
        assert_eq!(get("scrap_rate").value, Some(1.0 / 4.0), "one of four finished was scrapped");

        // The series' own last point always equals the metric's value.
        for id in ["throughput_week", "first_pass_yield", "scrap_rate"] {
            let series = computed.series.iter().find(|s| s.id.as_str() == id).unwrap();
            let last = series.points.last().unwrap().1;
            assert_eq!(Some(last), get(id).value, "{id}'s series must end on its own value");
        }
    }

    #[tokio::test]
    async fn no_finished_runs_gives_none_with_a_reason() {
        let engine = test_engine(Vec::new());
        let now = Utc::now();
        let ids = vec![MetricId::new("first_pass_yield").unwrap(), MetricId::new("scrap_rate").unwrap()];
        let computed = engine.metrics(&ids, now).await.unwrap();
        for v in &computed.values {
            assert_eq!(v.value, None, "{}", v.id);
            assert!(v.reason.is_some(), "{} should carry a reason", v.id);
        }
    }

    // ---------------------------------------------------------- compliance

    #[tokio::test]
    async fn compliance_and_open_controls_read_the_policy_rollup() {
        let engine = test_engine(vec!["cra".to_string()]);
        {
            let snapshot = engine.factory_snapshot();
            std::fs::create_dir_all(snapshot.policies_dir()).unwrap();
            std::fs::write(
                snapshot.policies_dir().join("cra.yaml"),
                "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
                 \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: knowledge\n\
                 \x20\x20- id: b\n\x20\x20\x20\x20title: B\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: attestation\n",
            )
            .unwrap();
            std::fs::create_dir_all(snapshot.root.join(".factory/knowledge")).unwrap();
            std::fs::write(
                snapshot.root.join(".factory/knowledge/page.md"),
                "---\ntags: [control/cra/a]\n---\n# Page\n",
            )
            .unwrap();
        }

        let now = Utc::now();
        let ids = vec![MetricId::new("compliance.cra").unwrap(), MetricId::new("open_controls.cra").unwrap()];
        let computed = engine.metrics(&ids, now).await.unwrap();
        let compliance = computed.values.iter().find(|v| v.id.as_str() == "compliance.cra").unwrap();
        let open = computed.values.iter().find(|v| v.id.as_str() == "open_controls.cra").unwrap();
        assert_eq!(compliance.value, Some(0.5), "one of two counted controls is satisfied");
        assert_eq!(open.value, Some(1.0), "the other is open");
    }

    #[tokio::test]
    async fn an_unloaded_framework_is_none_with_a_reason() {
        let engine = test_engine(Vec::new());
        let now = Utc::now();
        let computed = engine.metrics(&[MetricId::new("compliance.nope").unwrap()], now).await.unwrap();
        assert_eq!(computed.values[0].value, None);
        assert!(computed.values[0].reason.as_deref().unwrap().contains("nope"));
    }

    // ------------------------------------------------------- bench.resolve_rate

    /// A settled bench run with two attempts (one pass, one fail) across two
    /// configurations, summed into one resolve rate for the whole run --
    /// `bench.resolve_rate.<dataset>` names no agent or configuration of its
    /// own, unlike `bench show`'s own per-configuration table.
    #[tokio::test]
    async fn bench_resolve_rate_sums_pass_and_fail_across_every_configuration_in_the_newest_settled_run() {
        use factory_core::bench::{BenchAttempt, BenchRun, BenchRunStatus, Verdict};

        let engine = test_engine(Vec::new());
        let mut pass = BenchAttempt::pending("a1".into(), "case1".into(), "agent-a".into(), 1);
        pass.verdict = Some(Verdict::Pass);
        let mut fail = BenchAttempt::pending("a2".into(), "case2".into(), "agent-b".into(), 1);
        fail.verdict = Some(Verdict::Fail);
        let run = BenchRun {
            id: "run-1".into(),
            dataset: "eval-set-a".into(),
            dataset_revision: 1,
            cases: Vec::new(),
            case_bases: Default::default(),
            agents: vec!["agent-a".into(), "agent-b".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Done,
            attempts: vec![pass.clone(), fail.clone()],
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
        };
        // `put_run` never trusts `attempts` embedded on the struct -- they
        // live in their own table (`BenchStore::attempts`), written
        // separately, the same way `bench::engine` itself writes them.
        engine.bench.put_run(&run).await.unwrap();
        engine.bench.put_attempt(&run.id, &pass).await.unwrap();
        engine.bench.put_attempt(&run.id, &fail).await.unwrap();

        let now = Utc::now();
        let id = MetricId::new("bench.resolve_rate.eval-set-a").unwrap();
        let computed = engine.metrics(&[id], now).await.unwrap();
        assert_eq!(computed.values[0].value, Some(0.5), "one pass and one fail across two configurations");
    }

    #[tokio::test]
    async fn bench_resolve_rate_is_none_with_a_reason_when_no_settled_run_exists() {
        let engine = test_engine(Vec::new());
        let now = Utc::now();
        let id = MetricId::new("bench.resolve_rate.eval-set-a").unwrap();
        let computed = engine.metrics(&[id], now).await.unwrap();
        assert_eq!(computed.values[0].value, None);
        assert!(computed.values[0].reason.as_deref().unwrap().contains("no settled bench run"));
    }

    // ------------------------------------------------------- goal_tasks_done

    #[tokio::test]
    async fn goal_tasks_done_counts_only_the_labelled_and_done_tasks() {
        let engine = test_engine(Vec::new());

        let mut labelled_done = NewTask { title: "done".into(), ..Default::default() };
        labelled_done.labels.insert("goal".to_string(), "ship-compliant/cra-open-zero".to_string());
        let t1 = engine
            .store
            .create(&task_from_new(labelled_done, "root".to_string(), "assistant".to_string(), "shell".to_string()))
            .await
            .unwrap();
        engine
            .store
            .update(&t1.id, &TaskPatch { status: Some(factory_core::task::TaskStatus::Done), ..Default::default() })
            .await
            .unwrap();

        let mut labelled_running = NewTask { title: "running".into(), ..Default::default() };
        labelled_running.labels.insert("goal".to_string(), "ship-compliant/cra-open-zero".to_string());
        engine
            .store
            .create(&task_from_new(labelled_running, "root".to_string(), "assistant".to_string(), "shell".to_string()))
            .await
            .unwrap();

        let mut other_label_done = NewTask { title: "other".into(), ..Default::default() };
        other_label_done.labels.insert("goal".to_string(), "raise-quality/fpy-90".to_string());
        let t3 = engine
            .store
            .create(&task_from_new(other_label_done, "root".to_string(), "assistant".to_string(), "shell".to_string()))
            .await
            .unwrap();
        engine
            .store
            .update(&t3.id, &TaskPatch { status: Some(factory_core::task::TaskStatus::Done), ..Default::default() })
            .await
            .unwrap();

        let now = Utc::now();
        let id = MetricId::new("goal_tasks_done.ship-compliant.cra-open-zero").unwrap();
        let computed = engine.metrics(&[id], now).await.unwrap();
        assert_eq!(computed.values[0].value, Some(1.0), "only the done task under this exact objective/kr counts");
    }

    // ---------------------------------------------------- unknown/unavailable

    #[tokio::test]
    async fn an_unknown_id_refuses_the_whole_call() {
        let engine = test_engine(Vec::new());
        let err = engine.metrics(&[MetricId::new("bogus_metric").unwrap()], Utc::now()).await.unwrap_err();
        assert!(err.to_string().contains("bogus_metric"), "{err}");
    }

    #[tokio::test]
    async fn an_unavailable_id_comes_back_as_none_with_the_registrys_reason() {
        let engine = test_engine(Vec::new());
        let computed = engine.metrics(&[MetricId::new("unit_cost").unwrap()], Utc::now()).await.unwrap();
        assert_eq!(computed.values[0].value, None);
        assert!(computed.values[0].reason.as_deref().unwrap().contains("design §12.6"));
        assert!(!computed.registry[0].available);
    }

    // -------------------------------------------------------- default ids

    #[tokio::test]
    async fn default_ids_include_unavailable_fixed_metrics_and_what_the_catalogues_imply() {
        let engine = test_engine(vec!["cra".to_string()]);
        {
            let snapshot = engine.factory_snapshot();
            std::fs::create_dir_all(snapshot.policies_dir()).unwrap();
            std::fs::write(
                snapshot.policies_dir().join("cra.yaml"),
                "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n\
                 \x20\x20- id: a\n\x20\x20\x20\x20title: A\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: knowledge\n",
            )
            .unwrap();
            let goals_dir = factory_core::goals::goals_dir(&snapshot.root);
            std::fs::create_dir_all(&goals_dir).unwrap();
            std::fs::write(
                goals_dir.join("direction.yaml"),
                "vision: V\nmission: M\nnorth_star: {metric: first_pass_yield, why: because}\ninputs: [throughput_week]\n",
            )
            .unwrap();
            std::fs::write(
                goals_dir.join("2026-q4.yaml"),
                "cycle: { id: 2026-q4, from: 2026-10-01, to: 2026-12-31 }\n\
                 objectives:\n\
                 \x20\x20- id: obj\n\x20\x20\x20\x20title: T\n\x20\x20\x20\x20key_results:\n\
                 \x20\x20\x20\x20\x20\x20- {id: kr, title: K, kind: committed, metric: compliance.cra, baseline: 0, target: 1}\n",
            )
            .unwrap();
        }

        let ids = engine.default_metric_ids().await;
        let has = |s: &str| ids.iter().any(|id| id.as_str() == s);
        assert!(has("throughput_week"), "fixed metrics are always in the default set");
        assert!(has("unit_cost"), "an unavailable fixed metric is still 'non-parameterised'");
        assert!(has("tokens_per_run"));
        assert!(has("compliance.cra"), "implied by the loaded policy catalogue");
        assert!(has("open_controls.cra"));
        assert!(has("compliance.cra"), "also implied by the goals catalogue's own key result");
    }
}
