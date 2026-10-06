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
    let snapshot = engine.factory_snapshot();
    let agents = factory_agents::selection::Service {
        catalog: &engine.shared.registry,
        scopes: agent_inputs(&snapshot),
        foreman: snapshot.config.daemon.foreman.clone(),
    };
    factory_process::creation::Service {
        store: engine.l4.store.as_ref(),
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
