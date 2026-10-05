//! The live L5 check-result provider. Only authored subjects, receipts and
//! limits enter; lower gathering and evaluation happen here on every read.
use crate::{checks, evidence, metric_values::copy_input_error, metrics_service, reported};
use chrono::{DateTime, Utc};
use factory_kernel::{
    Attestation, CheckEvaluationFact, CheckObservation, FactProvider, FactoryError, Provide,
    Result, ScopeCheckEvaluation, ScopeNode, L5,
};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub struct ScopeInput {
    pub scope: ScopeNode,
    pub subjects: Vec<checks::EvaluationSubject>,
}
pub struct Read {
    pub scopes: Vec<ScopeInput>,
    pub tags: BTreeSet<String>,
    pub attestations: Vec<Attestation>,
    /// Raw authored inputs, deferred so the shared lower gather retains
    /// its historical failure precedence over a failed budget read.
    pub budgets: Result<Vec<Option<evidence::BudgetIntent>>>,
    /// Reports capture time before gathering; detail historically captures
    /// it after shared gathering. None preserves that latter sequence.
    pub now: Option<DateTime<Utc>>,
}
pub struct Provider<'a, P> {
    // `#278`: the one lower-evidence seam, shared with its own
    // `metric_values` for a `Check::Metric` -- see the module doc comment
    // on `metrics_service::Service::metric_values`.
    metrics: metrics_service::Service<'a, P>,
    /// `#278`: the instance root, handed to `Plan::prepare` fresh on every
    /// read, same as every other metrics entry point.
    root: PathBuf,
    /// `#278`: the live `scope.metrics` declarations, projected down by
    /// the outside router the same way `quality_configuration`/
    /// `reported_configuration` already are for every other metrics entry
    /// point -- never a computed value.
    reported: reported::Configuration,
}

/// Gather only the primary declarations, then evaluate both authored sets
/// against that same evidence. Scenario promotion historically compares its
/// baseline against overlay-selected evidence, not an independent gather.
pub struct ComparisonRead {
    pub primary: Read,
    pub alternative: Vec<ScopeInput>,
}
impl<'a, P: metrics_service::Ports> Provider<'a, P> {
    pub fn new(
        metrics: metrics_service::Service<'a, P>,
        root: PathBuf,
        reported: reported::Configuration,
    ) -> Self {
        Self {
            metrics,
            root,
            reported,
        }
    }

    /// `#278`: every metric id a `Check::Metric` among `per_scope` asks
    /// for, computed once and shared across every scope and control in
    /// this read -- see `metrics_service::Service::metric_values`.
    async fn metric_values<S: checks::CheckSource>(
        &self,
        per_scope: &[(&str, &[S])],
        now: DateTime<Utc>,
    ) -> Result<std::collections::BTreeMap<crate::metrics::MetricId, crate::metrics::MetricValue>>
    {
        self.metrics
            .metric_values(per_scope, self.root.clone(), &self.reported, now)
            .await
    }
}
impl<P: Send + Sync> FactProvider for Provider<'_, P> {
    type Level = L5;
}
#[async_trait::async_trait]
impl<P: metrics_service::Ports + Send + Sync> Provide<CheckEvaluationFact> for Provider<'_, P> {
    type Query = Read;
    type Value = CheckEvaluationFact;
    type Error = FactoryError;
    async fn get(&self, read: &Read) -> Result<CheckEvaluationFact> {
        let targets: Vec<_> = read
            .scopes
            .iter()
            .map(|input| (input.scope.name.as_str(), input.subjects.as_slice()))
            .collect();
        let shared = self.metrics.evidence.shared(&targets).await?;
        let budgets = read.budgets.as_ref().map_err(copy_input_error)?;
        if budgets.len() != read.scopes.len() {
            return Err(FactoryError::BadRequest(
                "check budget inputs do not match their scope selections".into(),
            ));
        }
        let now = read.now.unwrap_or_else(Utc::now);
        let metric_values = self.metric_values(&targets, now).await?;
        let mut scopes = Vec::new();
        for (input, budget) in read.scopes.iter().zip(budgets) {
            let evidence = self
                .metrics
                .evidence
                .for_scope(
                    &input.scope,
                    &input.subjects,
                    &read.tags,
                    &read.attestations,
                    &shared,
                    budget.as_ref(),
                    &metric_values,
                    now,
                )
                .await?;
            let findings = checks::evidence_findings(&evidence, &input.scope.name);
            let statuses = checks::evaluate(&input.subjects, &evidence, now)
                .into_iter()
                .map(|status| CheckObservation {
                    control: status.control,
                    title: status.title,
                    refs: status.refs,
                    status: status.status,
                })
                .collect();
            scopes.push(ScopeCheckEvaluation {
                scope: input.scope.name.clone(),
                statuses,
                findings,
            });
        }
        Ok(CheckEvaluationFact { at: now, scopes })
    }
}

#[async_trait::async_trait]
impl<P: metrics_service::Ports + Send + Sync> Provide<factory_kernel::CheckComparisonFact>
    for Provider<'_, P>
{
    type Query = ComparisonRead;
    type Value = factory_kernel::CheckComparisonFact;
    type Error = FactoryError;
    async fn get(&self, read: &ComparisonRead) -> Result<Self::Value> {
        let primary = &read.primary;
        let targets: Vec<_> = primary
            .scopes
            .iter()
            .map(|input| (input.scope.name.as_str(), input.subjects.as_slice()))
            .collect();
        let shared = self.metrics.evidence.shared(&targets).await?;
        let budgets = primary.budgets.as_ref().map_err(copy_input_error)?;
        if budgets.len() != primary.scopes.len() || read.alternative.len() != primary.scopes.len() {
            return Err(FactoryError::BadRequest(
                "comparison inputs do not match their scope selections".into(),
            ));
        }
        let now = primary.now.unwrap_or_else(Utc::now);
        // `#278`: every `Check::Metric` either side of the comparison
        // names, so an overlay that only adds one still reads real values.
        let alternative_targets: Vec<_> = read
            .alternative
            .iter()
            .map(|input| (input.scope.name.as_str(), input.subjects.as_slice()))
            .collect();
        let mut metric_targets = targets.clone();
        metric_targets.extend(alternative_targets);
        let metric_values = self.metric_values(&metric_targets, now).await?;
        let mut scopes = Vec::new();
        for ((input, alternative), budget) in
            primary.scopes.iter().zip(&read.alternative).zip(budgets)
        {
            if input.scope != alternative.scope {
                return Err(FactoryError::BadRequest(
                    "comparison scope does not match its primary selection".into(),
                ));
            }
            let evidence = self
                .metrics
                .evidence
                .for_scope(
                    &input.scope,
                    &input.subjects,
                    &primary.tags,
                    &primary.attestations,
                    &shared,
                    budget.as_ref(),
                    &metric_values,
                    now,
                )
                .await?;
            let observations = |subjects: &[checks::EvaluationSubject]| {
                checks::evaluate(subjects, &evidence, now)
                    .into_iter()
                    .map(|status| CheckObservation {
                        control: status.control,
                        title: status.title,
                        refs: status.refs,
                        status: status.status,
                    })
                    .collect()
            };
            scopes.push(factory_kernel::ScopeCheckComparison {
                scope: input.scope.name.clone(),
                primary: observations(&input.subjects),
                alternative: observations(&alternative.subjects),
            });
        }
        Ok(factory_kernel::CheckComparisonFact { at: now, scopes })
    }
}
