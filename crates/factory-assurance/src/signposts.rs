//! L5 owns signpost evaluation and the physical live fact producer. Raw
//! authored thresholds/subjects arrive downward; metrics are computed by
//! the same-level live service, not passed in or obtained through a callback.
use crate::{
    metrics::{self, MetricError, MetricId, MetricValue},
    metrics_service::{self, Plan, PolicyInputs, QualityBudgets},
};
use chrono::{DateTime, NaiveDate, Utc};
use factory_kernel::{FactProvider, Provide, Result, SignpostFact, SignpostObservation, L5};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};
/// A threshold on a registry metric, watched on every read -- "is reality
/// moving towards this scenario". Never itself starts, stops, or gates
/// anything (design §8, same as everywhere else in this module); see
/// [`evaluate_signposts`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signpost {
    pub metric: MetricId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<f64>,
    /// Inactive before this date -- `None` means active immediately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<NaiveDate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignpostState {
    /// Active and within bounds.
    Quiet,
    /// Active and past a threshold.
    Triggered,
    /// Before its own `from` date.
    NotYetActive,
    /// Active, but there is no metric value to check it against.
    NoData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignpostStatus {
    pub metric: MetricId,
    pub state: SignpostState,
    pub reason: String,
}

/// Evaluate every signpost against `values` (already-computed metric
/// values, L5's live metric service gathers, exactly like a scenario projection
/// takes them) as of `now`. Preserves `signposts`' own order -- like an
/// objective's key results in `goals.rs`, that order is itself information
/// (an author's own priority), not something to sort away.
///
/// `below`/`above` are both checked when both are set -- `Triggered` if
/// either is breached -- rather than one refusing the other, since a
/// two-sided band (e.g. "watch if this share leaves 0.4..0.8") is a
/// legitimate signpost, not an authoring mistake; the authored loader never requires
/// exactly one, only that at least one is set
/// (a missing-threshold finding). Both comparisons are
/// strict (`<`/`>`): a value sitting exactly on a threshold reads `Quiet`,
/// the same "not yet past it" reading `within_max_age`'s own `<=` gives a
/// control right at its freshness window in `policy.rs`.
pub fn evaluate_signposts(
    signposts: &[Signpost],
    values: &BTreeMap<MetricId, MetricValue>,
    now: DateTime<Utc>,
) -> Vec<SignpostStatus> {
    signposts
        .iter()
        .map(|sp| evaluate_signpost(sp, values, now))
        .collect()
}

fn evaluate_signpost(
    sp: &Signpost,
    values: &BTreeMap<MetricId, MetricValue>,
    now: DateTime<Utc>,
) -> SignpostStatus {
    if let Some(from) = sp.from {
        let start = from.and_hms_opt(0, 0, 0).unwrap().and_utc();
        if now < start {
            return SignpostStatus {
                metric: sp.metric.clone(),
                state: SignpostState::NotYetActive,
                reason: format!("active from {from}"),
            };
        }
    }

    let Some(mv) = values.get(&sp.metric) else {
        return SignpostStatus {
            metric: sp.metric.clone(),
            state: SignpostState::NoData,
            reason: "no metric value supplied".to_string(),
        };
    };
    let Some(value) = mv.value else {
        let reason = mv
            .reason
            .clone()
            .unwrap_or_else(|| "no reason given".to_string());
        return SignpostStatus {
            metric: sp.metric.clone(),
            state: SignpostState::NoData,
            reason,
        };
    };

    let below_hit = sp.below.is_some_and(|t| value < t);
    let above_hit = sp.above.is_some_and(|t| value > t);
    if below_hit || above_hit {
        let mut parts = Vec::new();
        if below_hit {
            parts.push(format!("{value} is below {}", sp.below.unwrap()));
        }
        if above_hit {
            parts.push(format!("{value} is above {}", sp.above.unwrap()));
        }
        SignpostStatus {
            metric: sp.metric.clone(),
            state: SignpostState::Triggered,
            reason: parts.join("; "),
        }
    } else {
        SignpostStatus {
            metric: sp.metric.clone(),
            state: SignpostState::Quiet,
            reason: format!("{value} within bounds"),
        }
    }
}

pub struct ScenarioInput {
    pub name: String,
    pub signposts: Vec<Signpost>,
}
/// Raw file metadata only; no upper-level verdict or computed metric.
pub type Revision = Vec<(OsString, Option<SystemTime>, u64)>;
pub struct Read {
    pub now: DateTime<Utc>,
    pub plan: Plan,
    pub scenarios: Vec<ScenarioInput>,
    pub policy: Option<PolicyInputs>,
    pub budgets: QualityBudgets,
    pub revision: Revision,
    pub use_cache: bool,
}
pub fn metric_ids(scenarios: &[ScenarioInput]) -> Vec<MetricId> {
    let mut ids = Vec::new();
    for scenario in scenarios {
        for signpost in &scenario.signposts {
            if !ids.contains(&signpost.metric)
                && !matches!(
                    metrics::resolve(&signpost.metric),
                    Err(MetricError::Unknown(_))
                )
            {
                ids.push(signpost.metric.clone());
            }
        }
    }
    ids
}
const TTL: Duration = Duration::from_secs(60);
struct Cached {
    started: Instant,
    stored: Instant,
    revision: Revision,
    fact: SignpostFact,
}
#[derive(Default)]
pub struct Cache(Mutex<Option<Cached>>);
impl Cache {
    pub fn is_populated(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }
    fn lookup(&self, revision: &Revision) -> Option<SignpostFact> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|cached| &cached.revision == revision && cached.stored.elapsed() < TTL)
            .map(|cached| cached.fact.clone())
    }
}
pub struct Provider<'a, P> {
    metrics: metrics_service::Service<'a, P>,
    cache: &'a Cache,
}
impl<'a, P: metrics_service::Ports> Provider<'a, P> {
    pub fn new(metrics: metrics_service::Service<'a, P>, cache: &'a Cache) -> Self {
        Self { metrics, cache }
    }
}
impl<P: Send + Sync> FactProvider for Provider<'_, P> {
    type Level = L5;
}
#[async_trait::async_trait]
impl<P: metrics_service::Ports + Send + Sync> Provide<SignpostFact> for Provider<'_, P> {
    type Query = Read;
    type Value = SignpostFact;
    type Error = factory_kernel::FactoryError;
    async fn get(&self, read: &Read) -> Result<SignpostFact> {
        let started = Instant::now();
        if read.use_cache {
            if let Some(fact) = self.cache.lookup(&read.revision) {
                return Ok(fact);
            }
        }
        let gathered = self
            .metrics
            .gather(&read.plan, read.policy.as_ref(), read.now, None)
            .await?;
        let measured = self
            .metrics
            .finish(&read.plan, gathered, &read.budgets, read.now, None)
            .await?;
        let values: BTreeMap<_, _> = measured
            .values
            .into_iter()
            .map(|v| (v.id.clone(), v))
            .collect();
        let triggered = read
            .scenarios
            .iter()
            .flat_map(|scenario| {
                evaluate_signposts(&scenario.signposts, &values, read.now)
                    .into_iter()
                    .filter(|status| status.state == SignpostState::Triggered)
                    .map(|status| SignpostObservation {
                        scenario: scenario.name.clone(),
                        metric: status.metric.to_string(),
                        reason: status.reason,
                    })
            })
            .collect();
        let fact = SignpostFact {
            at: read.now,
            triggered,
        };
        if read.use_cache {
            let mut cache = self.cache.0.lock().unwrap_or_else(|e| e.into_inner());
            if cache
                .as_ref()
                .is_none_or(|previous| previous.started <= started)
            {
                *cache = Some(Cached {
                    started,
                    stored: Instant::now(),
                    revision: read.revision.clone(),
                    fact: fact.clone(),
                });
            }
        }
        Ok(fact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn now() -> DateTime<Utc> {
        "2026-10-16T00:00:00Z".parse().unwrap()
    }
    fn threshold() -> Signpost {
        Signpost {
            metric: MetricId::new("fail_rate").unwrap(),
            below: Some(2.0),
            above: Some(4.0),
            from: None,
        }
    }
    fn values(value: Option<f64>, reason: Option<&str>) -> BTreeMap<MetricId, MetricValue> {
        let id = threshold().metric;
        BTreeMap::from([(
            id.clone(),
            MetricValue {
                id,
                value,
                as_of: now(),
                reason: reason.map(str::to_owned),
            },
        )])
    }
    #[test]
    fn thresholds_are_strict_two_sided_and_keep_exact_reasons() {
        for (value, state, reason) in [
            (1.0, SignpostState::Triggered, "1 is below 2"),
            (2.0, SignpostState::Quiet, "2 within bounds"),
            (4.0, SignpostState::Quiet, "4 within bounds"),
            (5.0, SignpostState::Triggered, "5 is above 4"),
        ] {
            let result = evaluate_signposts(&[threshold()], &values(Some(value), None), now());
            assert_eq!(result[0].state, state);
            assert_eq!(result[0].reason, reason);
        }
        let mut both = threshold();
        both.below = Some(4.0);
        both.above = Some(2.0);
        assert_eq!(
            evaluate_signposts(&[both], &values(Some(3.0), None), now())[0].reason,
            "3 is below 4; 3 is above 2"
        );
    }
    #[test]
    fn activation_uses_utc_midnight_and_unknown_is_never_a_trigger() {
        let mut sp = threshold();
        sp.from = Some(now().date_naive());
        let previous = evaluate_signposts(
            &[sp.clone()],
            &values(Some(1.0), None),
            now() - chrono::Duration::nanoseconds(1),
        );
        assert_eq!(previous[0].state, SignpostState::NotYetActive);
        assert_eq!(previous[0].reason, "active from 2026-10-16");
        assert_eq!(
            evaluate_signposts(&[sp.clone()], &values(Some(1.0), None), now())[0].state,
            SignpostState::Triggered
        );
        for (v, reason) in [
            (BTreeMap::new(), "no metric value supplied"),
            (values(None, None), "no reason given"),
            (values(None, Some("no finished runs")), "no finished runs"),
        ] {
            let result = evaluate_signposts(&[sp.clone()], &v, now());
            assert_eq!(result[0].state, SignpostState::NoData);
            assert_eq!(result[0].reason, reason);
        }
    }
    #[test]
    fn registry_projection_deduplicates_in_authored_order_and_ignores_unknown_ids() {
        let mut unknown = threshold();
        unknown.metric = MetricId::new("not_registered").unwrap();
        let mut backup = threshold();
        backup.metric = MetricId::new("backup_age_hours").unwrap();
        let input = vec![
            ScenarioInput {
                name: "first".into(),
                signposts: vec![unknown, threshold(), backup.clone()],
            },
            ScenarioInput {
                name: "second".into(),
                signposts: vec![backup, threshold()],
            },
        ];
        assert_eq!(
            metric_ids(&input)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["fail_rate", "backup_age_hours"]
        );
    }
    #[test]
    fn provider_cache_expires_without_freshening_and_requires_matching_revision() {
        let revision = vec![("s.yaml".into(), None, 3)];
        let fact = SignpostFact {
            at: now(),
            triggered: Vec::new(),
        };
        let cache = Cache(Mutex::new(Some(Cached {
            started: Instant::now(),
            stored: Instant::now(),
            revision: revision.clone(),
            fact: fact.clone(),
        })));
        assert_eq!(cache.lookup(&revision), Some(fact));
        assert!(cache.lookup(&vec![("s.yaml".into(), None, 4)]).is_none());
        cache.0.lock().unwrap().as_mut().unwrap().stored =
            Instant::now() - TTL - Duration::from_secs(1);
        assert!(cache.lookup(&revision).is_none());
        assert_eq!(cache.0.lock().unwrap().as_ref().unwrap().fact.at, now());
    }
}
