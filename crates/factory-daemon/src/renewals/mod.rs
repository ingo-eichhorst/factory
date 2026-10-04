//! Important dates (#236). L1 and L2 own their read-only metadata caches;
//! L6 supervises dates through lower fact ports and its existing policy
//! clocks. People/UI composition does not turn this into an L1 upward read.
pub(crate) mod declarations;
pub(crate) mod probes;
pub(crate) mod store;
use crate::{engine::Engine, facts::Facts};
use chrono::{DateTime, Utc};
use factory_core::{config::Factory, error::Result, event::Event, renewals::*};
use factory_kernel::{
    CredentialExpiryFact, InfrastructureExpiryFact, ScheduledRunDatesFact, L1, L2, L6,
};
use std::{collections::BTreeSet, process::Stdio, sync::Arc, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

impl Engine {
    pub(crate) fn with_renewal_stores(
        mut self,
        infrastructure: store::ObservationStore<L1>,
        credentials: store::ObservationStore<L2>,
        alerts: store::AlertStore,
    ) -> Self {
        self.infrastructure_expiries = infrastructure;
        self.credential_expiries = credentials;
        self.renewal_alerts = alerts;
        self
    }
    /// Fast metadata cache read, never a credential/TLS probe on page GET.
    pub(crate) async fn important_dates(
        &self,
        scope: Option<&str>,
    ) -> Result<ImportantDatesReport> {
        let snapshot = self.factory_snapshot();
        let (asked, targets) = factory_core::config::subtree_scopes(&snapshot, scope)?;
        let mut names: BTreeSet<_> = targets.iter().map(|scope| scope.name.clone()).collect();
        if let Some(asked) = asked {
            names.insert(asked.name);
        }
        let now = Utc::now();
        let facts = Facts::<L6>::new(self);
        let mut observations = facts
            .get::<InfrastructureExpiryFact>(&())
            .await?
            .observations;
        observations.extend(facts.get::<CredentialExpiryFact>(&()).await?.observations);
        let runs = facts.get::<ScheduledRunDatesFact>(&()).await?.runs;
        let declarations = facts
            .get::<factory_kernel::RenewalDeclarationsFact>(&())
            .await?;
        let mut entries = compose(
            &snapshot,
            &declarations.declarations,
            observations,
            &runs,
            now,
        );
        // Same-level native state, projected on read, not copied to a ledger.
        let attestations = self.policies.all().await?;
        for attestation in &attestations {
            if attestation.clock.is_some() || attestation.corrective.is_some() {
                continue;
            }
            let mut item = probes::observation(
                format!("policy:{}", attestation.id),
                format!(
                    "Policy {} attestation {}",
                    attestation.control, attestation.id
                ),
                DateKind::Other,
                DateSource::PolicyAttestation,
                now,
            );
            item.scope = Some(attestation.scope.clone());
            item.expires_at = Some(attestation.expires_at);
            item.basis = DateBasis::Observed;
            item.observed_at = Some(attestation.attested_at);
            item.detail =
                "native policy attestation expiry; policy owns validity and withdrawal".into();
            item.renew = "review the control and record fresh evidence on Policy".into();
            item.owner = attestation.attested_by.clone();
            item.affects = vec![scope_dependency(&attestation.scope)];
            let mut date = factory_core::renewals::entry(item, None, now, &[]);
            date.href = href(
                &attestation.scope,
                "policy",
                &attestation.control.to_string(),
            );
            date.resolved = attestation.withdrawn.is_some()
                || (attestation.expires_at <= now
                    && attestations.iter().any(|fresh| {
                        fresh.scope == attestation.scope
                            && fresh.control == attestation.control
                            && fresh.withdrawn.is_none()
                            && fresh.clock.is_none()
                            && fresh.corrective.is_none()
                            && fresh.expires_at > now
                    }));
            entries.push(date);
        }
        let clock = self.policy_clock(scope).await?;
        for item in clock.items {
            for deadline in item.deadlines {
                let mut observed = probes::observation(
                    format!("cra:{}:{}", item.item, deadline.deadline),
                    format!("CRA {} {}", item.item, deadline.deadline),
                    DateKind::Other,
                    DateSource::CraDeadline,
                    now,
                );
                observed.scope = Some(item.scope.clone());
                observed.affects = vec![scope_dependency(&item.scope)];
                observed.expires_at = Some(deadline.due_at);
                observed.basis = DateBasis::Observed;
                observed.observed_at = Some(clock.now);
                observed.lead_seconds = 6 * 3600; // existing clock's presentation window, not a new legal clock
                observed.detail = format!("native CRA clock: {}", deadline.state);
                observed.renew = "record the submission and evidence through the CRA clock".into();
                let mut date = factory_core::renewals::entry(observed, None, now, &[]);
                date.resolved = matches!(
                    deadline.state,
                    factory_core::reporting_clock::ClockDeadlineState::Met
                        | factory_core::reporting_clock::ClockDeadlineState::Late
                ) || item.excluded.is_some();
                date.href = href(&item.scope, "policy", "cra/art-14");
                entries.push(date);
            }
        }
        if scope.is_some() {
            entries.retain(|entry| {
                entry
                    .observation
                    .scope
                    .as_ref()
                    .is_some_and(|scope| names.contains(scope))
                    || entry.observation.affects.iter().any(|dependency| {
                        dependency
                            .scope
                            .as_ref()
                            .is_some_and(|scope| names.contains(scope))
                    })
                    || (entry.observation.scope.is_none() && entry.observation.affects.is_empty())
            });
            for entry in &mut entries {
                entry.observation.affects.retain(|dependency| {
                    dependency
                        .scope
                        .as_ref()
                        .is_none_or(|scope| names.contains(scope))
                });
                entry
                    .scheduled_risks
                    .retain(|run| names.contains(&run.scope));
            }
        }
        let mut issues = declarations.findings;
        issues.extend(entries.iter().filter_map(|entry| {
            entry
                .observation
                .issue
                .as_ref()
                .map(|issue| format!("{}: {issue}", entry.observation.name))
        }));
        Ok(factory_core::renewals::report(entries, now, issues))
    }
}

fn scope_dependency(scope: &str) -> DateDependency {
    DateDependency {
        scope: Some(scope.into()),
        agent: None,
        environment: None,
        provider: None,
        label: scope.into(),
    }
}
fn dependencies(
    snapshot: &Factory,
    labels: &[String],
    default_scope: Option<&str>,
) -> Vec<DateDependency> {
    if labels.is_empty() {
        return default_scope.map(scope_dependency).into_iter().collect();
    }
    labels
        .iter()
        .flat_map(|label| {
            if let Ok(scope) = snapshot.scope(label) {
                return vec![scope_dependency(&scope.name)];
            }
            for name in snapshot.scope_names().into_iter().rev() {
                let Ok(scope) = snapshot.scope(&name) else {
                    continue;
                };
                if let Some(agent) =
                    label
                        .strip_prefix(&format!("{}/", scope.name))
                        .filter(|agent| {
                            scope
                                .declared_agents()
                                .iter()
                                .any(|declared| declared.name() == *agent)
                        })
                {
                    return vec![DateDependency {
                        scope: Some(scope.name.clone()),
                        agent: Some(agent.into()),
                        environment: None,
                        provider: None,
                        label: label.clone(),
                    }];
                }
            }
            if let Some(environment) = label.strip_prefix("environment:") {
                if let Some((scope, env)) = snapshot
                    .config
                    .environments()
                    .into_iter()
                    .find(|(_, env)| env.name == environment)
                {
                    return vec![DateDependency {
                        scope: Some(scope),
                        agent: None,
                        environment: Some(env.name),
                        provider: None,
                        label: label.clone(),
                    }];
                }
            }
            if let Some(provider) = label.strip_prefix("provider:") {
                let mut out = Vec::new();
                for scope in &snapshot.config.scopes {
                    for agent in scope.declared_agents() {
                        if agent.provider.as_deref() == Some(provider) {
                            out.push(DateDependency {
                                scope: Some(scope.name.clone()),
                                agent: Some(agent.name()),
                                environment: None,
                                provider: Some(provider.into()),
                                label: format!("{}/{}", scope.name, agent.name()),
                            });
                        }
                    }
                }
                if !out.is_empty() {
                    return out;
                }
            }
            vec![DateDependency {
                scope: None,
                agent: None,
                environment: None,
                provider: None,
                label: format!("unresolved dependency: {label}"),
            }]
        })
        .collect()
}

fn compose(
    snapshot: &Factory,
    declarations: &[ScopedRenewalDeclaration],
    observations: Vec<ExpiryObservation>,
    runs: &[ScheduledRunDate],
    now: DateTime<Utc>,
) -> Vec<ImportantDateEntry> {
    let mut used = BTreeSet::new();
    let mut entries = Vec::new();
    for scoped in declarations {
        let scope = scoped.scope.as_deref();
        let decl = &scoped.declaration;
        let identity = decl.observe.as_deref().unwrap_or(&decl.name);
        let declared_dependencies = dependencies(snapshot, &decl.affects, scope);
        let observed = observations
            .iter()
            .find(|observation| observation.id == identity)
            .or_else(|| {
                // The documented Claude declaration has a human-facing alias.
                // Bind it only when the matching credential is unambiguous,
                // narrowed to its actual declared dependants when supplied.
                if decl.observe.is_some() || decl.name != "claude-subscription-token" {
                    return None;
                }
                let mut candidates = observations.iter().filter(|observation| {
                    observation.kind == DateKind::Credential
                        && observation.name.to_ascii_lowercase().contains("claude")
                        && (declared_dependencies.is_empty()
                            || observation.affects.iter().any(|dependency| {
                                declared_dependencies.iter().any(|declared| {
                                    dependency.scope == declared.scope
                                        && declared.agent.as_ref().is_none_or(|agent| {
                                            dependency.agent.as_ref() == Some(agent)
                                        })
                                })
                            }))
                });
                let first = candidates.next();
                if candidates.next().is_none() {
                    first
                } else {
                    None
                }
            });
        let mut observed = observed.cloned().unwrap_or_else(|| {
            probes::observation(
                declaration_id(scope, &decl.name),
                decl.name.clone(),
                decl.kind,
                DateSource::Declaration,
                now,
            )
        });
        if decl.observe.is_some() && observed.source == DateSource::Declaration {
            observed.issue = Some("the named observation is unavailable".into());
        }
        if observed.source != DateSource::Declaration {
            used.insert(observed.id.clone());
            let source_id = observed.id.clone();
            observed.id = declaration_id(scope, &decl.name);
            observed.detail = format!("{} (observation {source_id})", observed.detail);
        }
        if let Some(scope) = scope {
            observed.scope = Some(scope.into());
        }
        observed.affects.extend(declared_dependencies);
        let mut seen_dependencies = BTreeSet::new();
        observed.affects.retain(|dependency| {
            seen_dependencies.insert((
                dependency.scope.clone(),
                dependency.agent.clone(),
                dependency.environment.clone(),
                dependency.provider.clone(),
                dependency.label.clone(),
            ))
        });
        let mut date = factory_core::renewals::entry(observed, Some(decl), now, runs);
        date.href = scope
            .map(|scope| href(scope, "dates", ""))
            .unwrap_or_else(|| "#all/dates".into());
        entries.push(date);
    }
    entries.extend(
        observations
            .into_iter()
            .filter(|observation| !used.contains(&observation.id))
            .map(|observation| factory_core::renewals::entry(observation, None, now, runs)),
    );
    entries
}

fn declaration_id(scope: Option<&str>, name: &str) -> String {
    format!(
        "renewal:{}",
        serde_json::to_string(&(scope, name)).expect("plain identity serializes")
    )
}

fn encode(text: &str) -> String {
    text.as_bytes()
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (*byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
fn href(scope: &str, page: &str, tail: &str) -> String {
    let tail = if tail.is_empty() {
        String::new()
    } else {
        format!("/{}", encode(tail))
    };
    let scope = if scope == "all" {
        "%61ll".into()
    } else {
        encode(scope)
    };
    format!("#{scope}/{page}{tail}")
}

/// L1/L2 observation job. A metadata tool's latency cannot hold dispatch,
/// startup, or an L6 scheduled-run warning behind it.
async fn observe_loop(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(Duration::from_secs(15));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last = None;
    loop {
        let snapshot = engine.factory_snapshot();
        let fingerprint =
            serde_json::to_string(&(snapshot.config.clone(), snapshot.config.scopes.clone()))
                .unwrap_or_default();
        if last
            .as_ref()
            .is_none_or(|(old, at): &(String, std::time::Instant)| {
                *old != fingerprint || at.elapsed() >= Duration::from_secs(300)
            })
        {
            let observed = tokio::time::timeout(
                Duration::from_secs(90),
                probes::observe(&snapshot, &probes::Tools::default(), Utc::now()),
            )
            .await;
            let observed = observed.unwrap_or_else(|_| probes::Observed::unavailable(Utc::now()));
            if let Err(error) = engine
                .infrastructure_expiries
                .replace(observed.infrastructure, observed.infrastructure_complete)
                .await
            {
                tracing::warn!("infrastructure expiry cache: {error}");
            }
            if let Err(error) = engine
                .credential_expiries
                .replace(observed.credentials, observed.credentials_complete)
                .await
            {
                tracing::warn!("credential expiry cache: {error}");
            }
            last = Some((fingerprint, std::time::Instant::now()));
            engine
                .bus
                .publish(Event::ImportantDatesUpdated { at: Utc::now() });
        }
        tokio::select! { _ = tick.tick() => {}, _ = shutdown.changed() => { if *shutdown.borrow() { return; } } }
    }
}

async fn push(engine: &Engine, report: &ImportantDatesReport) -> Result<()> {
    let Some(hook) = engine.factory_snapshot().config.renewals_notify else {
        return Ok(());
    };
    for entry in report.entries.iter().filter(|entry| {
        !entry.resolved
            && matches!(
                entry.milestone,
                Some(RenewalMilestone::OneDay | RenewalMilestone::Expired)
            )
    }) {
        let identity = serde_json::to_string(&(
            &entry.observation.id,
            entry.observation.expires_at,
            entry.milestone,
        ))
        .expect("plain push identity serializes");
        if !engine
            .renewal_alerts
            .claim(identity.clone(), report.at)
            .await?
        {
            continue;
        }
        let payload = serde_json::to_vec(entry).expect("plain important-date metadata serializes");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", &hook.command])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .env_remove("FACTORY_TOKEN")
            .env_remove("FACTORY_TASK_TOKEN")
            .env_remove("FACTORY_RUN_TOKEN");
        let delivered = match command.spawn() {
            Ok(mut child) => {
                let work = async {
                    let Some(mut stdin) = child.stdin.take() else {
                        return false;
                    };
                    if stdin.write_all(&payload).await.is_err() {
                        return false;
                    }
                    drop(stdin);
                    child.wait().await.is_ok_and(|status| status.success())
                };
                tokio::time::timeout(
                    Duration::from_secs(
                        hook.timeout
                            .as_ref()
                            .map(|span| span.seconds())
                            .unwrap_or(15),
                    ),
                    work,
                )
                .await
                .unwrap_or(false)
            }
            Err(_) => false,
        };
        engine.renewal_alerts.finish(identity, delivered).await?;
        if !delivered {
            tracing::warn!("important-date push was attempted but did not report delivery; command output discarded");
        }
    }
    Ok(())
}

/// L6 supervisor: current milestone replaces the same logical Inbox item;
/// only the configured push effect needs durable once-per-date receipts.
pub(crate) async fn run(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let observer = tokio::spawn(observe_loop(engine.clone(), shutdown.clone()));
    struct StopObserver(tokio::task::AbortHandle);
    impl Drop for StopObserver {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _stop = StopObserver(observer.abort_handle());
    let mut bus = engine.bus.subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        match engine.important_dates(None).await {
            Ok(report) => {
                if let Err(error) = push(&engine, &report).await {
                    tracing::warn!("renewal alert metadata: {error}");
                }
            }
            Err(error) => tracing::warn!("important dates unavailable: {error}"),
        }
        loop {
            tokio::select! {
                _ = tick.tick() => break,
                received = bus.recv() => {
                    match received {
                        Ok(Event::ImportantDatesUpdated { .. } | Event::TaskCreated { .. } | Event::TaskUpdated { .. } | Event::TaskDeleted { .. } | Event::PolicyChanged { .. }) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        _ => {}
                    }
                },
                _ = shutdown.changed() => { if *shutdown.borrow() { observer.abort(); return; } }
            }
        }
    }
}

#[cfg(test)]
mod tests;
