//! Requests served by L3 Agent. The arms are moved verbatim from the single
//! `dispatch_request` match; `Request::level` decides which file a request lands in.
use crate::engine::*;
use crate::l3_service::Asker;
use super::misrouted;

/// Who asked, in the plain words L3 journals (`#330`).
fn asker(caller: &crate::access::Caller) -> Asker {
    Asker {
        by: caller.describe(),
        source: match caller {
            crate::access::Caller::Owner => "owner",
            crate::access::Caller::Agent { .. } => "agent",
        },
    }
}

impl Engine {
    pub(super) async fn route_l3(
        self: &Arc<Self>,
        caller: &crate::access::Caller,
        req: Request,
    ) -> Result<Payload> {
        match req {
            Request::RuntimeConnections => Ok(Payload::RuntimeConnections {
                runtimes: self.runtime_connections().await,
            }),
            Request::Agents => {
                let (mut scopes, available) = self.scope_views().await?;
                self.l3_service().decorate_instances(&mut scopes);
                let (roles, scope_roles) = self.l3_service().role_views();
                Ok(Payload::Scopes {
                    scopes,
                    available,
                    roles,
                    scope_roles,
                })
            }
            Request::RoleList { scope } => Ok(Payload::Roles {
                board: self.l3_service().role_board(scope.as_deref()).await?,
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
            Request::AgentStart { scope, name, instance } => Ok(Payload::Agent {
                agent: self
                    .l3_service()
                    .start_asked(&scope, &name, instance, &asker(caller))
                    .await?
                    .redacted(),
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
                        if let Err(error) = engine.l3_service().start_agent(&scope, &name).await {
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
                self.l3_service().reconcile_agents().await;
                Ok(Payload::Deleted { deleted: true })
            }
            // The one handler `factory agent scale`, `POST /api/agents/scale`
            // and the roster's − / + all reach (`#329`): the count is written
            // into the scope's own config first, so a refused edit never
            // starts or stops anything, then the sessions are lined up with
            // it. New instances start in the background, like a newly
            // configured agent; the answer says which.
            Request::AgentScale { scope, name, instances, by } => {
                let (scope, previous, wanted) = self.set_agent_instances(&scope, &name, instances, by)?;
                let (scale, to_start) = self.l3_service().scale_agent(&scope, &name, previous).await?;
                let asked = caller.describe();
                tracing::info!(scope, name, previous, wanted, by = %asked, "scaled: {}", scale.summary());
                self.l3_service().asked(
                    &asker(caller),
                    "scaled",
                    &AgentSession::id_for(&scope, &name),
                    &scope,
                    &name,
                    format!("{} scaled {previous} -> {wanted} by {asked}: {}", AgentSession::id_for(&scope, &name), scale.summary()),
                );
                self.shared.bus.publish(Event::AgentScaled {
                    scope: scope.clone(),
                    name: name.clone(),
                    previous,
                    wanted,
                    by: asked,
                    summary: scale.summary(),
                });
                if !to_start.is_empty() {
                    let engine = self.clone();
                    tokio::spawn(async move {
                        for instance in to_start {
                            if let Err(error) = engine.l3_service().start_instance(&scope, &name, instance).await {
                                tracing::warn!(scope, name, instance, "could not start a scaled-up instance: {error}");
                            }
                        }
                    });
                }
                Ok(Payload::Scaled { scale })
            }
            Request::AgentStop { id } => Ok(Payload::Agent {
                agent: self.l3_service().stop_agent(&id, &asker(caller)).await?.redacted(),
            }),
            Request::AgentStopCheck { id } => Ok(Payload::StopCheck {
                check: self.l3_service().stop_check(&id).await?,
            }),
            Request::AgentAttach { id } => Ok(Payload::Attach {
                attach: self.l3_service().attach(&id, &asker(caller)).await?,
            }),
            Request::AgentRole { id, role } => Ok(Payload::Agent {
                agent: self
                    .l3_service().set_agent_role(&id, role.map(Role::new))
                    .await?
                    .redacted(),
            }),
            Request::AgentInput { id, text, keys } => {
                self.l3_service().agent_input(&id, text.as_deref(), &keys).await?;
                Ok(Payload::Ok)
            }
            Request::AgentOutput { id, lines } => Ok(Payload::Text {
                text: self.l3_service().agent_output(&id, lines.unwrap_or(200)).await?,
            }),
            Request::AgentScreen { id } => match self.l3_service().agent_screen(&id).await? {
                Some(screen) => Ok(Payload::Screen { screen }),
                None => Err(FactoryError::BadRequest(
                    "this runtime cannot render a screen for that session".into(),
                )),
            },
            other => Err(misrouted(other.level())),
        }
    }
}
