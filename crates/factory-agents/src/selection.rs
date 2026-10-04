//! L3 owns agent selection and validates its own registered runtime adapters.
use crate::roster::{ForemanConfig, RosterScope, ScopeAgent};
use factory_kernel::{CommandPort, FactoryError, Result, L3};
/// Narrow catalog capability, implemented by the outside plugin registry.
/// The five adapter seams are unchanged; no higher service enters this owner.
pub trait Catalog: Send + Sync {
    fn require_agent(&self, name: &str) -> Result<()>;
    fn require_runtime(&self, name: &str) -> Result<()>;
    fn agent_names(&self) -> Vec<String>;
}
pub trait Selection: CommandPort<Level = L3> + Send + Sync {
    fn select(&self, scope: &str, name: &str, runtime: &str) -> Result<String>;
}
impl<P: Selection + ?Sized> Selection for &P {
    fn select(&self, scope: &str, name: &str, runtime: &str) -> Result<String> {
        (**self).select(scope, name, runtime)
    }
}
pub struct Service<'a> {
    pub catalog: &'a dyn Catalog,
    pub scopes: Vec<RosterScope>,
    pub foreman: ForemanConfig,
}
impl CommandPort for Service<'_> {
    type Level = L3;
}
impl Selection for Service<'_> {
    fn select(&self, scope: &str, name: &str, runtime: &str) -> Result<String> {
        let (name, _, _) = self.resolve_agent(scope, name)?;
        self.catalog.require_runtime(runtime)?;
        Ok(name)
    }
}
impl Service<'_> {
    pub fn resolve_agent(
        &self,
        scope_name: &str,
        name: &str,
    ) -> Result<(String, String, Option<ScopeAgent>)> {
        let scope = factory_kernel::resolve_scope(&self.scopes, scope_name)?;
        let declared_here = crate::roster::agents_with(
            &scope.name,
            scope.agent.as_ref(),
            &scope.agents,
            &self.foreman,
        );
        if let Some(declared) = declared_here.iter().find(|a| a.name() == name).cloned() {
            // The name resolves; the adapter behind it still has to exist.
            self.catalog.require_agent(&declared.harness)?;
            return Ok((declared.name(), declared.harness.clone(), Some(declared)));
        }
        if self.catalog.require_agent(name).is_ok() {
            return Ok((name.to_string(), name.to_string(), None));
        }

        let declared: Vec<String> = declared_here.iter().map(|a| a.name()).collect();
        let adapters: Vec<String> = self.catalog.agent_names();
        Err(FactoryError::BadRequest(format!(
            "scope {scope_name:?} has no agent named {name:?}. It declares: {}. \
             Any adapter also works: {}.",
            if declared.is_empty() {
                "none".into()
            } else {
                declared.join(", ")
            },
            adapters.join(", "),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Catalog {
        calls: Mutex<Vec<String>>,
    }
    impl super::Catalog for Catalog {
        fn require_agent(&self, name: &str) -> Result<()> {
            self.calls.lock().unwrap().push(format!("agent:{name}"));
            if name == "shell" {
                Ok(())
            } else {
                Err(FactoryError::BadRequest(format!("missing agent {name}")))
            }
        }
        fn require_runtime(&self, name: &str) -> Result<()> {
            self.calls.lock().unwrap().push(format!("runtime:{name}"));
            if name == "herdr" {
                Ok(())
            } else {
                Err(FactoryError::BadRequest(format!("missing runtime {name}")))
            }
        }
        fn agent_names(&self) -> Vec<String> {
            vec!["shell".into()]
        }
    }
    #[test]
    fn selection_resolves_alias_declared_names_and_validates_runtime_before_ack() {
        let catalog = Catalog {
            calls: Mutex::new(vec![]),
        };
        let declared =
            serde_json::from_value(serde_json::json!({"name":"worker","harness":"shell"})).unwrap();
        let service = Service {
            catalog: &catalog,
            foreman: ForemanConfig::default(),
            scopes: vec![RosterScope {
                name: "projects/demo".into(),
                path: "/tmp/projects/demo".into(),
                agent: None,
                agents: vec![declared],
                roles: Default::default(),
            }],
        };
        assert_eq!(service.select("demo", "worker", "herdr").unwrap(), "worker");
        assert_eq!(
            *catalog.calls.lock().unwrap(),
            vec!["agent:shell", "runtime:herdr"]
        );
        assert!(service
            .select("demo", "worker", "missing")
            .unwrap_err()
            .to_string()
            .contains("missing runtime"));
        assert_eq!(service.select("demo", "shell", "herdr").unwrap(), "shell");
        let unknown = service
            .select("demo", "unknown", "herdr")
            .unwrap_err()
            .to_string();
        assert!(
            unknown.contains("It declares: worker.")
                && unknown.contains("Any adapter also works: shell.")
        );
        assert!(service.select("absent", "worker", "herdr").is_err());
    }
}
