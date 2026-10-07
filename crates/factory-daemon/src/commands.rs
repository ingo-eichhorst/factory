//! Outside-stack constructors for physical level command services.
use crate::engine::Engine;
use factory_core::{
    error::{FactoryError, Result},
    event::{Event, EventBus},
    task::{Task, TaskEntry},
};
use factory_kernel::Commands;

pub(crate) struct CreationObserver(pub EventBus);
impl factory_process::creation::Observer for CreationObserver {
    fn entry(&self, id: &str, entry: TaskEntry) {
        self.0.publish(Event::TaskEntry {
            id: id.into(),
            entry,
        });
    }
    fn created(&self, task: Task) {
        self.0.publish(Event::TaskCreated { task });
    }
}
fn agent_inputs(
    snapshot: &factory_core::config::Factory,
) -> Vec<factory_agents::roster::RosterScope> {
    snapshot
        .config
        .scopes
        .iter()
        .map(|scope| factory_agents::roster::RosterScope {
            name: scope.name.clone(),
            path: scope.path.clone(),
            agent: scope.agent.clone(),
            agents: scope.agents.clone(),
            roles: scope.roles.clone(),
        })
        .collect()
}
pub(crate) fn agents(engine: &Engine) -> factory_agents::selection::Service<'_> {
    let snapshot = engine.factory_snapshot();
    factory_agents::selection::Service {
        catalog: &engine.shared.registry,
        scopes: agent_inputs(&snapshot),
        foreman: snapshot.config.daemon.foreman.clone(),
    }
}
pub(crate) fn process<'a>(
    engine: &'a Engine,
    observer: &'a CreationObserver,
) -> factory_process::creation::Service<'a, factory_agents::selection::Service<'a>> {
    process_with(engine.l4.store.as_ref(), &engine.shared.registry, &engine.factory_snapshot(), observer)
}
/// The task creation service over the pieces it needs, so the L4 service can build it from its own state.
pub(crate) fn process_with<'a>(
    store: &'a dyn factory_core::adapter::TaskStore,
    registry: &'a factory_plugins::registry::Registry,
    snapshot: &factory_core::config::Factory,
    observer: &'a CreationObserver,
) -> factory_process::creation::Service<'a, factory_agents::selection::Service<'a>> {
    let agents = factory_agents::selection::Service {
        catalog: registry,
        scopes: agent_inputs(snapshot),
        foreman: snapshot.config.daemon.foreman.clone(),
    };
    factory_process::creation::Service {
        store,
        observer,
        agents: Commands::new(agents),
        inputs: factory_process::creation::Inputs {
            scopes: snapshot
                .config
                .scopes
                .iter()
                .map(|scope| factory_process::creation::Scope {
                    name: scope.name.clone(),
                    path: scope.path.clone(),
                    agent: scope.agent.clone(),
                    runtime: scope.runtime.clone(),
                })
                .collect(),
            default_agent: snapshot.config.daemon.default_agent.clone(),
            default_runtime: snapshot.config.daemon.default_runtime.clone(),
        },
    }
}
/// Legacy task payloads are composed beside the ladder, never returned by a port.
pub(crate) async fn task_snapshot(engine: &Engine, id: String) -> Result<Task> {
    let fact = crate::facts::Facts::<factory_kernel::People>::new(engine)
        .get::<factory_kernel::TaskSnapshotFact>(&id)
        .await?;
    serde_json::from_value(fact.0).map_err(|e| FactoryError::Other(e.into()))
}

type Process<'a> = factory_process::creation::Service<'a, factory_agents::selection::Service<'a>>;
type Inventory<'a> = factory_process::facts::Provider<'a>;
type Assurance<'a> = factory_assurance::remediation::Service<Process<'a>, Inventory<'a>>;
pub(crate) type Direction<'a> = factory_direction::remediation::Service<Assurance<'a>, Inventory<'a>>;

pub(crate) fn assurance<'a>(engine: &'a Engine, observer: &'a CreationObserver) -> Assurance<'a> {
    Assurance {
        tasks: Commands::new(process(engine, observer)),
        inventory: <factory_kernel::TaskInventoryFact as crate::facts::Port>::provider(engine),
    }
}
pub(crate) fn direction<'a>(engine: &'a Engine, observer: &'a CreationObserver) -> Direction<'a> {
    Direction {
        assurance: Commands::new(assurance(engine, observer)),
        inventory: <factory_kernel::TaskInventoryFact as crate::facts::Port>::provider(engine),
    }
}
/// The one way L4 reaches L3: a sealed `Commands<L4, _>` edge over the L3 service (S8a).
pub(crate) fn l3(engine: &Engine) -> Commands<factory_kernel::L4, crate::dispatch_port::L3Port<'_>> {
    Commands::new(crate::dispatch_port::L3Port(engine.l3_service()))
}

/// The caller's side of L2's `Notes`: each thing L2 reports becomes a `sandbox` entry on the run's
/// task, appended and published exactly as `Engine::entry` does it.
pub(crate) struct EntryNotes {
    pub(crate) store: std::sync::Arc<dyn factory_core::adapter::TaskStore>,
    pub(crate) bus: EventBus,
}
impl EntryNotes {
    pub(crate) fn of(engine: &Engine) -> Self {
        Self::new(engine.l4.store.clone(), engine.shared.bus.clone())
    }

    pub(crate) fn new(store: std::sync::Arc<dyn factory_core::adapter::TaskStore>, bus: EventBus) -> Self {
        Self { store, bus }
    }
}
#[async_trait::async_trait]
impl factory_environment::provision::Notes for EntryNotes {
    async fn note(&self, note: factory_environment::provision::Note) {
        let mut entry = TaskEntry::new("daemon", "sandbox", note.message).in_run(&note.run);
        if let Some(data) = note.data {
            entry = entry.with_data(data);
        }
        if let Err(e) = self.store.append_entry(&note.task, &entry).await {
            tracing::warn!(task = %note.task, "could not record journal entry: {e}");
        }
        self.bus.publish(Event::TaskEntry { id: note.task, entry });
    }
}

/// The runs that have not finished, as L4 reads them for L2's reconcile.
pub(crate) struct StoreLedger(pub(crate) std::sync::Arc<dyn factory_core::adapter::TaskStore>);
#[async_trait::async_trait]
impl factory_environment::provision::RunLedger for StoreLedger {
    async fn active_runs(&self) -> Result<std::collections::BTreeSet<String>> {
        Ok(self.0.active_runs().await?.into_iter().map(|run| run.id).collect())
    }
}
