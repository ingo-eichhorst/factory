//! Recorded change outcomes, shared by the rolling metrics, weekly trend and
//! per-release cohort. No Git, runtime, clock or history reads live here.
use super::*;
use std::collections::BTreeMap;

#[cfg(test)]
pub fn change_outcome(
    deployment: &Deployment,
    history: &[Deployment],
    incidents: &[Incident],
    observed_at: DateTime<Utc>,
) -> (bool, bool) {
    match deployment.status {
        DeployStatus::Failed | DeployStatus::RolledBack => return (true, false),
        DeployStatus::Running => return (false, false),
        DeployStatus::Succeeded => {}
    }
    let Some(from) = deployment.finished_at else {
        return (false, false);
    };
    let next = history
        .iter()
        .filter(|next| {
            next.environment == deployment.environment
                && next.scope == deployment.scope
                && next.started_at > deployment.started_at
                && next.started_at <= observed_at
        })
        .map(|next| next.started_at)
        .min();
    let horizon = from + Duration::hours(CHANGE_FAILURE_HORIZON_HOURS);
    let until = next.map_or(horizon, |next| next.min(horizon));
    let failed = incidents.iter().any(|incident| {
        incident.environment == deployment.environment
            && incident.started_at >= from
            && incident.started_at < until
            && incident.started_at <= observed_at
    });
    (failed, !failed && until > observed_at)
}

fn recorded_outcomes(
    history: &[Deployment],
    incidents: &[Incident],
    observed_at: DateTime<Utc>,
) -> BTreeMap<String, (bool, bool)> {
    let mut starts: BTreeMap<&str, Vec<DateTime<Utc>>> = BTreeMap::new();
    for incident in incidents.iter().filter(|incident| incident.started_at <= observed_at) {
        starts.entry(&incident.environment).or_default().push(incident.started_at);
    }
    for times in starts.values_mut() {
        times.sort_unstable();
    }
    let mut ordered: Vec<_> = history.iter().filter(|deployment| deployment.started_at <= observed_at).collect();
    ordered.sort_by_key(|deployment| deployment.started_at);
    let mut next_starts = BTreeMap::new();
    let mut outcomes = BTreeMap::new();
    for deployment in ordered.into_iter().rev() {
        let key = (&deployment.scope, &deployment.environment);
        let next = next_starts.insert(key, deployment.started_at);
        let outcome = match (deployment.status, deployment.finished_at) {
            (DeployStatus::Failed | DeployStatus::RolledBack, _) => (true, false),
            (DeployStatus::Succeeded, Some(from)) => {
                let horizon = from + Duration::hours(CHANGE_FAILURE_HORIZON_HOURS);
                let until = next.map_or(horizon, |next| next.min(horizon));
                let failed = starts.get(deployment.environment.as_str()).is_some_and(|times| {
                    let first = times.partition_point(|at| *at < from);
                    times.get(first).is_some_and(|at| *at < until)
                });
                (failed, !failed && until > observed_at)
            }
            _ => (false, false),
        };
        outcomes.insert(deployment.id.clone(), outcome);
    }
    outcomes
}

pub fn dora_between(
    history: &[Deployment],
    incidents: &[Incident],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    observed_at: DateTime<Utc>,
) -> Dora {
    let finished: Vec<_> = history
        .iter()
        .filter(|deployment| {
            deployment.finished() && deployment.finished_at.is_some_and(|at| at >= from && at < to && at <= observed_at)
        })
        .collect();
    let succeeded: Vec<_> = finished.iter().filter(|deployment| deployment.status == DeployStatus::Succeeded).collect();
    let days = (to - from).num_seconds().max(1) as f64 / 86_400.0;
    let deploy_frequency = (!succeeded.is_empty()).then(|| succeeded.len() as f64 / (days / 7.0));
    let mut lead: Vec<f64> = succeeded
        .iter()
        .filter_map(|deployment| {
            Some((deployment.finished_at? - deployment.release.committed_at?).num_seconds().max(0) as f64)
        })
        .collect();
    let recorded = recorded_outcomes(history, incidents, observed_at);
    let outcomes: Vec<_> =
        finished.iter().map(|deployment| recorded.get(&deployment.id).copied().unwrap_or_default()).collect();
    let failed = outcomes.iter().filter(|(failed, _)| *failed).count();
    let in_window: Vec<_> = incidents
        .iter()
        .filter(|incident| {
            incident.started_at < to
                && incident.started_at <= observed_at
                && incident.ended_at.is_none_or(|end| end >= from)
        })
        .collect();
    let mut restores: Vec<_> = in_window
        .iter()
        .filter(|incident| incident.ended_at.is_some_and(|end| end >= from && end < to && end <= observed_at))
        .map(|incident| incident.duration_seconds(observed_at) as f64)
        .collect();
    Dora {
        window_days: days.ceil() as i64,
        deploy_frequency,
        lead_time_p50: percentile(&mut lead, 50.0),
        change_failure_rate: (!finished.is_empty()).then(|| failed as f64 / finished.len() as f64),
        mttr: (!restores.is_empty()).then(|| restores.iter().sum::<f64>() / restores.len() as f64),
        time_to_restore_p50: percentile(&mut restores, 50.0),
        incidents: in_window.len() as u32,
        observing_changes: outcomes.iter().filter(|(_, observing)| *observing).count() as u32,
    }
}

/// Nonoverlapping, half-open periods. Subsequent failures within the one-hour
/// observation horizon still belong to the deployment's original cohort.
pub fn weekly(history: &[Deployment], incidents: &[Incident], days: i64, now: DateTime<Utc>) -> Vec<DoraPeriod> {
    let mut from = now - Duration::days(days.clamp(1, SAMPLE_RETENTION_DAYS));
    let mut periods = Vec::new();
    while from < now {
        let to = (from + Duration::days(7)).min(now);
        periods.push(DoraPeriod { from, to, dora: dora_between(history, incidents, from, to, now) });
        from = to;
    }
    periods
}

pub fn release_cohorts(
    history: &[Deployment],
    incidents: &[Incident],
    now: DateTime<Utc>,
) -> BTreeMap<(String, String), ReleaseEffectiveness> {
    let since = now - Duration::days(DEFAULT_WINDOW_DAYS);
    let outcomes = recorded_outcomes(history, incidents, now);
    let mut releases: BTreeMap<_, ReleaseEffectiveness> = BTreeMap::new();
    for deployment in history
        .iter()
        .filter(|deployment| deployment.finished() && deployment.finished_at.is_some_and(|at| at >= since && at <= now))
    {
        let evidence = releases
            .entry((deployment.scope.clone(), deployment.release.commit.clone()))
            .or_insert_with(|| ReleaseEffectiveness { window_days: DEFAULT_WINDOW_DAYS, ..Default::default() });
        let (failed, observing) = outcomes.get(&deployment.id).copied().unwrap_or_default();
        evidence.finished += 1;
        evidence.failed_changes += u32::from(failed);
        evidence.observing += u32::from(observing);
    }
    for evidence in releases.values_mut() {
        evidence.change_failure_rate = Some(evidence.failed_changes as f64 / evidence.finished as f64);
    }
    releases
}

#[cfg(test)]
mod tests {
    use super::super::tests::{deploy, t};
    use super::*;

    fn incident(at: i64, end: Option<i64>) -> Incident {
        Incident { environment: "prod".into(), started_at: t(at), ended_at: end.map(t), checks: vec!["api".into()] }
    }

    #[test]
    fn a_running_next_deployment_and_other_environments_cannot_be_blamed_on_the_old_release() {
        let history = vec![deploy(0, DeployStatus::Succeeded, None), deploy(10, DeployStatus::Running, None)];
        assert_eq!(change_outcome(&history[0], &history, &[incident(12, None)], t(100)), (false, false));
        let mut other = incident(3, None);
        other.environment = "staging".into();
        assert_eq!(change_outcome(&history[0], &history, &[other], t(100)), (false, false));
        assert_eq!(change_outcome(&history[0], &history, &[incident(9, Some(10))], t(100)), (true, false));
    }

    #[test]
    fn weekly_boundaries_do_not_duplicate_deployments_or_leak_future_observations() {
        let old = deploy(0, DeployStatus::Succeeded, Some(-10));
        let boundary = deploy(7 * 24 * 60 - 2, DeployStatus::Succeeded, Some(0));
        let history = vec![old, boundary];
        let periods = weekly(&history, &[incident(7 * 24 * 60 + 1, Some(7 * 24 * 60 + 2))], 14, t(14 * 24 * 60));
        assert_eq!(periods.len(), 2);
        assert_eq!(periods[0].to, periods[1].from);
        assert_eq!(periods[0].dora.deploy_frequency, Some(1.0));
        assert_eq!(periods[1].dora.deploy_frequency, Some(1.0));
        assert_eq!(periods[0].dora.change_failure_rate, Some(0.0));
        assert_eq!(periods[1].dora.change_failure_rate, Some(1.0));
        let pending = dora_between(&history, &[incident(5, Some(6))], t(0), t(4), t(4));
        assert_eq!(pending.change_failure_rate, Some(0.0));
        assert_eq!(pending.observing_changes, 1);
        assert_eq!(pending.time_to_restore_p50, None);
    }

    #[test]
    fn an_incident_across_a_period_edge_still_belongs_to_the_deployment_before_the_edge() {
        let history = vec![deploy(7 * 24 * 60 - 5, DeployStatus::Succeeded, None)];
        let periods = weekly(&history, &[incident(7 * 24 * 60 + 1, Some(7 * 24 * 60 + 2))], 14, t(14 * 24 * 60));
        assert_eq!(periods[0].dora.change_failure_rate, Some(1.0));
        assert_eq!(periods[1].dora.change_failure_rate, None, "no deployments is not a zero failure rate");
        assert_eq!(periods[1].dora.time_to_restore_p50, Some(60.0));
        assert_eq!(weekly(&[], &[], 90, t(0)).len(), 13);
    }

    #[test]
    fn per_release_cohorts_include_incidents_but_not_running_old_or_other_scope_deployments() {
        let mut first = deploy(0, DeployStatus::Succeeded, None);
        first.release.commit = "same-release".into();
        let mut other = deploy(20, DeployStatus::Failed, None);
        other.scope = "other".into();
        other.release.commit = first.release.commit.clone();
        let mut old = deploy(-30 * 24 * 60, DeployStatus::Failed, None);
        old.release.commit = first.release.commit.clone();
        let running = deploy(30, DeployStatus::Running, None);
        let cohorts = release_cohorts(&[first.clone(), other, old, running], &[incident(3, Some(4))], t(100));
        let evidence = &cohorts[&(first.scope.clone(), first.release.commit.clone())];
        assert_eq!(evidence.finished, 1);
        assert_eq!(evidence.failed_changes, 1, "a passed deployment followed by an incident is a change failure");
        assert_eq!(evidence.change_failure_rate, Some(1.0));
        assert_eq!(evidence.observing, 0);
        assert!(release_cohorts(&[], &[], t(100)).is_empty());
    }
}
