//! What the L5 Metrics tab lists (`#333`): every metric a scope is measured
//! by, where each value comes from, and who reads it. The measuring is L5's;
//! a policy control, a quality scenario or a goals key result only judges
//! against it -- and each is judged here with the very arithmetic it uses
//! itself (`metrics::judge_metric`), so a row can never say "met" where the
//! consumer's own tab says otherwise.
//!
//! Every input is plain and fresh per read: L6's applicable policy subjects
//! arrive as the same [`PolicyInputs`] the metric service already takes,
//! key results as [`GoalKeyResult`]s L6 projects, and quality measures come
//! from L5's own [`quality_inputs::Inputs`]. Nothing here reads a value
//! itself -- [`rows`] is handed the values one ordinary metrics read
//! computed, so a service source is still asked at most once.
use crate::{
    checks::Check,
    metrics::{self, MetricBounds, MetricError, MetricId, MetricJudgement, MetricValue},
    metrics_service::{self, PolicyInputs},
    quality::{self, Measure},
    quality_inputs, reported,
};
use chrono::{DateTime, Utc};
use factory_kernel::{Duration, Result, ScopeTree};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

// ================================================================ readers

/// Which kind of authored thing reads a metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerKind {
    /// A policy control's `check: metric`, `<framework>/<control>`.
    Policy,
    /// A quality scenario's metric `measure`, `<attribute>/<scenario>`.
    Quality,
    /// A goals key result bound to a metric, `<objective>/<kr>`.
    Goal,
}

/// One authored reader of one metric, with the bounds it holds the value to.
/// `draft` marks a quality measure with no finite threshold -- the Quality
/// tab's own `draft`, never judged met.
#[derive(Debug, Clone)]
pub struct Reader {
    pub kind: ConsumerKind,
    pub id: String,
    pub scope: Option<String>,
    pub metric: MetricId,
    pub bounds: MetricBounds,
    pub draft: bool,
}

/// One current-cycle key result bound to a metric, as L6 projects it: L5
/// cannot see the goals catalogue, and needs no more than this to judge it.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalKeyResult {
    /// `<objective>/<kr>`, the id a check-in names it by.
    pub id: String,
    pub metric: MetricId,
    pub baseline: f64,
    pub target: f64,
}

/// A metric this build can at least name -- an unknown id would refuse the
/// whole metrics read, and its own finding already says so where it was
/// written (`check_vocabulary`, `goals::load`, the quality loader).
fn known(id: &MetricId) -> bool {
    !matches!(metrics::resolve(id), Err(MetricError::Unknown(_)))
}

/// Every `check: metric` of every applicable, non-exempt control, at the
/// scope it applies in. `max_age` is the control's own effective window
/// (`EvaluationSubject::max_age`, every layer's tightening already folded
/// in) -- exactly what `checks::direct_status` judges it against. A circular
/// id never reaches here: `check_vocabulary` refuses it, and so does this.
pub fn policy_readers(inputs: &PolicyInputs) -> Vec<Reader> {
    let mut out = Vec::new();
    for scope in &inputs.scopes {
        for subject in &scope.subjects {
            if subject.not_applicable.is_some() {
                continue;
            }
            for check in &subject.evidence {
                let Check::Metric {
                    metric,
                    above,
                    below,
                    ..
                } = check
                else {
                    continue;
                };
                if crate::checks::is_circular_metric(metric) || !known(metric) {
                    continue;
                }
                out.push(Reader {
                    kind: ConsumerKind::Policy,
                    id: subject.control.to_string(),
                    scope: Some(scope.scope.name.clone()),
                    metric: metric.clone(),
                    bounds: MetricBounds {
                        above: *above,
                        below: *below,
                        max_age: subject.max_age,
                    },
                    draft: false,
                });
            }
        }
    }
    out
}

/// Every metric `measure` in every bound scope's utility tree. `quality.*`
/// is left out, as `quality_inputs::metric_ids` leaves it out: a scenario
/// measured by a quality rollup would be circular.
pub fn quality_readers(inputs: &quality_inputs::Inputs) -> Vec<Reader> {
    let mut out = Vec::new();
    for (scope, tree) in &inputs.trees {
        for attribute in &tree.attributes {
            for applied in &attribute.scenarios {
                let Some(Measure::Metric(measure)) = &applied.scenario.measure else {
                    continue;
                };
                if quality::is_quality_metric(&measure.metric) || !known(&measure.metric) {
                    continue;
                }
                out.push(Reader {
                    kind: ConsumerKind::Quality,
                    id: format!("{}/{}", attribute.id, applied.scenario.id),
                    scope: Some(scope.scope.name.clone()),
                    metric: measure.metric.clone(),
                    bounds: MetricBounds {
                        above: measure.above,
                        below: measure.below,
                        max_age: measure.max_age,
                    },
                    draft: !quality::is_judgeable(&Measure::Metric(measure.clone())),
                });
            }
        }
    }
    out
}

/// Each key result as a reader: met once its value reaches `target` from
/// `baseline`'s side -- `>= target` when the target lies above the baseline,
/// `<= target` when below -- with no freshness window of its own.
pub fn goal_readers(key_results: &[GoalKeyResult], scope: Option<&str>) -> Vec<Reader> {
    key_results
        .iter()
        .filter(|kr| known(&kr.metric))
        .map(|kr| {
            let (above, below) = if kr.target >= kr.baseline {
                (Some(kr.target), None)
            } else {
                (None, Some(kr.target))
            };
            Reader {
                kind: ConsumerKind::Goal,
                id: kr.id.clone(),
                scope: scope.map(str::to_string),
                metric: kr.metric.clone(),
                bounds: MetricBounds {
                    above,
                    below,
                    max_age: None,
                },
                draft: false,
            }
        })
        .collect()
}

/// The ids a scoped listing computes: every reader's metric, plus every
/// `reported.<source>.<metric>` a scope in `members` declares itself --
/// deduplicated and sorted, so the same selection always lists the same way.
pub fn listed_ids(
    readers: &[Reader],
    catalogue: &reported::Catalogue,
    members: &BTreeSet<String>,
) -> Vec<MetricId> {
    let mut ids: BTreeSet<MetricId> = readers.iter().map(|r| r.metric.clone()).collect();
    for source in catalogue.sources.values() {
        if !members.contains(&source.scope.name) {
            continue;
        }
        for metric_id in source.declared.keys() {
            if let Ok(id) = MetricId::new(format!("reported.{}.{metric_id}", source.id)) {
                ids.insert(id);
            }
        }
    }
    ids.into_iter().collect()
}

// =================================================================== rows

/// Where a value comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MetricSource {
    /// Computed from what Factory itself records.
    BuiltIn,
    /// A scope's own file; `path` is relative to the instance root.
    File {
        source: String,
        scope: String,
        path: String,
    },
    /// A scope's declared network dependency service, read over loopback.
    Service {
        source: String,
        scope: String,
        service: String,
        endpoint: String,
        path: String,
        url: String,
    },
    /// A `reported.*` id no valid declaration answers for: an undeclared
    /// source, or a service source [`reported::validate`] refused.
    Unresolved {
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        service: Option<String>,
        reason: String,
    },
}

/// A consumer's verdict on the value it reads -- `metrics::MetricJudgement`,
/// plus `draft` for a quality measure with no finite threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerStatus {
    Met,
    NotMet,
    Stale,
    NoData,
    Draft,
}

/// One reader of a row's metric, judged against its own bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Consumer {
    pub kind: ConsumerKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub status: ConsumerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<Duration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

/// A value's age against the strictest window any consumer holds it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale,
    /// No value to be fresh or stale.
    NoData,
    /// No consumer gives a `max_age`.
    NoWindow,
}

/// One listed metric: where its value comes from, how fresh it is, and every
/// consumer's verdict. The value, title, unit and direction are the same
/// response's own `values`/`registry` entries for `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricRow {
    pub id: MetricId,
    pub source: MetricSource,
    pub freshness: Freshness,
    /// The shortest `max_age` among the consumers, the one `freshness` is
    /// measured against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<Duration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumers: Vec<Consumer>,
}

/// Where `id`'s value comes from, off the same validated declarations the
/// read itself used. `root` makes a file path relative for display.
pub fn source_of(id: &MetricId, catalogue: &reported::Catalogue, root: &Path) -> MetricSource {
    let Some((source_id, _)) = id.as_str().strip_prefix("reported.").and_then(|r| r.split_once('.'))
    else {
        return MetricSource::BuiltIn;
    };
    let Some(source) = catalogue.sources.get(source_id) else {
        return MetricSource::Unresolved {
            source: source_id.to_string(),
            scope: None,
            service: None,
            reason: format!("no scope declares a metrics source {source_id:?}"),
        };
    };
    let scope = source.scope.name.clone();
    match &source.location {
        reported::Location::File(path) => MetricSource::File {
            source: source.id.clone(),
            scope,
            path: path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string(),
        },
        reported::Location::Service(target) => MetricSource::Service {
            source: source.id.clone(),
            scope,
            service: target.service.clone(),
            endpoint: target.endpoint.clone(),
            path: target.path.clone(),
            url: target.url(),
        },
        reported::Location::Refused { service, reason } => MetricSource::Unresolved {
            source: source.id.clone(),
            scope: Some(scope),
            service: service.clone(),
            reason: reason.clone(),
        },
    }
}

/// One row per value, in `values`' own order. A policy consumer reads a
/// `reported.*` value as the scope it applies in sees it -- `None` when the
/// source is declared outside that scope's subtree -- the same narrowing
/// the Policy tab applies (`metrics_service::metrics_for_evaluated_scope`).
pub fn rows(
    values: &[MetricValue],
    readers: &[Reader],
    catalogue: &reported::Catalogue,
    scopes: &ScopeTree,
    root: &Path,
    now: DateTime<Utc>,
) -> Result<Vec<MetricRow>> {
    let mut out = Vec::new();
    for value in values {
        let mut consumers = Vec::new();
        for reader in readers.iter().filter(|r| r.metric == value.id) {
            let seen = match (&reader.kind, &reader.scope) {
                (ConsumerKind::Policy, Some(scope)) => {
                    let subtree = metrics_service::subtree_names(scopes, scope)?;
                    let one = std::iter::once((value.id.clone(), value.clone())).collect();
                    metrics_service::metrics_for_evaluated_scope(&one, catalogue, &subtree, now)
                        .remove(&value.id)
                        .unwrap_or_else(|| value.clone())
                }
                _ => value.clone(),
            };
            consumers.push(judge(reader, &seen, now));
        }
        consumers.sort_by(|a, b| {
            (a.kind, &a.scope, &a.id).cmp(&(b.kind, &b.scope, &b.id))
        });
        let max_age = readers
            .iter()
            .filter(|r| r.metric == value.id)
            .filter_map(|r| r.bounds.max_age)
            .min_by_key(|d| d.as_time_delta());
        let freshness = match (value.value.filter(|v| v.is_finite()), max_age) {
            (None, _) => Freshness::NoData,
            (Some(_), None) => Freshness::NoWindow,
            (Some(_), Some(window)) if now - value.as_of > window.as_time_delta() => {
                Freshness::Stale
            }
            (Some(_), Some(_)) => Freshness::Fresh,
        };
        out.push(MetricRow {
            id: value.id.clone(),
            source: source_of(&value.id, catalogue, root),
            freshness,
            max_age,
            consumers,
        });
    }
    Ok(out)
}

fn judge(reader: &Reader, value: &MetricValue, now: DateTime<Utc>) -> Consumer {
    let (status, reasons) = if reader.draft {
        (
            ConsumerStatus::Draft,
            vec![format!("the measure on {} has no finite threshold", reader.metric)],
        )
    } else {
        let (judgement, reasons, _) = metrics::judge_metric(&reader.metric, reader.bounds, value, now);
        let status = match judgement {
            MetricJudgement::Met => ConsumerStatus::Met,
            MetricJudgement::NotMet => ConsumerStatus::NotMet,
            MetricJudgement::Stale => ConsumerStatus::Stale,
            MetricJudgement::NoData => ConsumerStatus::NoData,
        };
        (status, reasons)
    };
    Consumer {
        kind: reader.kind,
        id: reader.id.clone(),
        scope: reader.scope.clone(),
        status,
        above: reader.bounds.above,
        below: reader.bounds.below,
        max_age: reader.bounds.max_age,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::{EvaluationSubject, Exemption};
    use crate::metrics_service::PolicyScope;
    use factory_kernel::{ControlRef, ScopeNode};
    use std::{collections::BTreeMap, path::PathBuf};

    fn id(s: &str) -> MetricId {
        MetricId::new(s).unwrap()
    }

    fn node(name: &str, path: &str) -> ScopeNode {
        ScopeNode {
            name: name.into(),
            path: path.into(),
        }
    }

    fn tree() -> ScopeTree {
        ScopeTree {
            scopes: vec![
                node("company", "."),
                node("finance", "projects/finance"),
                node("ops", "projects/ops"),
            ],
        }
    }

    fn subject(control: &str, checks: Vec<Check>, max_age: Option<&str>) -> EvaluationSubject<bool> {
        let (framework, id) = control.split_once('/').unwrap();
        EvaluationSubject {
            control: ControlRef::new(framework, id),
            title: control.into(),
            kind: true,
            maps_to: Vec::new(),
            evidence: checks,
            max_age: max_age.map(|d| d.parse().unwrap()),
            not_applicable: None,
        }
    }

    fn metric_check(metric: &str, above: Option<f64>, below: Option<f64>) -> Check {
        Check::Metric {
            metric: id(metric),
            above,
            below,
            max_age: None,
        }
    }

    fn policy(scope: &str, subjects: Vec<EvaluationSubject<bool>>) -> PolicyInputs {
        PolicyInputs {
            scopes: vec![PolicyScope {
                scope: node(scope, &format!("projects/{scope}")),
                subjects,
                budget: None,
            }],
            attestations: Vec::new(),
        }
    }

    fn value(metric: &str, v: Option<f64>, as_of: DateTime<Utc>) -> MetricValue {
        MetricValue {
            id: id(metric),
            value: v,
            as_of,
            reason: v.is_none().then(|| "nothing yet".to_string()),
        }
    }

    fn catalogue(location: reported::Location) -> reported::Catalogue {
        let mut declared = BTreeMap::new();
        declared.insert(
            "bookings_without_beleg".to_string(),
            reported::DeclaredMetric {
                title: "Bookings without Beleg".into(),
                unit: metrics::Unit::Count,
                better: metrics::Better::Lower,
            },
        );
        let mut sources = BTreeMap::new();
        sources.insert(
            "finance".to_string(),
            reported::ValidSource {
                id: "finance".into(),
                scope: node("finance", "projects/finance"),
                location,
                declared,
            },
        );
        reported::Catalogue {
            sources,
            findings: Vec::new(),
        }
    }

    fn fava() -> reported::Location {
        reported::Location::Service(reported::ServiceTarget {
            service: "fava".into(),
            endpoint: "127.0.0.1:8793".into(),
            host: reported::LoopbackHost::V4,
            port: 8793,
            path: "/metrics".into(),
        })
    }

    #[test]
    fn policy_readers_take_metric_checks_with_the_controls_effective_window_and_skip_the_rest() {
        let mut exempt = subject("gobd/exempt", vec![metric_check("fail_rate", None, Some(0.1))], None);
        exempt.not_applicable = Some(Exemption {
            scope: "finance".into(),
            rationale: "test".into(),
        });
        let inputs = policy(
            "finance",
            vec![
                subject(
                    "gobd/belegprinzip",
                    vec![
                        metric_check("reported.finance.bookings_without_beleg", None, Some(0.0)),
                        Check::Attestation,
                    ],
                    Some("7d"),
                ),
                subject("gobd/circular", vec![metric_check("compliance.gobd", Some(1.0), None)], None),
                subject("gobd/unknown", vec![metric_check("no_such_metric", Some(1.0), None)], None),
                exempt,
            ],
        );
        let readers = policy_readers(&inputs);
        assert_eq!(readers.len(), 1, "{readers:?}");
        let reader = &readers[0];
        assert_eq!(reader.kind, ConsumerKind::Policy);
        assert_eq!(reader.id, "gobd/belegprinzip");
        assert_eq!(reader.scope.as_deref(), Some("finance"));
        assert_eq!(reader.bounds.below, Some(0.0));
        assert_eq!(reader.bounds.max_age.unwrap().as_hours(), 7 * 24);
    }

    #[test]
    fn a_key_result_is_met_from_its_baselines_side_of_the_target() {
        let readers = goal_readers(
            &[
                GoalKeyResult {
                    id: "books/coverage".into(),
                    metric: id("first_pass_yield"),
                    baseline: 0.5,
                    target: 0.9,
                },
                GoalKeyResult {
                    id: "books/backlog".into(),
                    metric: id("fail_rate"),
                    baseline: 0.4,
                    target: 0.1,
                },
            ],
            Some("company"),
        );
        assert_eq!((readers[0].bounds.above, readers[0].bounds.below), (Some(0.9), None));
        assert_eq!((readers[1].bounds.above, readers[1].bounds.below), (None, Some(0.1)));
        assert!(readers.iter().all(|r| r.bounds.max_age.is_none()));
    }

    #[test]
    fn listed_ids_are_the_readers_metrics_plus_the_selections_own_declared_ones() {
        let readers = policy_readers(&policy(
            "ops",
            vec![subject("house/fails", vec![metric_check("fail_rate", None, Some(0.2))], None)],
        ));
        let catalogue = catalogue(fava());
        let finance: BTreeSet<String> = ["finance".to_string()].into();
        let ops: BTreeSet<String> = ["ops".to_string()].into();
        assert_eq!(
            listed_ids(&readers, &catalogue, &finance),
            vec![id("fail_rate"), id("reported.finance.bookings_without_beleg")]
        );
        assert_eq!(listed_ids(&readers, &catalogue, &ops), vec![id("fail_rate")]);
    }

    #[test]
    fn each_consumer_is_judged_against_its_own_bounds_and_freshness_against_the_strictest_window() {
        let now = Utc::now();
        let metric = "reported.finance.bookings_without_beleg";
        let mut readers = policy_readers(&policy(
            "finance",
            vec![
                subject("gobd/belegprinzip", vec![metric_check(metric, None, Some(0.0))], Some("1d")),
                subject("gobd/lenient", vec![metric_check(metric, None, Some(5.0))], Some("30d")),
            ],
        ));
        readers.extend(goal_readers(
            &[GoalKeyResult {
                id: "books/clean".into(),
                metric: id(metric),
                baseline: 20.0,
                target: 0.0,
            }],
            Some("company"),
        ));
        let catalogue = catalogue(fava());
        let fresh = [value(metric, Some(3.0), now - chrono::Duration::hours(2))];
        let rows = rows(&fresh, &readers, &catalogue, &tree(), Path::new("/i"), now).unwrap();
        let row = &rows[0];
        assert_eq!(row.max_age.unwrap().to_string(), "1d");
        assert_eq!(row.freshness, Freshness::Fresh);
        let status = |id: &str| row.consumers.iter().find(|c| c.id == id).unwrap().status;
        assert_eq!(status("gobd/belegprinzip"), ConsumerStatus::NotMet);
        assert_eq!(status("gobd/lenient"), ConsumerStatus::Met);
        assert_eq!(status("books/clean"), ConsumerStatus::NotMet);
        assert_eq!(
            row.source,
            MetricSource::Service {
                source: "finance".into(),
                scope: "finance".into(),
                service: "fava".into(),
                endpoint: "127.0.0.1:8793".into(),
                path: "/metrics".into(),
                url: "http://127.0.0.1:8793/metrics".into(),
            }
        );

        let old = [value(metric, Some(0.0), now - chrono::Duration::days(3))];
        let rows = super::rows(&old, &readers, &catalogue, &tree(), Path::new("/i"), now).unwrap();
        assert_eq!(rows[0].freshness, Freshness::Stale);
        let status = |id: &str| rows[0].consumers.iter().find(|c| c.id == id).unwrap().status;
        assert_eq!(status("gobd/belegprinzip"), ConsumerStatus::Stale);
        assert_eq!(status("gobd/lenient"), ConsumerStatus::Met);
        assert_eq!(status("books/clean"), ConsumerStatus::Met);

        let none = [value(metric, None, now)];
        let rows = super::rows(&none, &readers, &catalogue, &tree(), Path::new("/i"), now).unwrap();
        assert_eq!(rows[0].freshness, Freshness::NoData);
        assert!(rows[0].consumers.iter().all(|c| c.status == ConsumerStatus::NoData));
    }

    /// The Policy tab reads a `reported.*` value as the control's own scope
    /// sees it; a control applying at `ops` cannot be met by a source
    /// `finance` declares, whatever the selection-wide value is.
    #[test]
    fn a_policy_consumer_outside_the_declaring_scope_reads_none_like_the_policy_tab() {
        let now = Utc::now();
        let metric = "reported.finance.bookings_without_beleg";
        let readers = policy_readers(&policy(
            "ops",
            vec![subject("house/beleg", vec![metric_check(metric, None, Some(5.0))], None)],
        ));
        let values = [value(metric, Some(0.0), now)];
        let rows = rows(&values, &readers, &catalogue(fava()), &tree(), Path::new("/i"), now).unwrap();
        let consumer = &rows[0].consumers[0];
        assert_eq!(consumer.status, ConsumerStatus::NoData);
        assert!(consumer.reasons[0].contains("outside the selected subtree"), "{:?}", consumer.reasons);
    }

    #[test]
    fn a_metric_with_no_windowed_consumer_has_no_window_and_a_built_in_source() {
        let now = Utc::now();
        let readers = goal_readers(
            &[GoalKeyResult {
                id: "o/kr".into(),
                metric: id("fail_rate"),
                baseline: 0.5,
                target: 0.1,
            }],
            None,
        );
        let rows = rows(
            &[value("fail_rate", Some(0.05), now)],
            &readers,
            &reported::Catalogue::default(),
            &tree(),
            Path::new("/i"),
            now,
        )
        .unwrap();
        assert_eq!(rows[0].freshness, Freshness::NoWindow);
        assert_eq!(rows[0].source, MetricSource::BuiltIn);
        assert_eq!(rows[0].consumers[0].status, ConsumerStatus::Met);
    }

    #[test]
    fn source_of_names_a_file_relative_to_the_root_and_explains_an_unresolved_one() {
        let root = PathBuf::from("/instance");
        let file = catalogue(reported::Location::File(root.join("data/finance/metrics.json")));
        let metric = id("reported.finance.bookings_without_beleg");
        assert_eq!(
            source_of(&metric, &file, &root),
            MetricSource::File {
                source: "finance".into(),
                scope: "finance".into(),
                path: "data/finance/metrics.json".into(),
            }
        );
        let refused = catalogue(reported::Location::Refused {
            service: Some("fava".into()),
            reason: "no loopback endpoint".into(),
        });
        assert!(matches!(
            source_of(&metric, &refused, &root),
            MetricSource::Unresolved { service: Some(s), reason, .. } if s == "fava" && reason.contains("loopback")
        ));
        assert!(matches!(
            source_of(&id("reported.nobody.x"), &file, &root),
            MetricSource::Unresolved { scope: None, reason, .. } if reason.contains("no scope declares")
        ));
    }
}
