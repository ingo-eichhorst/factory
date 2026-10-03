//! Live fact ports (#193 phase 3). The level modules own gathering;
//! this is only typed wiring. The Engine reference is temporary wiring
//! until phase 6 splits services, not an implementation of Provide on Engine.
mod l1;
mod l2;
mod l3;
mod l4;
mod l5;

use crate::engine::Engine;
use chrono::{DateTime, Utc};
use factory_core::{
    error::{FactoryError, Result},
    operations::Window,
};
use factory_kernel::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
};

/// Exact scope and requested task/workflow names, not an unbounded history.
#[derive(Clone)]
pub(crate) struct NamedQuery {
    pub scope: String,
    pub names: BTreeSet<String>,
}

pub(crate) struct AttestedQuery {
    pub scopes: Option<BTreeSet<String>>,
    pub categories: Option<BTreeSet<String>>,
    pub window: Window,
}

/// Registered producer, with a response constrained to its own fact type.
/// No serialization, Any downcasts or string-keyed provider lookup.
pub(crate) trait Port: Fact + Sized {
    type Query: Sync;
    type Value: FactValue<Self>;
    type Provider<'a>: Provide<Self, Query = Self::Query, Value = Self::Value, Error = FactoryError>;
    fn provider(engine: &Engine) -> Self::Provider<'_>;
}

/// Reader identity is explicit now. Below bounds and level-crate dependency
/// enforcement are phase 4; this handle does not claim to enforce them yet.
pub(crate) struct Facts<'a, R: Level> {
    engine: &'a Engine,
    reader: PhantomData<R>,
}
impl<'a, R: Level> Facts<'a, R> {
    pub(crate) fn new(engine: &'a Engine) -> Self {
        Self {
            engine,
            reader: PhantomData,
        }
    }
    pub(crate) async fn get<F: Port>(&self, query: &F::Query) -> Result<F::Value> {
        F::provider(self.engine).get(query).await
    }
}

macro_rules! port {
    ($fact:ty, $level:ident, $query:ty, $value:ty) => {
        impl Port for $fact {
            type Query = $query;
            type Value = $value;
            type Provider<'a> = $level::Provider<'a>;
            fn provider(engine: &Engine) -> Self::Provider<'_> {
                $level::Provider { engine }
            }
        }
    };
}
port!(DaemonConfigFact, l1, (), DaemonConfigFact);
port!(BackupFact, l1, DateTime<Utc>, BackupFact);
port!(ScopeCapacityFact, l1, (), ScopeCapacityFact);
port!(EnvironmentMetricFact, l1, Option<String>, BTreeMap<String, EnvironmentMetricFact>);
port!(SecretsPresence, l2, BTreeSet<String>, BTreeMap<String, SecretsPresence>);
port!(DependenciesFact, l2, String, DependenciesFact);
port!(ExploitedFinding, l2, String, Vec<ExploitedFinding>);
port!(AgentFact, l3, String, Vec<AgentFact>);
port!(TaskFact, l4, NamedQuery, BTreeMap<String, Vec<TaskFact>>);
port!(WorkflowFact, l4, NamedQuery, BTreeMap<String, Vec<WorkflowFact>>);
port!(AttestedRun, l4, AttestedQuery, Vec<AttestedRun>);
port!(
    ConfirmedSecurityReport,
    l4,
    Option<String>,
    Vec<ConfirmedSecurityReport>
);
port!(GateFact, l5, BTreeSet<String>, BTreeMap<String, GateFact>);
port!(KnowledgeTags, l5, (), KnowledgeTags);

#[cfg(test)]
mod tests {
    use super::*;
    fn registered<F: Port>() {}
    #[test]
    fn every_catalogued_fact_has_a_typed_producer_owned_port() {
        registered::<DaemonConfigFact>();
        registered::<BackupFact>();
        registered::<ScopeCapacityFact>();
        registered::<EnvironmentMetricFact>();
        registered::<SecretsPresence>();
        registered::<DependenciesFact>();
        registered::<ExploitedFinding>();
        registered::<AgentFact>();
        registered::<TaskFact>();
        registered::<WorkflowFact>();
        registered::<AttestedRun>();
        registered::<ConfirmedSecurityReport>();
        registered::<GateFact>();
        registered::<KnowledgeTags>();
        let source = include_str!("mod.rs").split("#[cfg(test)]").next().unwrap();
        let mut wired: Vec<_> = source
            .split("port!(")
            .skip(1)
            .map(|rest| rest.trim_start().split(',').next().unwrap().trim())
            .collect();
        let mut catalogue: Vec<_> = FACT_CATALOGUE.iter().map(|e| e.fact).collect();
        wired.sort_unstable();
        catalogue.sort_unstable();
        assert_eq!(wired, catalogue);
    }
}
