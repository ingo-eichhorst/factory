//! Operations of running systems (`#185`): the Operations tab's report,
//! deployments recorded as they start and finish, their post-deploy
//! verification, and the loop that runs every declared health check.
//!
//! Every decision -- status, incidents, availability, error budget, DORA --
//! is pure and lives in `factory_core::environments`. This module gathers
//! the records those decisions read (`store.rs`), runs checks
//! (`checks.rs`), and says who did what.
//!
//! **Never takes the daemon down.** A check runs in a task of its own,
//! bounded by its timeout; one that hangs, fails to start or panics is a
//! failed sample. A store that will not write is a line in the log, and
//! the loop carries on.

pub mod checks;
pub mod store;
mod promotion;
mod recovery;
mod releases;

pub use store::EnvironmentStore;

use chrono::{DateTime, Duration, Utc};
use factory_core::config::Factory;
use factory_core::environments::{
    self as env, Actor, ActorKind, CheckDecl, DeployFinish, DeployStart, DeployStatus, DeployVerification,
    Deployment, EnvStatus, EnvironmentDecl, EnvironmentsReport, ReleaseAdd, ReleaseFacts, Sample,
};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use crate::access::Caller;
use crate::engine::Engine;
use store::Finished;

/// How often the loop looks for a check that is due. A check runs at most
/// every `environments::MIN_EVERY_SECONDS`, so this is fine enough.
const TICK: std::time::Duration = std::time::Duration::from_secs(5);

/// How often old samples are pruned.
const PRUNE_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// How long `git show` may take to say when a commit was made.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn operation_targets(run: &factory_core::workflow::WorkflowRun, environment: &str) -> bool {
    run.definition.nodes.iter().any(|node| {
        node.task.labels.get("factory.promotion.target").is_some_and(|target| target == environment)
            || node.task.labels.get(factory_kernel::RECOVERY_ENVIRONMENT_LABEL).is_some_and(|target| target == environment)
    })
}

/// One check the loop can run: which environment, where, and what.
#[derive(Debug, Clone)]
struct Due {
    environment: String,
    url: Option<String>,
    dir: PathBuf,
    check: CheckDecl,
}

/// Every check of every declared, unpaused environment, with the scope
/// directory a `command` check runs in.
fn declared_checks(factory: &Factory) -> Vec<Due> {
    factory
        .config
        .environments()
        .into_iter()
        .filter(|(_, decl)| !decl.paused)
        .flat_map(|(scope, decl)| {
            let dir = factory.scope_path(&scope).unwrap_or_else(|_| factory.root.clone());
            decl.checks
                .iter()
                .map(|check| Due {
                    environment: decl.name.clone(),
                    url: decl.url.clone(),
                    dir: dir.clone(),
                    check: check.clone(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Run `due` in a task of its own, so a panic inside a check is a failed
/// sample and never the caller's problem.
async fn run_isolated(due: Due) -> Sample {
    let name = due.check.display_name();
    let environment = due.environment.clone();
    let handle = tokio::spawn(async move { checks::run(&due.environment, due.url.as_deref(), &due.dir, &due.check).await });
    match handle.await {
        Ok(sample) => sample,
        Err(e) => {
            tracing::warn!(environment, check = name, "health check task failed: {e}");
            Sample {
                environment,
                check: name,
                at: Utc::now(),
                ok: false,
                latency_ms: 0,
                slow: false,
                detail: Some("the check itself failed inside the daemon".into()),
            }
        }
    }
}

/// What the loop remembers between ticks: when each check last started,
/// which are still running, each check's latest answer and each
/// environment's status, so only a change is published.
#[derive(Default)]
struct Checker {
    last_started: HashMap<(String, String), std::time::Instant>,
    in_flight: BTreeSet<(String, String)>,
    latest: HashMap<(String, String), EnvStatus>,
    status: HashMap<String, EnvStatus>,
}

impl Checker {
    /// The checks due at `now`, marked started.
    fn due(&mut self, all: Vec<Due>, now: std::time::Instant) -> Vec<Due> {
        let mut out = Vec::new();
        for due in all {
            let key = (due.environment.clone(), due.check.display_name());
            if self.in_flight.contains(&key) {
                continue;
            }
            let every = std::time::Duration::from_secs(due.check.every_seconds());
            if self.last_started.get(&key).is_some_and(|last| now.duration_since(*last) < every) {
                continue;
            }
            self.last_started.insert(key.clone(), now);
            self.in_flight.insert(key);
            out.push(due);
        }
        out
    }

    /// Take in one answer; the environment's new status if it changed.
    /// The first status after a start is remembered, not announced.
    fn answered(&mut self, sample: &Sample, checks_of_env: &[String]) -> Option<EnvStatus> {
        let key = (sample.environment.clone(), sample.check.clone());
        self.in_flight.remove(&key);
        self.latest.insert(key, sample.status());
        let latest: Vec<Option<EnvStatus>> = checks_of_env
            .iter()
            .map(|c| self.latest.get(&(sample.environment.clone(), c.clone())).copied())
            .collect();
        let now = env::status_of_checks(&latest);
        match self.status.insert(sample.environment.clone(), now) {
            Some(before) if before != now => Some(now),
            _ => None,
        }
    }
}

/// The daemon's health loop. Reads the configuration fresh every tick, so
/// an environment added, changed or paused is picked up without a restart.
pub async fn run(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Sample>();
    let mut checker = Checker::default();
    let mut last_prune: Option<std::time::Instant> = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let factory = engine.factory_snapshot();
                for due in checker.due(declared_checks(&factory), std::time::Instant::now()) {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let _ = tx.send(run_isolated(due).await);
                    });
                }
                if last_prune.is_none_or(|t| t.elapsed() >= PRUNE_EVERY) {
                    last_prune = Some(std::time::Instant::now());
                    let before = Utc::now() - Duration::days(env::SAMPLE_RETENTION_DAYS);
                    match engine.environments.prune_samples(before).await {
                        Ok(0) => {}
                        Ok(n) => tracing::info!(pruned = n, "pruned health samples older than {}d", env::SAMPLE_RETENTION_DAYS),
                        Err(e) => tracing::warn!("could not prune health samples: {e}"),
                    }
                }
            }
            Some(sample) = rx.recv() => {
                engine.take_sample(&mut checker, sample).await;
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { return; }
            }
        }
    }
}

impl Engine {
    /// Record one answer, and publish the environment's status if it moved.
    async fn take_sample(&self, checker: &mut Checker, sample: Sample) {
        if let Err(e) = self.environments.append_sample(sample.clone()).await {
            tracing::warn!(environment = sample.environment, check = sample.check, "could not record a health sample: {e}");
        }
        let factory = self.factory_snapshot();
        let checks: Vec<String> = factory
            .config
            .environments()
            .into_iter()
            .find(|(_, d)| d.name == sample.environment)
            .map(|(_, d)| d.checks.iter().map(CheckDecl::display_name).collect())
            .unwrap_or_default();
        if let Some(status) = checker.answered(&sample, &checks) {
            tracing::info!(environment = sample.environment, ?status, "environment status changed");
            self.bus.publish(Event::EnvironmentStatusChanged {
                environment: sample.environment.clone(),
                status,
                at: sample.at,
            });
        }
    }

    /// The Operations tab's report, narrowed to `scope`'s subtree when one
    /// is named.
    pub(crate) async fn environments_report(&self, scope: Option<String>) -> Result<EnvironmentsReport> {
        self.environment_report(scope, true).await
    }

    /// Metric production reads L1 history only, not pending L4 workflows.
    async fn environment_report(&self, scope: Option<String>, actions: bool) -> Result<EnvironmentsReport> {
        let factory = self.factory_snapshot();
        let now = Utc::now();
        let members: Option<BTreeSet<String>> = match scope.as_deref() {
            None => None,
            Some(name) => {
                let (asked, subtree) = crate::policies::subtree_scopes(&factory, Some(name))?;
                let mut names: BTreeSet<String> = subtree.into_iter().map(|s| s.name).collect();
                if let Some(asked) = asked {
                    names.insert(asked.name);
                }
                Some(names)
            }
        };
        let within = |s: &str| members.as_ref().is_none_or(|m| m.contains(s));
        let declared: Vec<(String, EnvironmentDecl)> =
            factory.config.environments().into_iter().filter(|(s, _)| within(s)).collect();
        let window = declared
            .iter()
            .filter_map(|(_, d)| d.slo.as_ref().map(env::Slo::window_days))
            .max()
            .unwrap_or(env::DEFAULT_WINDOW_DAYS)
            .max(env::DEFAULT_WINDOW_DAYS);
        let names: BTreeSet<&str> = declared.iter().map(|(_, d)| d.name.as_str()).collect();
        let samples: Vec<Sample> = self
            .environments
            .samples_since(now - Duration::days(window))
            .await?
            .into_iter()
            .filter(|s| names.contains(s.environment.as_str()))
            .collect();
        let history = self.environments.deployments().await?;
        let deployments: Vec<Deployment> = history.iter().filter(|d| within(&d.scope)).cloned().collect();
        let added: Vec<(String, ReleaseFacts, DateTime<Utc>)> =
            self.environments.releases_added().await?.into_iter().filter(|(s, _, _)| within(s)).collect();
        let mut report = env::report(&declared, &samples, &deployments, &added, now);
        if actions {
            let pending = self.workflows.active_runs().await?;
            for deployment in &report.deployments {
                if let Ok(plan) = self.deployment_mirror_plan(&deployment.id).await {
                    let receipt = crate::facts::Facts::<factory_kernel::People>::new(self).get::<factory_kernel::DeploymentMirrorFact>(&deployment.id).await?.into_iter().next();
                    report.deployment_mirrors.insert(deployment.id.clone(), env::DeploymentMirrorOffer { plan, receipt });
                }
            }
            report.recovery_journal = Some(crate::facts::Facts::<factory_kernel::People>::new(self).get::<factory_kernel::RecoveryJournalFact>(
                &crate::facts::RecoveryQuery { scopes: members.clone(), limit: 200 }
            ).await?);
            report.recoveries = crate::facts::Facts::<factory_kernel::People>::new(self).get::<factory_kernel::EnvironmentRecoveryFact>(
                &crate::facts::RecoveryQuery { scopes: members.clone(), limit: 200 }
            ).await?;
            for card in &mut report.environments {
                if let Some((scope, environment)) = declared.iter().find(|(_, environment)| environment.name == card.name) {
                    let mut reason = self.recovery_recipe(scope, environment).await.err().map(|error| error.to_string());
                    if pending.iter().any(|run| operation_targets(run, &card.name)) || card.running.is_some() {
                        reason = Some("an environment operation is already pending or running".into());
                    }
                    card.recovery_ready = reason.is_none();
                    card.recovery_reason = reason;
                }
                if let Some((_, source)) = declared.iter().find(|(_, environment)| {
                    environment.name == card.name && environment.promotes_to.is_some()
                }) {
                    let reason = match self.promotion_target(source, card.current.as_ref(), &history).await {
                        Err(error) => Some(error.to_string()),
                        Ok((_, target, _)) if pending.iter().any(|run| operation_targets(run, &target.name)) => {
                            Some("a promotion to this target is already pending".into())
                        }
                        Ok(_) => None,
                    };
                    card.promotion_ready = reason.is_none();
                    card.promotion_reason = reason;
                }
            }
        }
        Ok(report)
    }

    /// The scope a `deploy.start` belongs to: a declared environment's own,
    /// else the one it names, else the caller's, else the root scope's.
    pub(crate) fn deploy_scope(&self, req: &DeployStart, caller: &Caller) -> Result<String> {
        let factory = self.factory_snapshot();
        if let Some((scope, _)) = factory.config.environments().into_iter().find(|(_, d)| d.name == req.environment) {
            if let Some(asked) = &req.scope {
                if asked != &scope {
                    return Err(FactoryError::BadRequest(format!(
                        "environment {:?} is declared by scope {scope:?}, not {asked:?}",
                        req.environment
                    )));
                }
            }
            return Ok(scope);
        }
        if let Some(scope) = req.scope.clone().or_else(|| caller.scope().map(str::to_string)) {
            factory.scope(&scope)?;
            return Ok(scope);
        }
        factory
            .config
            .scope
            .as_ref()
            .map(|s| s.name.clone())
            .ok_or_else(|| FactoryError::BadRequest("name the environment's scope with --scope".into()))
    }

    async fn actor(&self, caller: &Caller) -> Actor {
        match caller {
            Caller::Owner => Actor { kind: ActorKind::Person, name: "owner".into(), run_id: None, task_id: None },
            Caller::Agent { name, run_id: Some(run_id), .. } => {
                let task_id = self.store.get_run(run_id).await.ok().flatten().map(|r| r.task_id);
                Actor { kind: ActorKind::Run, name: name.clone(), run_id: Some(run_id.clone()), task_id }
            }
            Caller::Agent { name, run_id: None, .. } => {
                Actor { kind: ActorKind::Agent, name: name.clone(), run_id: None, task_id: None }
            }
        }
    }

    /// Record a deployment starting. One still running on the same
    /// environment was never finished; it is ended as failed, naming the
    /// one that took its place, rather than left running forever.
    pub(crate) async fn deploy_start(&self, caller: &Caller, mut req: DeployStart) -> Result<Deployment> {
        let _edit = self.deployment_edit.lock().await;
        if !env::is_env_name(&req.environment) {
            return Err(FactoryError::BadRequest(format!(
                "{:?} is not an environment name: lowercase letters, digits, `-` and `_`",
                req.environment
            )));
        }
        req.release.commit = req.release.commit.trim().to_string();
        if req.release.commit.is_empty() {
            return Err(FactoryError::BadRequest("a deployment names the commit it releases".into()));
        }
        let scope = self.deploy_scope(&req, caller)?;
        if req.strict_verification && !self.factory_snapshot().config.environments().iter()
            .any(|(_, environment)| environment.name == req.environment && !environment.paused && !environment.checks.is_empty()) {
            return Err(FactoryError::BadRequest("strict verification needs declared, unpaused environment checks".into()));
        }
        if req.release.committed_at.is_none() {
            let dir = self.factory_snapshot().scope_path(&scope).ok();
            req.release.committed_at = match dir {
                Some(dir) => committed_at(dir, req.release.commit.clone()).await,
                None => None,
            };
        }
        let history = self.environments.deployments().await?;
        let on_env: Vec<&Deployment> = history.iter().filter(|d| d.environment == req.environment).collect();
        let previous_commit =
            on_env.iter().find(|d| d.status == DeployStatus::Succeeded).map(|d| d.release.commit.clone());
        self.enrich_release(&scope, &mut req.release, previous_commit.as_deref()).await?;
        let actor = self.actor(caller).await;
        let now = Utc::now();
        let deployment = Deployment {
            id: uuid::Uuid::new_v4().to_string(),
            scope,
            environment: req.environment.clone(),
            release: req.release,
            manual: actor.kind == ActorKind::Person && req.via.is_none(),
            strict_verification: req.strict_verification,
            actor,
            via: req.via,
            started_at: req.started_at.unwrap_or(now),
            finished_at: None,
            status: DeployStatus::Running,
            reason: None,
            previous_commit,
            verification: None,
        };
        for stale in on_env.iter().filter(|d| d.status == DeployStatus::Running) {
            let finished = Finished {
                status: DeployStatus::Failed,
                at: now,
                reason: Some(format!("never finished; superseded by deployment {}", deployment.id)),
                verification: None,
            };
            self.environments.finished(&stale.id, finished).await?;
            if let Some(ended) = self.environments.deployment(&stale.id).await? {
                self.bus.publish(Event::DeploymentUpdated { deployment: Box::new(ended) });
            }
        }
        self.environments.started(&deployment).await?;
        self.bus.publish(Event::DeploymentUpdated { deployment: Box::new(deployment.clone()) });
        Ok(deployment)
    }

    /// Record how a deployment ended. A success runs the environment's own
    /// checks first and is only recorded as one when they pass.
    pub(crate) async fn deploy_finish(&self, req: DeployFinish) -> Result<Deployment> {
        let _edit = self.deployment_edit.lock().await;
        let Some(deployment) = self.environments.deployment(&req.id).await? else {
            return Err(FactoryError::BadRequest(format!("no deployment {}", req.id)));
        };
        if deployment.status != DeployStatus::Running {
            return Err(FactoryError::BadRequest(format!(
                "deployment {} already finished: {}",
                req.id,
                deployment.status.as_str()
            )));
        }
        if req.status == DeployStatus::Running {
            return Err(FactoryError::BadRequest("finish a deployment as succeeded, failed or rolled_back".into()));
        }
        let mut finished = Finished { status: req.status, at: Utc::now(), reason: req.reason, verification: None };
        if req.status == DeployStatus::Succeeded && req.verify {
            if let Some(verification) = self.verify_environment(&deployment.environment).await {
                if !verification.ok {
                    let failing: Vec<String> = verification
                        .checks
                        .iter()
                        .filter(|s| !s.ok)
                        .map(|s| format!("{}: {}", s.check, s.detail.as_deref().unwrap_or("failed")))
                        .collect();
                    finished.status = DeployStatus::Failed;
                    finished.reason = Some(format!("post-deploy verification failed: {}", failing.join("; ")));
                }
                finished.verification = Some(verification);
            }
        }
        if req.status == DeployStatus::Succeeded && deployment.strict_verification && finished.verification.is_none() {
            finished.status = DeployStatus::Failed;
            finished.reason = Some("required post-deploy verification was skipped, paused or no longer declared".into());
        }
        // Verification is part of the attempt; its time belongs in the
        // duration and the instant the release became verified/running.
        finished.at = Utc::now();
        self.environments.finished(&req.id, finished).await?;
        let ended = self
            .environments
            .deployment(&req.id)
            .await?
            .ok_or_else(|| FactoryError::BadRequest(format!("no deployment {}", req.id)))?;
        self.bus.publish(Event::DeploymentUpdated { deployment: Box::new(ended.clone()) });
        Ok(ended)
    }

    /// Run every check of a declared, unpaused environment once, now, and
    /// keep the answers as samples too. `None` when it has none to run.
    async fn verify_environment(&self, environment: &str) -> Option<DeployVerification> {
        let factory = self.factory_snapshot();
        let due: Vec<Due> = declared_checks(&factory).into_iter().filter(|d| d.environment == environment).collect();
        if due.is_empty() {
            return None;
        }
        let expected = due.len();
        let handles: Vec<_> = due.into_iter().map(|d| tokio::spawn(run_isolated(d))).collect();
        let mut samples = Vec::new();
        for h in handles {
            if let Ok(sample) = h.await {
                samples.push(sample);
            }
        }
        for s in &samples {
            if let Err(e) = self.environments.append_sample(s.clone()).await {
                tracing::warn!("could not record a verification sample: {e}");
            }
        }
        Some(DeployVerification { at: Utc::now(), ok: samples.len() == expected && samples.iter().all(|s| s.ok), checks: samples })
    }

    pub(crate) async fn release_add(&self, req: ReleaseAdd) -> Result<(String, ReleaseFacts)> {
        let factory = self.factory_snapshot();
        factory.scope(&req.scope)?;
        let mut release = req.release;
        release.commit = release.commit.trim().to_string();
        if release.commit.is_empty() {
            return Err(FactoryError::BadRequest("a release names its commit".into()));
        }
        if release.committed_at.is_none() {
            if let Ok(dir) = factory.scope_path(&req.scope) {
                release.committed_at = committed_at(dir, release.commit.clone()).await;
            }
        }
        let added = self.environments.releases_added().await?;
        let deployments = self.environments.deployments().await?;
        let previous = added.iter().filter(|(scope, facts, _)| scope == &req.scope && facts.commit != release.commit)
            .map(|(_, facts, at)| (facts.commit.clone(), *at))
            .chain(deployments.iter().filter(|deployment| deployment.scope == req.scope && deployment.release.commit != release.commit)
                .map(|deployment| (deployment.release.commit.clone(), deployment.started_at)))
            .max_by_key(|(_, at)| *at).map(|(commit, _)| commit);
        self.enrich_release(&req.scope, &mut release, previous.as_deref()).await?;
        self.environments.release_added(&req.scope, &release, Utc::now()).await?;
        Ok((req.scope, release))
    }

    /// Per environment, what the metrics read: its SLA figures and DORA
    /// keys, from the same report the tab draws.
    pub(crate) async fn environment_cards(&self, scope: Option<&str>) -> Result<BTreeMap<String, env::EnvironmentCard>> {
        Ok(self
            .environment_report(scope.map(str::to_string), false)
            .await?
            .environments
            .into_iter()
            .map(|c| (c.name.clone(), c))
            .collect())
    }
}

/// When `commit` was made, asked of the scope's repository -- where lead
/// time for changes starts. `None` for anything git cannot answer in time.
async fn committed_at(dir: PathBuf, commit: String) -> Option<DateTime<Utc>> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(&dir)
        .args(["show", "-s", "--format=%cI", &commit, "--"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(GIT_TIMEOUT, cmd.output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    DateTime::parse_from_rfc3339(text.lines().next()?.trim()).ok().map(|t| t.with_timezone(&Utc))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use factory_core::environments::{EnvStatus, INCIDENT_THRESHOLD};
    use factory_core::role::Role;

    /// A throwaway instance with a real database file and one scope that
    /// declares `environments`.
    pub(crate) fn engine_with(environments: &str) -> (Arc<Engine>, PathBuf) {
        use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration};
        use factory_plugins::{Registry, SqliteStore};
        let root = std::env::temp_dir().join(format!("factory-environments-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory")).unwrap();
        let database = root.join(".factory/factory.sqlite");
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::open(&database).unwrap());
        let mut company: factory_core::config::Scope =
            serde_yaml_ng::from_str(&format!("id: company-id\nname: company\nenvironments:\n{environments}")).unwrap();
        company.path = PathBuf::from(".");
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(company.clone()),
            scopes: vec![company],
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        config.validate_environments().unwrap();
        let factory = Factory { root: root.clone(), config };
        let engine = Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new())
            .with_environment_store(EnvironmentStore::open(&database).unwrap());
        (Arc::new(engine), root)
    }

    fn start(environment: &str, commit: &str) -> DeployStart {
        DeployStart {
            environment: environment.into(),
            strict_verification: false,
            scope: None,
            release: ReleaseFacts { commit: commit.into(), ..Default::default() },
            via: None,
            started_at: None,
        }
    }

    fn finish(id: &str, status: DeployStatus) -> DeployFinish {
        DeployFinish { id: id.into(), status, reason: None, verify: true }
    }

    #[tokio::test]
    async fn a_deployment_is_recorded_verified_and_survives_a_new_store() {
        let (engine, root) = engine_with(
            "  - name: staging\n    tier: staging\n    checks: [{ kind: command, command: 'true', name: alive }]\n",
        );
        let mut events = engine.bus.subscribe();
        let d = engine.deploy_start(&Caller::Owner, start("staging", "abc123")).await.unwrap();
        assert_eq!(d.status, DeployStatus::Running);
        assert!(d.manual, "the owner with nothing in between is a deploy by hand");
        assert_eq!(d.scope, "company");
        assert!(matches!(events.recv().await.unwrap(), Event::DeploymentUpdated { .. }));

        let done = engine.deploy_finish(finish(&d.id, DeployStatus::Succeeded)).await.unwrap();
        assert_eq!(done.status, DeployStatus::Succeeded);
        let v = done.verification.as_ref().unwrap();
        assert!(v.ok);
        assert_eq!(v.checks[0].check, "alive");
        assert!(done.duration_seconds().is_some());
        let again = engine.deploy_finish(finish(&d.id, DeployStatus::Failed)).await.unwrap_err();
        assert!(again.to_string().contains("already finished"), "{again}");

        // A new store over the same file: the history is the database's.
        let reopened = EnvironmentStore::open(&root.join(".factory/factory.sqlite")).unwrap();
        let history = reopened.deployments().await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, DeployStatus::Succeeded);

        let second = engine.deploy_start(&Caller::Owner, start("staging", "def456")).await.unwrap();
        assert_eq!(second.previous_commit.as_deref(), Some("abc123"), "what a rollback would target");
        let report = engine.environments_report(None).await.unwrap();
        let card = report.environments.iter().find(|c| c.name == "staging").unwrap();
        assert_eq!(card.current.as_ref().unwrap().release.commit, "abc123");
        assert_eq!(card.running.as_ref().unwrap().release.commit, "def456");
        assert!(card.uptime_24h.is_some(), "the verification's answer is a sample too");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn startup_reconciliation_fails_an_unfinished_run_deploy_but_keeps_manual_deploys_running() {
        use factory_core::{NewRun, Trigger};
        let (engine, root) = engine_with("  - name: prod\n  - name: manual\n");
        let task = factory_core::adapter::store::task_from_new(
            factory_core::task::NewTask { title: "release".into(), ..Default::default() },
            "company".into(), "shell".into(), "shell".into(),
        );
        engine.store.create(&task).await.unwrap();
        let run = engine.store.create_run(&NewRun {
            task_id: task.id, trigger: Trigger::Manual, agent: "shell".into(), adapter: "shell".into(),
            runtime: "herdr".into(), token: "run-test-token".into(), queued_at: None, scheduled_for: None,
        }).await.unwrap();
        let actor = Caller::Agent { scope: "company".into(), name: "shell".into(), role: Role::foreman(), run_id: Some(run.id.clone()) };
        let recorded = engine.deploy_start(&actor, start("prod", "abc")).await.unwrap();
        let manual = engine.deploy_start(&Caller::Owner, start("manual", "abc")).await.unwrap();
        engine.reconcile_run_deployments().await;
        assert_eq!(engine.environments.deployment(&recorded.id).await.unwrap().unwrap().status, DeployStatus::Running);
        engine.store.update_run(&run.id, &factory_core::run::RunPatch {
            status: Some(factory_core::RunStatus::Cancelled), ..Default::default()
        }).await.unwrap();
        engine.reconcile_run_deployments().await;
        let failed = engine.environments.deployment(&recorded.id).await.unwrap().unwrap();
        assert_eq!(failed.status, DeployStatus::Failed);
        assert!(failed.reason.unwrap().contains("without a deploy finish receipt"));
        assert_eq!(engine.environments.deployment(&manual.id).await.unwrap().unwrap().status, DeployStatus::Running);
        engine.reconcile_run_deployments().await;
        assert_eq!(engine.environments.deployments().await.unwrap().len(), 2);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn strict_verification_cannot_be_skipped_or_removed_during_a_deploy() {
        let (engine, root) = engine_with("  - name: prod\n    checks: [{ kind: command, command: 'true', name: api }]\n");
        let request = || DeployStart { strict_verification: true, ..start("prod", "abc") };
        let first = engine.deploy_start(&Caller::Owner, request()).await.unwrap();
        let skipped = engine.deploy_finish(DeployFinish { verify: false, ..finish(&first.id, DeployStatus::Succeeded) }).await.unwrap();
        assert_eq!(skipped.status, DeployStatus::Failed);
        assert!(skipped.reason.unwrap().contains("required post-deploy verification"));
        let second = engine.deploy_start(&Caller::Owner, request()).await.unwrap();
        let reopened = EnvironmentStore::open(&root.join(".factory/factory.sqlite")).unwrap();
        assert!(reopened.deployment(&second.id).await.unwrap().unwrap().strict_verification);
        let mut factory = engine.factory_snapshot();
        factory.config.scopes[0].environments[0].paused = true;
        factory.config.scope = None;
        let paused = Engine::new(factory, factory_plugins::Registry::with_builtins(), engine.store.clone(), PathBuf::from("factory"), Vec::new())
            .with_environment_store(reopened);
        let ended = paused.deploy_finish(finish(&second.id, DeployStatus::Succeeded)).await.unwrap();
        assert_eq!(ended.status, DeployStatus::Failed);
        assert!(ended.verification.is_none());
        assert!(paused.deploy_start(&Caller::Owner, request()).await.is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_success_whose_checks_fail_is_recorded_as_failed_with_the_reason() {
        let (engine, root) = engine_with(
            "  - name: prod\n    tier: production\n    checks: [{ kind: command, command: 'echo down >&2; exit 1', name: api }]\n",
        );
        let d = engine.deploy_start(&Caller::Owner, start("prod", "abc")).await.unwrap();
        let done = engine.deploy_finish(finish(&d.id, DeployStatus::Succeeded)).await.unwrap();
        assert_eq!(done.status, DeployStatus::Failed);
        assert!(done.reason.as_deref().unwrap().contains("api: exit 1: down"), "{:?}", done.reason);
        assert!(!done.verification.unwrap().ok);

        // Skipping verification is said, not hidden: no verification on it.
        let d = engine.deploy_start(&Caller::Owner, start("prod", "abd")).await.unwrap();
        let skipped = engine
            .deploy_finish(DeployFinish { verify: false, ..finish(&d.id, DeployStatus::Succeeded) })
            .await
            .unwrap();
        assert_eq!(skipped.status, DeployStatus::Succeeded);
        assert!(skipped.verification.is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_undeclared_environment_is_recorded_and_a_stale_running_one_is_ended() {
        let (engine, root) = engine_with("  - name: prod\n");
        let agent = Caller::Agent { scope: "company".into(), name: "releaser".into(), role: Role::foreman(), run_id: None };
        let first = engine.deploy_start(&agent, start("review13", "aaa")).await.unwrap();
        assert_eq!(first.scope, "company", "the caller's own scope");
        assert_eq!(first.actor.kind, ActorKind::Agent);
        assert!(!first.manual);
        let second = engine
            .deploy_start(&agent, DeployStart { via: Some("release.sh".into()), ..start("review13", "bbb") })
            .await
            .unwrap();
        let first = engine.environments.deployment(&first.id).await.unwrap().unwrap();
        assert_eq!(first.status, DeployStatus::Failed);
        assert!(first.reason.unwrap().contains(&second.id));
        let report = engine.environments_report(None).await.unwrap();
        let review = report.environments.iter().find(|c| c.name == "review13").unwrap();
        assert!(!review.declared);

        let bad = engine
            .deploy_start(&Caller::Owner, DeployStart { scope: Some("elsewhere".into()), ..start("prod", "x") })
            .await
            .unwrap_err();
        assert!(bad.to_string().contains("declared by scope"), "{bad}");
        assert!(engine.deploy_start(&Caller::Owner, start("Prod!", "x")).await.is_err());
        std::fs::remove_dir_all(root).ok();
    }

    /// A deployment recorded from inside a task run -- the `shell` agent's
    /// run running `factory deploy start` with its own token -- names that
    /// run and its task as the actor, and is not a deploy by hand.
    #[tokio::test]
    async fn a_deployment_recorded_by_a_run_names_the_run_and_its_task() {
        use factory_core::task::NewTask;
        use factory_core::{NewRun, Trigger};
        let (engine, root) = engine_with("  - name: prod\n");
        let task = factory_core::adapter::store::task_from_new(
            NewTask { title: "deploy prod".into(), ..Default::default() },
            "company".into(),
            "shell".into(),
            "shell".into(),
        );
        let task = engine.store.create(&task).await.unwrap();
        let run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "shell".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        let caller = Caller::Agent { scope: "company".into(), name: "shell".into(), role: Role::foreman(), run_id: Some(run.id.clone()) };
        let d = engine.deploy_start(&caller, start("prod", "abc")).await.unwrap();
        assert_eq!(d.actor.kind, ActorKind::Run);
        assert_eq!(d.actor.run_id.as_deref(), Some(run.id.as_str()));
        assert_eq!(d.actor.task_id.as_deref(), Some(task.id.as_str()));
        assert!(!d.manual);
        std::fs::remove_dir_all(root).ok();
    }

    /// The loop's own bookkeeping: a check is due once per `every`, never
    /// twice while one is running, and only a status change is announced.
    #[tokio::test]
    async fn checks_fail_open_an_incident_and_recover_through_the_loops_bookkeeping() {
        let (engine, root) = engine_with(
            "  - name: prod\n    tier: production\n    slo: { availability: 99% }\n    \
             checks: [{ kind: command, command: 'exit 1', name: api, every: 60s, timeout: 5s }]\n",
        );
        let mut events = engine.bus.subscribe();
        let mut checker = Checker::default();
        let t0 = std::time::Instant::now();
        let factory = engine.factory_snapshot();
        let due = checker.due(declared_checks(&factory), t0);
        assert_eq!(due.len(), 1);
        assert!(checker.due(declared_checks(&factory), t0).is_empty(), "in flight");

        // Whole milliseconds: what the store keeps.
        let at = chrono::SubsecRound::trunc_subsecs(Utc::now() - Duration::minutes(10), 3);
        let sample = |i: i64, ok: bool| Sample {
            environment: "prod".into(),
            check: "api".into(),
            at: at + Duration::minutes(i),
            ok,
            latency_ms: 1,
            slow: false,
            detail: None,
        };
        engine.take_sample(&mut checker, sample(0, true)).await;
        assert!(checker.due(declared_checks(&factory), t0 + std::time::Duration::from_secs(30)).is_empty(), "not yet");
        assert_eq!(checker.due(declared_checks(&factory), t0 + std::time::Duration::from_secs(61)).len(), 1);
        for i in 1..=INCIDENT_THRESHOLD as i64 {
            engine.take_sample(&mut checker, sample(i, false)).await;
        }
        match events.recv().await.unwrap() {
            Event::EnvironmentStatusChanged { environment, status, .. } => {
                assert_eq!((environment.as_str(), status), ("prod", EnvStatus::Down));
            }
            other => panic!("{other:?}"),
        }
        let report = engine.environments_report(None).await.unwrap();
        let prod = &report.environments[0];
        assert_eq!(prod.status, EnvStatus::Down);
        assert_eq!(prod.incidents.len(), 1);
        assert_eq!(prod.incidents[0].ended_at, None);
        assert!(prod.error_budget.unwrap() < 0.0, "a third of samples failing overspends a 1% budget");

        engine.take_sample(&mut checker, sample(5, true)).await;
        assert!(matches!(
            events.recv().await.unwrap(),
            Event::EnvironmentStatusChanged { status: EnvStatus::Up, .. }
        ));
        let prod = engine.environments_report(None).await.unwrap().environments.remove(0);
        assert_eq!(prod.incidents[0].ended_at, Some(at + Duration::minutes(5)));
        assert_eq!(prod.dora.time_to_restore_p50, Some(240.0));
        std::fs::remove_dir_all(root).ok();
    }

    /// A check that hangs past its timeout, run the way the loop runs one,
    /// is a failed sample in bounded time -- and the next one still runs.
    #[tokio::test]
    async fn a_hanging_check_is_a_failed_sample_and_the_loop_goes_on() {
        let due = Due {
            environment: "prod".into(),
            url: None,
            dir: std::env::temp_dir(),
            check: serde_yaml_ng::from_str("{ kind: command, command: 'sleep 60', timeout: 1s, every: 5s }").unwrap(),
        };
        let started = std::time::Instant::now();
        let sample = run_isolated(due.clone()).await;
        assert!(!sample.ok);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let next = run_isolated(Due { check: serde_yaml_ng::from_str("{ kind: command, command: 'true' }").unwrap(), ..due }).await;
        assert!(next.ok);
    }

    #[tokio::test]
    async fn slow_status_events_report_and_verification_agree() {
        let (engine, root) = engine_with(
            "  - name: prod\n    slo: { availability: 99% }\n    checks: [{ kind: command, command: 'sleep 0.1', name: api, slow_after_ms: 1 }]\n",
        );
        let mut checker = Checker::default();
        let mut events = engine.bus.subscribe();
        let at = chrono::SubsecRound::trunc_subsecs(Utc::now() - Duration::seconds(5), 3);
        let fast = Sample {
            environment: "prod".into(), check: "api".into(), at,
            ok: true, latency_ms: 1, slow: false, detail: None,
        };
        engine.take_sample(&mut checker, fast.clone()).await;
        let slow = Sample {
            at: at + Duration::seconds(1), latency_ms: 100, slow: true,
            detail: Some("slow: 100ms exceeds 1ms".into()), ..fast.clone()
        };
        engine.take_sample(&mut checker, slow.clone()).await;
        assert!(matches!(events.try_recv().unwrap(), Event::EnvironmentStatusChanged { status: EnvStatus::Degraded, .. }));
        engine.take_sample(&mut checker, Sample { at: at + Duration::seconds(2), ..slow.clone() }).await;
        assert!(events.try_recv().is_err(), "unchanged status publishes nothing");
        let prod = engine.environments_report(None).await.unwrap().environments.remove(0);
        assert_eq!(prod.status, EnvStatus::Degraded);
        assert_eq!(prod.status_since, Some(slow.at));
        assert_eq!(prod.uptime_window, Some(1.0));
        assert_eq!(prod.error_budget, Some(1.0));
        assert!(prod.incidents.is_empty());
        assert_eq!(prod.checks[0].slow_after_ms, Some(1));
        assert!(prod.checks[0].last.as_ref().unwrap().slow);
        assert_eq!(prod.checks[0].strip.iter().map(|b| b.slow).sum::<u32>(), 2);
        engine.take_sample(&mut checker, Sample { at: at + Duration::seconds(3), ..fast }).await;
        assert!(matches!(events.try_recv().unwrap(), Event::EnvironmentStatusChanged { status: EnvStatus::Up, .. }));
        let verified = engine.verify_environment("prod").await.unwrap();
        assert!(verified.ok, "slow is not unavailable");
        assert!(verified.checks[0].slow);
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The SLA figures and DORA keys are registry metrics, computed from
    /// recorded samples and deployments, and listed by default for every
    /// declared environment.
    #[tokio::test]
    async fn availability_error_budget_and_the_dora_keys_are_metrics() {
        let (engine, root) = engine_with(
            "  - name: prod\n    tier: production\n    slo: { availability: 90% }\n    \
             checks: [{ kind: command, command: 'true', name: api }]\n",
        );
        let now = Utc::now();
        for i in 0..10 {
            let sample = Sample {
                environment: "prod".into(),
                check: "api".into(),
                at: now - Duration::minutes(30 - i),
                ok: i != 3,
                latency_ms: 1,
                slow: false,
                detail: None,
            };
            engine.environments.append_sample(sample).await.unwrap();
        }
        let d = engine
            .deploy_start(
                &Caller::Owner,
                DeployStart {
                    release: ReleaseFacts {
                        commit: "abc".into(),
                        committed_at: Some(now - Duration::hours(2)),
                        ..Default::default()
                    },
                    ..start("prod", "abc")
                },
            )
            .await
            .unwrap();
        engine.deploy_finish(finish(&d.id, DeployStatus::Succeeded)).await.unwrap();

        let defaults = engine.default_metric_ids().await;
        for name in factory_core::metrics::ENVIRONMENT_METRICS {
            assert!(defaults.iter().any(|id| id.as_str() == format!("{name}.prod")), "{name}.prod is listed");
        }
        let ids: Vec<factory_core::metrics::MetricId> = ["availability.prod", "error_budget.prod", "deploy_frequency.prod",
            "lead_time_p50.prod", "change_failure_rate.prod", "mttr.prod", "availability.nowhere"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        let m = engine.metrics(&ids, Utc::now()).await.unwrap();
        let value = |id: &str| m.values.iter().find(|v| v.id.as_str() == id).unwrap().clone();
        // Ten samples and the verification's one: ten of eleven healthy.
        assert!((value("availability.prod").value.unwrap() - 10.0 / 11.0).abs() < 1e-9);
        assert!(value("error_budget.prod").value.unwrap() > 0.0);
        assert!((value("deploy_frequency.prod").value.unwrap() - 0.25).abs() < 1e-9, "one in 28 days");
        let lead = value("lead_time_p50.prod").value.unwrap();
        assert!((7190.0..7300.0).contains(&lead), "about two hours: {lead}");
        assert_eq!(value("change_failure_rate.prod").value, Some(0.0));
        assert_eq!(value("mttr.prod").value, None);
        assert_eq!(value("mttr.prod").reason.as_deref(), Some("no incident ended in the window"));
        assert!(value("availability.nowhere").reason.unwrap().contains("no environment"));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_release_is_added_to_the_catalogue_without_a_deployment() {
        let (engine, root) = engine_with("  - name: prod\n");
        let (scope, release) = engine
            .release_add(ReleaseAdd { scope: "company".into(), release: ReleaseFacts { commit: "abc".into(), version: Some("v1".into()), ..Default::default() } })
            .await
            .unwrap();
        assert_eq!(scope, "company");
        assert_eq!(release.version.as_deref(), Some("v1"));
        let report = engine.environments_report(Some("company".into())).await.unwrap();
        assert_eq!(report.releases.len(), 1);
        assert!(report.releases[0].running_on.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn environment_metrics_respect_the_requested_scope_subtree() {
        let (source, root) = engine_with("  - name: prod\n");
        let mut factory = source.factory_snapshot();
        let mut sibling: factory_core::config::Scope =
            serde_yaml_ng::from_str("id: other-id\nname: other\n").unwrap();
        sibling.path = PathBuf::from("projects/other");
        factory.config.scopes.push(sibling);
        let engine = Arc::new(Engine::new(
            factory, factory_plugins::Registry::with_builtins(), source.store.clone(),
            PathBuf::from("factory"), Vec::new(),
        ).with_environment_store(source.environments.clone()));
        let now = Utc::now();
        engine.environments.append_sample(Sample {
            environment: "prod".into(), check: "api".into(), at: now,
            ok: true, latency_ms: 1, slow: false, detail: None,
        }).await.unwrap();
        let ids = ["availability.prod".parse().unwrap()];
        let own = engine.metrics_for(&ids, now, Some("company"), None).await.unwrap();
        assert_eq!(own.values[0].value, Some(1.0));
        let sibling = engine.metrics_for(&ids, now, Some("other"), None).await.unwrap();
        assert_eq!(sibling.values[0].value, None);
        assert!(sibling.values[0].reason.as_ref().unwrap().contains("no environment"));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn concurrent_starts_leave_one_running_attempt_and_finishes_are_single_use() {
        let (engine, root) = engine_with("  - name: prod\n");
        let (first, second) = tokio::join!(
            engine.deploy_start(&Caller::Owner, start("prod", "first")),
            engine.deploy_start(&Caller::Owner, start("prod", "second")),
        );
        first.unwrap();
        second.unwrap();
        let attempts = engine.environments.deployments().await.unwrap();
        assert_eq!(attempts.iter().filter(|d| d.status == DeployStatus::Running).count(), 1);
        assert_eq!(attempts.iter().filter(|d| d.status == DeployStatus::Failed).count(), 1);
        let running = attempts.iter().find(|d| d.status == DeployStatus::Running).unwrap();
        let (a, b) = tokio::join!(
            engine.deploy_finish(finish(&running.id, DeployStatus::Failed)),
            engine.deploy_finish(finish(&running.id, DeployStatus::Succeeded)),
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn deployment_duration_includes_verification() {
        let (engine, root) = engine_with(
            "  - name: prod\n    checks: [{ kind: command, command: 'sleep 1', name: alive }]\n",
        );
        let started = engine.deploy_start(&Caller::Owner, start("prod", "abc")).await.unwrap();
        let ended = engine.deploy_finish(finish(&started.id, DeployStatus::Succeeded)).await.unwrap();
        assert!(ended.finished_at.unwrap() - ended.started_at >= Duration::seconds(1));
        assert!(ended.finished_at.unwrap() >= ended.verification.unwrap().at);
        std::fs::remove_dir_all(root).ok();
    }
}
