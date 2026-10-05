//! Outside preparation of authored downward inputs and legacy wire adapters.
//! The actual metric-backed provider/cache is owned by L5, not Operations.
use crate::{engine::Engine, facts::Facts};
use chrono::{DateTime, Utc};
use factory_assurance::{
    metrics_service::Plan,
    signposts::{Read, ScenarioInput},
};
#[cfg(test)]
use factory_core::protocol::TriggeredSignpost;
use factory_core::scenario;
use factory_kernel::{FactoryError, People, Result, SignpostFact};
use std::sync::Arc;
fn fingerprint(dir: &std::path::Path) -> factory_assurance::signposts::Revision {
    let mut out: factory_assurance::signposts::Revision = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let meta = e.metadata().ok()?;
                    Some((e.file_name(), meta.modified().ok(), meta.len()))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

impl Engine {
    pub(crate) async fn signposts_fact(
        self: &Arc<Self>,
        now: DateTime<Utc>,
        use_cache: bool,
    ) -> Result<SignpostFact> {
        let snapshot = self.factory_snapshot();
        let dir = scenario::scenarios_dir(&snapshot.root);
        let (scenarios, revision) = tokio::task::spawn_blocking(move || {
            let revision = fingerprint(&dir);
            let (scenarios, _) = scenario::load(&dir);
            (scenarios, revision)
        })
        .await
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("scenario directory walk: {e}")))?;
        let scenarios = scenarios
            .into_iter()
            .map(|s| ScenarioInput {
                name: s.name,
                signposts: s.signposts,
            })
            .collect::<Vec<_>>();
        let ids = factory_assurance::signposts::metric_ids(&scenarios);
        let plan = Plan::prepare(
            &ids,
            snapshot.root.clone(),
            &snapshot.scope_tree(),
            &crate::quality::quality_configuration(&snapshot),
            &crate::metrics::reported_configuration(&snapshot),
            None,
        )
        .await?;
        let policy = if plan.needs_policy() {
            Some(self.metric_policy_inputs(&snapshot, None).await?)
        } else {
            None
        };
        let budgets = self.metric_quality_budgets(&snapshot, &plan).await;
        Facts::<People>::new(self)
            .get::<SignpostFact>(&Read {
                now,
                plan,
                scenarios,
                policy,
                budgets,
                revision,
                use_cache,
            })
            .await
    }
    /// Compatibility read for in-process Scenarios consumers: still fresh,
    /// but now through the actual L5 fact rather than an Engine metric loop.
    #[cfg(test)]
    pub(crate) async fn triggered_signposts(
        self: &Arc<Self>,
        now: DateTime<Utc>,
    ) -> Result<Vec<TriggeredSignpost>> {
        self.signposts_fact(now, false)
            .await?
            .triggered
            .into_iter()
            .map(|row| {
                TriggeredSignpost::try_from(row)
                    .map_err(|error| FactoryError::Other(anyhow::anyhow!(error)))
            })
            .collect()
    }
    #[cfg(test)]
    pub(crate) async fn triggered_signposts_cached(
        self: &Arc<Self>,
        now: DateTime<Utc>,
    ) -> Result<Vec<TriggeredSignpost>> {
        self.signposts_fact(now, true)
            .await?
            .triggered
            .into_iter()
            .map(|row| {
                TriggeredSignpost::try_from(row)
                    .map_err(|error| FactoryError::Other(anyhow::anyhow!(error)))
            })
            .collect()
    }
}
