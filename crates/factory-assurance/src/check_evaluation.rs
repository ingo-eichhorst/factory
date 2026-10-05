//! The live L5 check-result provider. Only authored subjects, receipts and
//! limits enter; lower gathering and evaluation happen here on every read.
use crate::{checks, evidence, metric_values::copy_input_error};
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
pub struct Provider<'a, P> {
    evidence: evidence::Service<'a, P>,
}
impl<'a, P: evidence::Ports> Provider<'a, P> {
    pub fn new(evidence: evidence::Service<'a, P>) -> Self {
        Self { evidence }
    }
}
impl<P: Send + Sync> FactProvider for Provider<'_, P> {
    type Level = L5;
}
#[async_trait::async_trait]
impl<P: evidence::Ports + Send + Sync> Provide<CheckEvaluationFact> for Provider<'_, P> {
    type Query = Read;
    type Value = CheckEvaluationFact;
    type Error = FactoryError;
    async fn get(&self, read: &Read) -> Result<CheckEvaluationFact> {
        let targets: Vec<_> = read
            .scopes
            .iter()
            .map(|input| (input.scope.name.as_str(), input.subjects.as_slice()))
            .collect();
        let shared = self.evidence.shared(&targets).await?;
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
                .evidence
                .for_scope(
                    &input.scope,
                    &input.subjects,
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
