//! The roles in effect where, as the roster, the pickers and the Roles view
//! read them.
//!
//! Nothing here decides what a role allows -- `authorize` does that, against
//! `Engine::roles_for`. This only says the same thing out loud: which roles a
//! scope has, where each was written, what it may do in the words `role.rs`
//! keeps, and who is holding it.

use std::collections::{BTreeMap, HashMap};

use factory_core::agent::AgentSession;
use factory_core::error::Result;
use factory_core::protocol::{GrantView, RoleBoard, RoleHolder, RoleLayer, RoleView};
use factory_core::role::{Grant, Role, RoleEntry, RoleOrigin, Roles};

use crate::engine::Engine;

fn role_view(entry: &RoleEntry) -> RoleView {
    RoleView {
        name: entry.def.name.as_str().to_string(),
        describe: entry.def.describe.clone(),
        grants: entry.def.written().into_iter().map(str::to_string).collect(),
        reach: entry.def.reach.as_str().to_string(),
        origin: entry.origin.clone(),
        overrides: entry.overrides.clone(),
        held_by: Vec::new(),
    }
}

fn views(roles: &Roles) -> Vec<RoleView> {
    roles.entries().map(role_view).collect()
}

/// The definitions one layer wrote, out of the chain that ends at it.
fn written_by(roles: &Roles, origin: &RoleOrigin) -> Vec<RoleView> {
    roles
        .entries()
        .filter(|entry| &entry.origin == origin)
        .map(role_view)
        .collect()
}

/// The layer a scope writes its roles into. The instance root's scope has no
/// `scope.roles` of its own -- it writes the top-level `roles:` every scope
/// starts from -- and every other scope writes its own block.
pub(crate) fn layer_written_by(
    factory: &factory_core::config::Factory,
    scope: &factory_core::config::Scope,
) -> RoleOrigin {
    let is_root = factory
        .config
        .scope
        .as_ref()
        .is_some_and(|root| root.id == scope.id);
    if is_root {
        RoleOrigin::Instance
    } else {
        RoleOrigin::Scope {
            scope: scope.name.clone(),
        }
    }
}

impl Engine {
    /// The roles that hold in every scope, and each scope's own set where it
    /// differs because it, or a scope above it, writes roles of its own. The
    /// roster offers a scope exactly the roles `set_agent_role` will accept
    /// there, and nothing else.
    pub fn role_views(&self) -> (Vec<RoleView>, BTreeMap<String, Vec<RoleView>>) {
        let factory = self.factory_snapshot();
        let everywhere = factory.config.roles().unwrap_or_else(|e| {
            tracing::error!("{e}; falling back to the built-in roles");
            Roles::presets()
        });
        let mut by_scope = BTreeMap::new();
        for scope in &factory.config.scopes {
            let inherits_only = scope.roles.is_empty()
                && factory
                    .config
                    .ancestors_of(scope)
                    .iter()
                    .all(|above| above.roles.is_empty());
            if inherits_only {
                continue;
            }
            by_scope.insert(scope.name.clone(), views(&self.roles_for(&scope.name)));
        }
        (views(&everywhere), by_scope)
    }

    /// The Roles view in one answer: every role in effect in `scope` with who
    /// holds it, and every layer that writes roles across the tree.
    pub async fn role_board(&self, scope: Option<&str>) -> Result<RoleBoard> {
        let factory = self.factory_snapshot();
        let grants = Grant::ALL
            .into_iter()
            .map(|grant| GrantView {
                name: grant.as_str().to_string(),
                describe: grant.describe().to_string(),
                group: grant.group().to_string(),
            })
            .collect();

        let (selected, writes, mut roles) = match scope {
            Some(name) => {
                let found = factory.scope(name)?;
                (
                    Some(found.name.clone()),
                    Some(layer_written_by(&factory, found)),
                    views(&factory.config.roles_for_scope(found)?),
                )
            }
            None => (None, None, views(&factory.config.roles()?)),
        };

        if let Some(name) = &selected {
            let found = factory.scope(name)?;
            for declared in found.agents_with(&factory.config.daemon.foreman) {
                let agent = declared.name();
                // The same answer `effective_role` gives: a role somebody gave
                // wins over the config until it is cleared.
                let given = match self.l3.agents.get_agent(&AgentSession::id_for(name, &agent)).await {
                    Ok(Some(session)) => session.assigned_role,
                    _ => None,
                };
                let holds = given.clone().unwrap_or(declared.role);
                if let Some(view) = roles.iter_mut().find(|view| view.name == holds.as_str()) {
                    view.held_by.push(RoleHolder {
                        name: agent,
                        given: given.is_some(),
                    });
                }
            }
        }

        let mut layers = vec![
            RoleLayer {
                origin: RoleOrigin::Builtin,
                path: None,
                parent: None,
                roles: views(&Roles::presets()),
            },
            RoleLayer {
                origin: RoleOrigin::Instance,
                path: None,
                parent: None,
                roles: written_by(&factory.config.roles()?, &RoleOrigin::Instance),
            },
        ];
        for layer in factory.config.scopes.iter().filter(|s| !s.roles.is_empty()) {
            let origin = RoleOrigin::Scope {
                scope: layer.name.clone(),
            };
            let parent = factory
                .config
                .ancestors_of(layer)
                .into_iter()
                .rev()
                .find(|above| !above.roles.is_empty())
                .map(|above| above.name.clone());
            layers.push(RoleLayer {
                path: factory
                    .scope_path(&layer.name)
                    .ok()
                    .map(|p| p.display().to_string()),
                parent,
                roles: written_by(&factory.config.roles_for_scope(layer)?, &origin),
                origin,
            });
        }

        Ok(RoleBoard {
            grants,
            scope: selected,
            writes,
            roles,
            layers,
        })
    }

    /// Every agent that depends on the definition of `role` written by
    /// `origin`: declared or given that role, in a scope whose chain resolves
    /// the name to that very definition. Deleting it would leave each of them
    /// on a role nothing defines, or on a different definition than the one
    /// they were given -- and neither should happen without a person naming
    /// them first.
    pub(crate) fn dependents_of(
        &self,
        factory: &factory_core::config::Factory,
        origin: &RoleOrigin,
        role: &Role,
        given: &HashMap<String, Role>,
    ) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for scope in &factory.config.scopes {
            let resolves_here = factory
                .config
                .roles_for_scope(scope)?
                .entry(role)
                .is_some_and(|entry| &entry.origin == origin);
            if !resolves_here {
                continue;
            }
            let declared = scope.agents_with(&factory.config.daemon.foreman);
            for agent in &declared {
                let id = AgentSession::id_for(&scope.name, &agent.name());
                let holds = given.get(&id).unwrap_or(&agent.role);
                if holds == role {
                    out.push(format!("{} in {}", agent.name(), scope.name));
                }
            }
            // An agent given the role whose declaration has since gone still
            // holds it, and still asks with it.
            for (id, holds) in given {
                let undeclared = declared
                    .iter()
                    .all(|agent| AgentSession::id_for(&scope.name, &agent.name()) != *id);
                if holds == role
                    && undeclared
                    && id.rsplit_once('/').map(|(s, _)| s) == Some(scope.name.as_str())
                {
                    out.push(format!("{} (given)", id));
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::agent::Lifetime;
    use factory_core::config::{Config, Factory, Scope};
    use factory_plugins::registry::Registry;
    use factory_plugins::SqliteStore;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn engine() -> Arc<Engine> {
        let at = |name: &str, path: &str, rest: &str| {
            let mut scope: Scope =
                serde_yaml_ng::from_str(&format!("id: {name}-id\nname: {name}\n{rest}")).unwrap();
            scope.path = PathBuf::from(path);
            scope
        };
        let mut config: Config = serde_yaml_ng::from_str(
            "instance:\n  id: i\n  name: test\nroles:\n  runner:\n    grants: [task.run]\n    reach: scope\n",
        )
        .unwrap();
        config.scopes = vec![
            at(
                "engineering",
                "projects",
                "roles:\n  reviewer:\n    grants: [task.edit, task.report]\n",
            ),
            at(
                "demo-app",
                "projects/demo",
                "roles:\n  reviewer:\n    describe: reviews, and opens follow-ups\n    grants: [task.create]\nagents:\n  - name: critic\n    harness: pi\n    role: reviewer\n  - name: helper\n    harness: pi\n    lifetime: permanent\n",
            ),
            at("plain", "projects/plain", ""),
            at("elsewhere", "elsewhere", ""),
        ];
        config.validate().unwrap();
        Arc::new(Engine::new(
            Factory {
                root: PathBuf::from("/tmp/factory-roles-board-test"),
                config,
            },
            Registry::with_builtins(),
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            vec![],
        ))
    }

    fn named<'a>(roles: &'a [RoleView], name: &str) -> &'a RoleView {
        roles.iter().find(|r| r.name == name).unwrap()
    }

    #[tokio::test]
    async fn one_scope_shows_every_role_with_where_it_came_from_and_who_holds_it() {
        let e = engine();
        let helper = AgentSession::new("demo-app", "helper", "pi", "herdr", Lifetime::Permanent, Role::worker());
        e.l4.store.put_agent(&helper).await.unwrap();
        e.set_agent_role("demo-app/helper", Some(Role::new("runner"))).await.unwrap();

        let board = e.role_board(Some("demo-app")).await.unwrap();
        assert_eq!(board.scope.as_deref(), Some("demo-app"));
        assert_eq!(board.writes, Some(RoleOrigin::Scope { scope: "demo-app".into() }));
        assert_eq!(board.grants.len(), Grant::ALL.len());
        assert_eq!(board.grants[0].describe, "create tasks");
        assert_eq!(board.grants[0].group, "Tasks");

        let reviewer = named(&board.roles, "reviewer");
        assert_eq!(reviewer.origin, RoleOrigin::Scope { scope: "demo-app".into() });
        assert_eq!(reviewer.overrides, Some(RoleOrigin::Scope { scope: "engineering".into() }));
        assert_eq!(reviewer.grants, ["task.create"]);
        assert_eq!(reviewer.held_by.len(), 1);
        assert_eq!(reviewer.held_by[0].name, "critic");
        assert!(!reviewer.held_by[0].given);

        let runner = named(&board.roles, "runner");
        assert_eq!(runner.origin, RoleOrigin::Instance);
        assert_eq!(runner.held_by[0].name, "helper");
        assert!(runner.held_by[0].given, "a role given with `factory agent role` says so");
        assert!(named(&board.roles, "worker").held_by.is_empty());
    }

    #[tokio::test]
    async fn the_layers_nest_by_path_and_say_what_each_one_replaces() {
        let board = engine().role_board(None).await.unwrap();
        let origins: Vec<RoleOrigin> = board.layers.iter().map(|l| l.origin.clone()).collect();
        assert_eq!(
            origins,
            [
                RoleOrigin::Builtin,
                RoleOrigin::Instance,
                RoleOrigin::Scope { scope: "engineering".into() },
                RoleOrigin::Scope { scope: "demo-app".into() },
            ],
            "scopes that define nothing are not repeated"
        );
        assert_eq!(board.layers[2].parent, None);
        assert_eq!(board.layers[3].parent.as_deref(), Some("engineering"));
        assert_eq!(
            board.layers[3].roles[0].overrides,
            Some(RoleOrigin::Scope { scope: "engineering".into() })
        );
        assert!(board.roles.iter().all(|r| r.name != "reviewer"), "all scopes: only what holds everywhere");
    }

    #[test]
    fn the_roster_is_offered_each_scopes_own_roles_only_where_they_differ() {
        let (everywhere, by_scope) = engine().role_views();
        assert!(everywhere.iter().any(|r| r.name == "runner"));
        assert!(everywhere.iter().all(|r| r.name != "reviewer"));
        assert!(by_scope["plain"].iter().any(|r| r.name == "reviewer"), "plain inherits engineering's");
        assert!(!by_scope.contains_key("elsewhere"), "nothing above elsewhere writes roles");
    }
}
