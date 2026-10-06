//! Requests served by L3 Agent. The arms are moved verbatim from the single
//! `dispatch_request` match; `Request::level` decides which file a request lands in.
use crate::engine::*;
use super::misrouted;

impl Engine {
    pub(super) async fn route_l3(
        self: &Arc<Self>,
        _caller: &crate::access::Caller,
        req: Request,
    ) -> Result<Payload> {
        match req {
            Request::RuntimeConnections => Ok(Payload::RuntimeConnections {
                runtimes: self.runtime_connections().await,
            }),
            Request::Agents => {
                let (scopes, available) = self.scope_views().await?;
                let (roles, scope_roles) = self.role_views();
                Ok(Payload::Scopes {
                    scopes,
                    available,
                    roles,
                    scope_roles,
                })
            }
            Request::RoleList { scope } => Ok(Payload::Roles {
                board: self.role_board(scope.as_deref()).await?,
            }),
            Request::RoleDefine {
                scope,
                name,
                role,
                replace,
            } => {
                let (scope, name) = self.define_role(&scope, &name, role, replace).await?;
                self.shared.bus.publish(Event::RolesChanged { scope, name });
                Ok(Payload::Ok)
            }
            Request::RoleDelete { scope, name } => {
                let (scope, name) = self.delete_role(&scope, &name).await?;
                self.shared.bus.publish(Event::RolesChanged { scope, name });
                Ok(Payload::Deleted { deleted: true })
            }
            Request::AgentStart { scope, name } => Ok(Payload::Agent {
                agent: self.start_agent(&scope, &name).await?.redacted(),
            }),
            Request::AgentConfigure { scope, agent } => {
                let (scope, agent) = self.configure_agent(&scope, agent)?;
                let name = agent.name();
                let autostart = agent.lifetime.is_standing() && agent.autostart();
                self.shared.bus.publish(Event::AgentConfigured {
                    scope: scope.clone(),
                    name: name.clone(),
                });
                if autostart {
                    let engine = self.clone();
                    tokio::spawn(async move {
                        if let Err(error) = engine.start_agent(&scope, &name).await {
                            tracing::warn!(scope, name, "could not start newly configured agent: {error}");
                        }
                    });
                }
                Ok(Payload::Ok)
            }
            Request::AgentDelete { scope, name } => {
                let (scope, removed) = self.delete_agent_declaration(&scope, &name)?;
                let name = removed.name();
                self.shared.bus.publish(Event::AgentDeleted { scope, name });
                self.reconcile_agents().await;
                Ok(Payload::Deleted { deleted: true })
            }
            Request::AgentStop { id } => Ok(Payload::Agent {
                agent: self.stop_agent(&id).await?.redacted(),
            }),
            Request::AgentRole { id, role } => Ok(Payload::Agent {
                agent: self
                    .set_agent_role(&id, role.map(Role::new))
                    .await?
                    .redacted(),
            }),
            Request::AgentInput { id, text, keys } => {
                self.agent_input(&id, text.as_deref(), &keys).await?;
                Ok(Payload::Ok)
            }
            Request::AgentOutput { id, lines } => Ok(Payload::Text {
                text: self.agent_output(&id, lines.unwrap_or(200)).await?,
            }),
            Request::AgentScreen { id } => match self.agent_screen(&id).await? {
                Some(screen) => Ok(Payload::Screen { screen }),
                None => Err(FactoryError::BadRequest(
                    "this runtime cannot render a screen for that session".into(),
                )),
            },
            other => Err(misrouted(other.level())),
        }
    }
}
