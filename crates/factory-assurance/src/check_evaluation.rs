//! The live L5 check-result provider. Only authored subjects, receipts and
//! limits enter; lower gathering and evaluation happen here on every read.
use crate::{
    checks, evidence,
    metric_values::copy_input_error,
    metrics_service::{self, CheckMetricInputs},
};
use chrono::{DateTime, Utc};
use factory_kernel::{
    Attestation, CheckEvaluationFact, CheckObservation, FactProvider, FactoryError, Provide,
    Result, ScopeCheckEvaluation, ScopeNode, L5,
};
use std::collections::BTreeSet;

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
/// Holds the metrics service rather than the bare evidence service it
/// wraps: a `check: metric` (`#278`) reads its value through
/// `metrics_service::Service::check_metrics`, with `metric_inputs` -- the
/// instance's raw `scope.metrics` declarations -- handed in at
/// construction, the same way every other authored input reaches L5.
pub struct Provider<'a, P> {
    metrics: metrics_service::Service<'a, P>,
    metric_inputs: CheckMetricInputs,
}

/// Gather only the primary declarations, then evaluate both authored sets
/// against that same evidence. Scenario promotion historically compares its
/// baseline against overlay-selected evidence, not an independent gather.
pub struct ComparisonRead {
    pub primary: Read,
    pub alternative: Vec<ScopeInput>,
}
impl<'a, P: metrics_service::Ports> Provider<'a, P> {
    pub fn new(metrics: metrics_service::Service<'a, P>, metric_inputs: CheckMetricInputs) -> Self {
        Self {
            metrics,
            metric_inputs,
        }
    }

    /// One scope's evidence: everything `for_scope` gathers, plus the
    /// metric values its `check: metric`s name, if any do.
    #[allow(clippy::too_many_arguments)]
    async fn scope_evidence(
        &self,
        input: &ScopeInput,
        tags: &BTreeSet<String>,
        attestations: &[Attestation],
        shared: &evidence::Shared,
        budget: Option<&evidence::BudgetIntent>,
        now: DateTime<Utc>,
    ) -> Result<checks::Evidence> {
        let mut evidence = self
            .metrics
            .evidence()
            .for_scope(
                &input.scope,
                &input.subjects,
                tags,
                attestations,
                shared,
                budget,
                now,
            )
            .await?;
        evidence.metrics = self
            .metrics
            .check_metrics(&self.metric_inputs, &input.scope, &input.subjects, now)
            .await?;
        Ok(evidence)
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
        let shared = self.metrics.evidence().shared(&targets).await?;
        let budgets = read.budgets.as_ref().map_err(copy_input_error)?;
        if budgets.len() != read.scopes.len() {
            return Err(FactoryError::BadRequest(
                "check budget inputs do not match their scope selections".into(),
            ));
        }
        let now = read.now.unwrap_or_else(Utc::now);
        let mut scopes = Vec::new();
        for (input, budget) in read.scopes.iter().zip(budgets) {
            let evidence = self
                .scope_evidence(
                    input,
                    &read.tags,
                    &read.attestations,
                    &shared,
                    budget.as_ref(),
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
        let shared = self.metrics.evidence().shared(&targets).await?;
        let budgets = primary.budgets.as_ref().map_err(copy_input_error)?;
        if budgets.len() != primary.scopes.len() || read.alternative.len() != primary.scopes.len() {
            return Err(FactoryError::BadRequest(
                "comparison inputs do not match their scope selections".into(),
            ));
        }
        let now = primary.now.unwrap_or_else(Utc::now);
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
                .scope_evidence(
                    input,
                    &primary.tags,
                    &primary.attestations,
                    &shared,
                    budget.as_ref(),
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
