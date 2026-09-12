//! What the daemon has to choose from. Built-ins are registered first; plugins
//! are discovered after and may not take a name that is already taken.

use factory_core::adapter::{AdapterKind, Agent, AgentRuntime, TaskStore};
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::{AdapterEntry, AdapterList};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::builtin::{HarnessAgent, HerdrRuntime, ShellAgent};
use crate::host::PluginProcess;
use crate::manifest;
use crate::proxy::{PluginAgent, PluginStore};

#[derive(Default)]
pub struct Registry {
    agents: BTreeMap<String, Arc<dyn Agent>>,
    runtimes: BTreeMap<String, Arc<dyn AgentRuntime>>,
    stores: BTreeMap<String, Arc<dyn TaskStore>>,
    sources: BTreeMap<(&'static str, String), String>,
    processes: Vec<Arc<PluginProcess>>,
    /// Plugins that could not be loaded, in the words a person can act on.
    pub problems: Vec<String>,
}

impl Registry {
    /// The adapters that ship in the box.
    pub fn with_builtins() -> Self {
        let mut r = Self::default();
        r.add_agent(Arc::new(HarnessAgent::claude_code()), "builtin");
        r.add_agent(Arc::new(HarnessAgent::pi()), "builtin");
        r.add_agent(Arc::new(HarnessAgent::codex()), "builtin");
        r.add_agent(Arc::new(HarnessAgent::opencode()), "builtin");
        r.add_agent(Arc::new(ShellAgent), "builtin");
        r.add_runtime(Arc::new(HerdrRuntime::new()), "builtin");
        r
    }

    pub fn add_agent(&mut self, agent: Arc<dyn Agent>, source: &str) {
        self.sources
            .insert(("agent", agent.name().to_string()), source.to_string());
        self.agents.insert(agent.name().to_string(), agent);
    }

    pub fn add_runtime(&mut self, runtime: Arc<dyn AgentRuntime>, source: &str) {
        self.sources
            .insert(("runtime", runtime.name().to_string()), source.to_string());
        self.runtimes.insert(runtime.name().to_string(), runtime);
    }

    pub fn add_store(&mut self, store: Arc<dyn TaskStore>, source: &str) {
        self.sources
            .insert(("task", store.name().to_string()), source.to_string());
        self.stores.insert(store.name().to_string(), store);
    }

    /// Load every plugin under `dir`. Nothing here can fail the daemon: a
    /// plugin that will not load becomes a problem to report, not a panic.
    pub async fn load_plugins(&mut self, dir: &Path) {
        let (plugins, mut problems) = manifest::discover(dir);
        self.problems.append(&mut problems);

        for plugin in plugins {
            let name = plugin.manifest.name.clone();
            let kind = plugin.manifest.kind;
            let taken = match kind {
                AdapterKind::Agent => self.agents.contains_key(&name),
                AdapterKind::Runtime => self.runtimes.contains_key(&name),
                AdapterKind::Task => self.stores.contains_key(&name),
                AdapterKind::Interface => false,
            };
            if taken {
                self.problems.push(format!(
                    "{}: a {kind} adapter named {name:?} is already registered; \
                     rename the plugin rather than shadowing the built-in",
                    plugin.manifest_path.display()
                ));
                continue;
            }

            let proc = Arc::new(PluginProcess::new(plugin.clone()));
            // Make it prove it starts and speaks the protocol now, so a broken
            // plugin shows up at startup rather than when a task needs it.
            if let Err(e) = proc.describe().await {
                self.problems
                    .push(format!("{}: {e}", plugin.manifest_path.display()));
                proc.shutdown().await;
                continue;
            }

            let source = proc.source();
            match kind {
                AdapterKind::Agent => self.add_agent(Arc::new(PluginAgent::new(proc.clone())), &source),
                AdapterKind::Task => self.add_store(Arc::new(PluginStore::new(proc.clone())), &source),
                AdapterKind::Runtime => {
                    self.problems.push(format!(
                        "{}: runtime plugins are not wired up yet -- the seam is \
                         there, the proxy is not",
                        plugin.manifest_path.display()
                    ));
                    proc.shutdown().await;
                    continue;
                }
                AdapterKind::Interface => {
                    self.problems.push(format!(
                        "{}: interface plugins are not wired up yet -- the seam is \
                         there, the proxy is not",
                        plugin.manifest_path.display()
                    ));
                    proc.shutdown().await;
                    continue;
                }
            }
            self.processes.push(proc);
        }
    }

    pub fn agent(&self, name: &str) -> Result<Arc<dyn Agent>> {
        self.agents.get(name).cloned().ok_or_else(|| FactoryError::NoSuchAdapter {
            kind: "agent",
            name: name.to_string(),
            available: keys(&self.agents),
        })
    }

    pub fn runtime(&self, name: &str) -> Result<Arc<dyn AgentRuntime>> {
        self.runtimes.get(name).cloned().ok_or_else(|| FactoryError::NoSuchAdapter {
            kind: "runtime",
            name: name.to_string(),
            available: keys(&self.runtimes),
        })
    }

    /// Every runtime currently registered, for something that wants to reach
    /// all of them rather than one by name -- listening for a push from each,
    /// in particular.
    pub fn runtimes(&self) -> Vec<Arc<dyn AgentRuntime>> {
        self.runtimes.values().cloned().collect()
    }

    pub fn store(&self, name: &str) -> Result<Arc<dyn TaskStore>> {
        self.stores.get(name).cloned().ok_or_else(|| FactoryError::NoSuchAdapter {
            kind: "task",
            name: name.to_string(),
            available: keys(&self.stores),
        })
    }

    pub fn list(&self) -> AdapterList {
        let mut adapters = Vec::new();
        for (name, a) in &self.agents {
            adapters.push(self.entry("agent", name, a.description()));
        }
        for (name, r) in &self.runtimes {
            adapters.push(self.entry("runtime", name, r.description()));
        }
        for (name, s) in &self.stores {
            adapters.push(self.entry("task", name, s.description()));
        }
        AdapterList { adapters }
    }

    fn entry(&self, kind: &'static str, name: &str, description: String) -> AdapterEntry {
        AdapterEntry {
            kind: kind.to_string(),
            name: name.to_string(),
            description,
            source: self
                .sources
                .get(&(kind, name.to_string()))
                .cloned()
                .unwrap_or_else(|| "unknown".into()),
        }
    }

    pub async fn shutdown(&self) {
        for p in &self.processes {
            p.shutdown().await;
        }
    }
}

fn keys<V>(map: &BTreeMap<String, V>) -> String {
    if map.is_empty() {
        "none".into()
    } else {
        map.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}
