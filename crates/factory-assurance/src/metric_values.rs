//! The physical live L5 metric-value port. Only raw authored plan inputs
//! enter; gathering and numerical projection use the same-level service.
use crate::metrics::MetricsWindow;
use crate::metrics_service::{self, Plan, PolicyInputs, QualityBudgets};
use chrono::{DateTime, Utc};
use factory_kernel::{FactProvider, FactoryError, MetricValuesFact, Provide, Result, L5};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Read {
    pub plan: Plan,
    pub policy: Result<Option<PolicyInputs>>,
    pub budgets: QualityBudgets,
    pub now: DateTime<Utc>,
    pub window: Option<MetricsWindow>,
}

pub struct Provider<'a, P> {
    metrics: metrics_service::Service<'a, P>,
    policy_gathered: AtomicBool,
}
impl<'a, P: metrics_service::Ports> Provider<'a, P> {
    pub fn new(metrics: metrics_service::Service<'a, P>) -> Self {
        Self {
            metrics,
            policy_gathered: AtomicBool::new(false),
        }
    }
    /// Outside-only request failure sequencing, not metric evidence or a
    /// status cache. The router constructs a separate provider per request.
    /// Readers know only Provide; no level consults this diagnostic.
    pub fn policy_was_gathered(&self) -> bool {
        self.policy_gathered.load(Ordering::Acquire)
    }
}
impl<P: Send + Sync> FactProvider for Provider<'_, P> {
    type Level = L5;
}
#[async_trait::async_trait]
impl<P: metrics_service::Ports + Send + Sync> Provide<MetricValuesFact> for Provider<'_, P> {
    type Query = Read;
    type Value = MetricValuesFact;
    type Error = FactoryError;
    async fn get(&self, read: &Read) -> Result<MetricValuesFact> {
        self.policy_gathered.store(false, Ordering::Release);
        let mut gathered = self
            .metrics
            .gather_measurements(&read.plan, read.now, read.window)
            .await?;
        if read.plan.needs_policy() {
            // Defer authored input failures until after the initial lower
            // reads, exactly like the original live metric request.
            let policy = read.policy.as_ref().map_err(copy_input_error)?;
            let policy = policy.as_ref().ok_or_else(|| {
                FactoryError::BadRequest("policy metric declarations were not resolved".into())
            })?;
            self.metrics.gather_policy(&mut gathered, policy).await?;
            self.policy_gathered.store(true, Ordering::Release);
        }
        let computed = self
            .metrics
            .finish(&read.plan, gathered, &read.budgets, read.now, read.window)
            .await?;
        Ok(MetricValuesFact {
            values: computed.values,
        })
    }
}

// Authored input problems are carried downward as request input, never an
// upper-level verdict. Preserve the existing public error variant and prose
// when reporting a borrowed input problem; the original source remains owned
// by the caller's Read, so repeated failed reads return the same error.
fn copy_input_error(error: &FactoryError) -> FactoryError {
    match error {
        FactoryError::TaskNotFound(id) => FactoryError::TaskNotFound(id.clone()),
        FactoryError::NoSuchAdapter {
            kind,
            name,
            available,
        } => FactoryError::NoSuchAdapter {
            kind,
            name: name.clone(),
            available: available.clone(),
        },
        FactoryError::NoSuchScope(scope) => FactoryError::NoSuchScope(scope.clone()),
        FactoryError::Adapter { adapter, message } => FactoryError::Adapter {
            adapter: adapter.clone(),
            message: message.clone(),
        },
        FactoryError::BadRequest(message) => FactoryError::BadRequest(message.clone()),
        FactoryError::Denied(message) => FactoryError::Denied(message.clone()),
        FactoryError::HarnessUnhealthy(message) => FactoryError::HarnessUnhealthy(message.clone()),
        FactoryError::CapacityHeld { agent, in_use, max } => FactoryError::CapacityHeld {
            agent: agent.clone(),
            in_use: *in_use,
            max: *max,
        },
        FactoryError::DispatchSuperseded(message) => {
            FactoryError::DispatchSuperseded(message.clone())
        }
        FactoryError::Other(error) => FactoryError::Other(anyhow::anyhow!(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deferred_input_errors_keep_every_public_error_code_and_message() {
        for error in [
            FactoryError::TaskNotFound("task".into()),
            FactoryError::NoSuchAdapter {
                kind: "runtime",
                name: "missing".into(),
                available: "shell".into(),
            },
            FactoryError::NoSuchScope("missing".into()),
            FactoryError::adapter("sqlite", "unavailable"),
            FactoryError::BadRequest("invalid catalogue".into()),
            FactoryError::Denied("grant".into()),
            FactoryError::HarnessUnhealthy("unavailable".into()),
            FactoryError::CapacityHeld {
                agent: "a".into(),
                in_use: 1,
                max: 1,
            },
            FactoryError::DispatchSuperseded("newer attempt".into()),
            FactoryError::Other(anyhow::anyhow!("input read failed")),
        ] {
            let copied = copy_input_error(&error);
            assert_eq!(copied.code(), error.code());
            assert_eq!(copied.to_string(), error.to_string());
        }
    }
}
