//! Requests served by L6 Direction. The arms are moved verbatim from the single
//! `dispatch_request` match; `Request::level` decides which file a request lands in.
use crate::engine::*;
use super::misrouted;

impl Engine {
    pub(super) async fn route_l6(
        self: &Arc<Self>,
        caller: &crate::access::Caller,
        req: Request,
    ) -> Result<Payload> {
        let l6 = self.l6_service();
        match req {
            Request::Policy { scope } => Ok(Payload::Policy {
                report: l6.policy_report(scope.as_deref()).await?,
            }),
            Request::PolicyClock { scope } => Ok(Payload::PolicyClock {
                clock: l6.policy_clock(scope.as_deref()).await?,
            }),
            Request::PolicyControl { control, scope } => Ok(Payload::PolicyControl {
                detail: l6.policy_control(control, &scope).await?,
            }),
            Request::PolicyAttest {
                control,
                scope,
                evidence,
                note,
                expires_at,
                clock,
                corrective,
            } => {
                let attestation = l6
                    .policy_attest(caller, control, scope, evidence, note, expires_at, clock, corrective)
                    .await?;
                self.shared.bus.publish(Event::PolicyChanged {
                    scope: attestation.scope.clone(),
                    control: attestation.control.clone(),
                });
                Ok(Payload::PolicyAttestation { attestation })
            }
            Request::PolicyWithdraw { id, reason } => {
                let attestation = l6.policy_withdraw(caller, id, reason).await?;
                self.shared.bus.publish(Event::PolicyChanged {
                    scope: attestation.scope.clone(),
                    control: attestation.control.clone(),
                });
                Ok(Payload::PolicyAttestation { attestation })
            }
            // An ordinary task in every way but how it was asked for, so it
            // answers the same way `Request::TaskCreate` itself does --
            // `Event::TaskCreated` already fired inside `policy_remediate`
            // (`Engine::create`), not published a second time here.
            Request::PolicyRemediate { control, scope, agent } => Ok(Payload::Task {
                task: l6.policy_remediate(control, scope, agent).await?,
            }),
            Request::PolicyExport { scope, format } => {
                let (filename, body) = l6.policy_export_render(scope.as_deref(), &format).await?;
                Ok(Payload::PolicyExport { format, filename, body })
            }
            Request::Dashboard { scope } => {
                let (dashboard, source) = self.dashboard_for(scope.as_deref())?;
                Ok(Payload::Dashboard {
                    tiles: dashboard.map(|d| d.tiles),
                    source,
                })
            }
            Request::DashboardSet { scope, tiles } => {
                let scope = self.set_dashboard(&scope, tiles)?;
                self.shared.bus.publish(Event::DashboardChanged { scope: scope.clone() });
                let (dashboard, source) = self.dashboard_for(Some(&scope))?;
                Ok(Payload::Dashboard {
                    tiles: dashboard.map(|d| d.tiles),
                    source,
                })
            }
            Request::DashboardReset { scope } => {
                let scope = self.reset_dashboard(&scope)?;
                self.shared.bus.publish(Event::DashboardChanged { scope: scope.clone() });
                let (dashboard, source) = self.dashboard_for(Some(&scope))?;
                Ok(Payload::Dashboard {
                    tiles: dashboard.map(|d| d.tiles),
                    source,
                })
            }
            Request::Goals { scope, cycle } => Ok(Payload::Goals {
                report: l6.goals_report(scope.as_deref(), cycle.as_deref()).await?,
            }),
            Request::GoalsCheckIn { kr, value, confidence, note } => {
                let checkin = l6.goals_checkin(caller, kr, value, confidence, note).await?;
                self.shared.bus.publish(Event::GoalsChanged { kr: checkin.kr.clone() });
                Ok(Payload::GoalsCheckIn { checkin })
            }
            Request::Scenarios { scope } => Ok(Payload::Scenarios {
                report: l6.scenarios_report(scope.as_deref()).await?,
            }),
            // No event: promote creates ordinary tasks through `Engine::create`,
            // which already publishes `Event::TaskCreated` for each one --
            // the same "not published a second time here" rule
            // `PolicyRemediate` follows just above.
            Request::ScenarioPromote { scenario, scope, agent } => Ok(Payload::ScenarioPromote {
                result: l6.scenario_promote(scenario, scope, agent).await?,
            }),
            Request::ScenarioWhatIf { scenario, drivers, scope } => Ok(Payload::ScenarioWhatIf {
                result: l6.scenario_whatif(scenario, drivers, scope.as_deref()).await?,
            }),
            Request::Budget { scope, group_by } => Ok(Payload::Budget {
                report: l6.budget_report(scope.as_deref(), group_by, Utc::now()).await?,
            }),
            other => Err(misrouted(other.level())),
        }
    }
}
