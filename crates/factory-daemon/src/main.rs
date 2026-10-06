//! The daemon. Loads an instance, registers adapters, mounts interfaces, and
//! runs until it is told to stop.

mod access;
mod state;
mod artifacts;
mod assignments;
mod agents;
mod backup;
mod bench;
mod budgets;
mod configuration;
mod costs;
mod datasets;
mod dependencies;
mod environments;
mod doctor;
mod discovery;
mod engine;
mod commands;
#[cfg(test)]
mod evidence_store_tests;
mod facts;
mod goals;
mod github_intake;
mod github_outbound;
mod github_deployments;
mod harness_health;
mod host;
mod host_power;
mod intake;
mod interfaces;
mod metrics;
mod occupancy;
mod openshell;
mod provision;
mod renewals;
mod operations;
mod recovery_journal;
mod policies;
mod power;
mod production;
mod quality;
mod resume;
mod roles;
mod scenarios;
mod signposts;
mod scheduler;
mod secrets;
mod service_observations;
mod schedule;
mod site;
mod stores;
mod suggestions;
#[cfg(test)]
mod storage_owner_tests;
#[cfg(test)]
mod provider_owner_tests;
#[cfg(test)]
mod workflow_preview_tests;
mod ui;
mod verification;
mod waiting;
mod worktree;
mod workspace_lifecycle;
mod workflows;

use anyhow::Context;
use clap::{Parser, Subcommand};
use factory_core::adapter::interface::{Interface, InterfaceContext};
use factory_core::adapter::TaskStore;
use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
use factory_core::event::Event;
use factory_plugins::registry::Registry;
use factory_plugins::SqliteStore;
use std::collections::HashMap;
use std::path::Component;
use std::path::PathBuf;
use std::sync::Arc;

use engine::Engine;
use interfaces::{HttpInterface, SocketInterface};

const BUILD_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("FACTORY_GIT_SHA"),
    ")"
);

#[derive(Parser)]
#[command(name = "factory-daemon", about = "The Factory daemon", version = BUILD_VERSION)]
struct Cli {
    /// Instance root. Defaults to the nearest ancestor holding a .factory/.
    #[arg(long, global = true, env = "FACTORY_ROOT")]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon (the default).
    Run,
    /// Write a .factory/ into a directory that has none.
    Init {
        /// Name for the instance. Defaults to the directory's own name.
        #[arg(long)]
        name: Option<String>,
        /// Put the first scope marker at this path below the instance root.
        #[arg(long, default_value = ".")]
        scope: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FACTORY_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    match cli.command.unwrap_or(Command::Run) {
        Command::Init { name, scope } => init(cli.root, name, scope),
        Command::Run => run(cli.root).await,
    }
}

fn init(root: Option<PathBuf>, name: Option<String>, scope: PathBuf) -> anyhow::Result<()> {
    let root = root.unwrap_or(std::env::current_dir()?);
    let dir = root.join(factory_core::config::FACTORY_DIR);
    let config_path = dir.join(factory_core::config::CONFIG_FILE);
    if config_path.exists() {
        anyhow::bail!("{} already exists", config_path.display());
    }

    if scope.is_absolute()
        || scope
            .components()
            .any(|c| match c {
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => true,
                Component::Normal(p) => p == std::ffi::OsStr::new(factory_core::config::FACTORY_DIR),
                _ => false,
            })
    {
        anyhow::bail!("--scope must be below the instance root and outside .factory");
    }
    let root_is_scope = scope.as_os_str().is_empty() || scope == std::path::Path::new(".");
    let nested_scope_config = (!root_is_scope).then(|| {
        root.join(&scope)
            .join(factory_core::config::FACTORY_DIR)
            .join(factory_core::config::CONFIG_FILE)
    });
    if let Some(path) = &nested_scope_config {
        if path.exists() {
            anyhow::bail!("{} already exists", path.display());
        }
    }
    std::fs::create_dir_all(dir.join("plugins"))?;

    let name = name.unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "factory".into())
    });
    let scope_name = if root_is_scope {
        name.clone()
    } else {
        scope
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "scope".into())
    };

    let first_scope = Scope {
        id: uuid::Uuid::new_v4().to_string(),
        name: scope_name,
        path: PathBuf::new(),
        agent: None,
        agents: Vec::new(),
        runtime: None,
        git: None,
        task_store: None,
        max_sessions: None,
        roles: Default::default(),
        dashboard: None,
        policies: Default::default(),
        quality: Default::default(),
        intake: Default::default(),
        dependencies: Default::default(),
        environments: Vec::new(),
        renewals: Vec::new(),
        metrics: None,
        backup: None,
    };
    let config = Config {
        version: 1,
        instance: Instance {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.clone(),
        },
        daemon: DaemonConfig::default(),
        scope: root_is_scope.then(|| first_scope.clone()),
        scopes: Vec::new(),
        roles: Default::default(),
        dashboard: None,
        policies: Default::default(),
        quality: Default::default(),
        infrastructure: Default::default(),
        secrets: Vec::new(),
        plugins_dir: None,
        renewals: Vec::new(),
        renewals_notify: None,
    };
    std::fs::write(&config_path, serde_yaml_ng::to_string(&config)?)?;
    println!("wrote {}", config_path.display());

    if !root_is_scope {
        #[derive(serde::Serialize)]
        struct ScopeFile<'a> {
            version: u32,
            scope: &'a Scope,
        }

        let scope_config = nested_scope_config.expect("a nested scope has a config path");
        let scope_dir = scope_config.parent().expect("a config path has a parent");
        std::fs::create_dir_all(scope_dir)?;
        std::fs::write(
            &scope_config,
            serde_yaml_ng::to_string(&ScopeFile { version: 1, scope: &first_scope })?,
        )?;
        println!("wrote {}", scope_config.display());
    }
    println!("start it with: factory-daemon --root {} run", root.display());
    Ok(())
}

async fn run(root: Option<PathBuf>) -> anyhow::Result<()> {
    let root = match root {
        Some(r) => r,
        None => Factory::discover(&std::env::current_dir()?)?.ok_or_else(|| {
            anyhow::anyhow!(
                "no .factory/ here or in any parent. Run `factory-daemon init` to make one."
            )
        })?,
    };
    let mut factory = Factory::load(&root)?;
    // Resolve directories that opt in through their own Factory config before
    // the engine, interfaces, or scheduler read the runtime scope list.
    let discovery_started = std::time::Instant::now();
    discovery::apply(&mut factory)?;
    factory.config.validate()?;
    backup::validate_config(&factory)?;
    tracing::info!(
        instance = %factory.config.instance.name,
        root = %factory.root.display(),
        scopes = factory.config.scopes.len(),
        discovery_ms = discovery_started.elapsed().as_millis(),
        "loading instance"
    );

    // Refusing early is kinder than two daemons fighting over one socket.
    let socket = factory.socket_path();
    if socket.exists() && tokio::net::UnixStream::connect(&socket).await.is_ok() {
        anyhow::bail!(
            "another daemon is already listening on {}",
            socket.display()
        );
    }

    let mut registry = Registry::with_builtins();
    registry
        .add_store(Arc::new(SqliteStore::open(&factory.database_path())?), "builtin");
    registry.load_plugins(&factory.plugins_dir()).await;
    for problem in &registry.problems {
        tracing::warn!("plugin not loaded -- {problem}");
    }

    // The instance default is the ledger: whatever engine a scope keeps its
    // tasks in, the runs, the journal and the standing agents stay here.
    let ledger = registry.store(&factory.config.daemon.task_store)?;

    // Looked up again by name on every search, so this is only the check --
    // but it belongs here, where a misspelt provider stops the daemon in
    // front of whoever started it rather than failing every search later.
    registry
        .knowledge(&factory.config.daemon.knowledge_provider)
        .context("daemon.knowledge_provider names a knowledge provider that is not registered")?;

    // A scope that names an engine nobody registered has to be a refusal now,
    // in front of whoever started the daemon. Falling back to the default
    // would put that project's tasks in the wrong database and say nothing.
    let mut by_scope: HashMap<String, Arc<dyn TaskStore>> = HashMap::new();
    for scope in &factory.config.scopes {
        let Some(name) = &scope.task_store else { continue };
        if name == &factory.config.daemon.task_store {
            continue;
        }
        let picked = registry
            .store(name)
            .with_context(|| format!("scope {:?} names a task store that is not registered", scope.name))?;
        by_scope.insert(scope.name.clone(), picked);
    }

    let store: Arc<dyn TaskStore> = if by_scope.is_empty() {
        ledger
    } else {
        let scoped = stores::ScopedStores::new(ledger, by_scope);
        tracing::info!("{}", scoped.description());
        Arc::new(scoped)
    };

    let interface_names: Vec<String> = factory
        .config
        .daemon
        .interfaces
        .iter()
        .map(|i| i.kind.clone())
        .collect();

    let workflow_store = workflows::WorkflowStore::open(&factory.database_path())?;
    let bench_store = bench::BenchStore::open(&factory.database_path())?;
    let policy_store = policies::PolicyStore::open(&factory.database_path())?;
    let suggestion_store = suggestions::SuggestionStore::open(&factory.database_path())?;
    let run_evidence_store =
        factory_process::evidence_store::RunEvidenceStore::open(&factory.database_path())?;
    let goals_store = goals::GoalsStore::open(&factory.database_path())?;
    let backup_store = backup::BackupStore::open(&factory.database_path())?;
    let environment_store = environments::EnvironmentStore::open(&factory.database_path())?;
    let infrastructure_expiries = renewals::store::InfrastructureExpiryStore::open(&factory.database_path())?;
    let credential_expiries = renewals::store::CredentialExpiryStore::open(&factory.database_path())?;
    let renewal_alerts = renewals::store::AlertStore::open(&factory.database_path())?;
    let engine = Arc::new(
        Engine::new(factory.clone(), registry, store, factory_bin(), interface_names)
            .with_workflow_store(workflow_store)
            .with_bench_store(bench_store)
            .with_policy_store(policy_store)
            .with_suggestion_store(suggestion_store)
            .with_run_evidence_store(run_evidence_store)
            .with_goals_store(goals_store)
            .with_backup_store(backup_store)
            .with_environment_store(environment_store)
            .with_renewal_stores(infrastructure_expiries, credential_expiries, renewal_alerts),
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let mut mounted = Vec::new();

    for cfg in &factory.config.daemon.interfaces {
        let ctx = InterfaceContext {
            config: cfg.clone(),
            root: factory.root.clone(),
            factory_dir: factory.factory_dir(),
        };
        let engine = engine.clone();
        let rx = shutdown_rx.clone();
        let kind = cfg.kind.clone();

        // Mount before announcing. An interface that cannot bind has to be an
        // error now, in front of whoever started the daemon -- not a line in
        // the shutdown log an hour later.
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let name = kind.clone();
        let handle = match kind.as_str() {
            "cli" => {
                let iface = Arc::new(SocketInterface::new(factory.socket_path()));
                tokio::spawn(async move { watch_interface(name, iface.serve(engine, ctx, rx), ready_tx).await })
            }
            "http" => {
                let iface = Arc::new(HttpInterface);
                tokio::spawn(async move { watch_interface(name, iface.serve(engine, ctx, rx), ready_tx).await })
            }
            other => {
                tracing::warn!("no interface adapter named {other:?}; skipping");
                continue;
            }
        };
        // `serve` only returns when it stops, so "still running after a moment"
        // is what success looks like here.
        match tokio::time::timeout(std::time::Duration::from_millis(400), ready_rx).await {
            Ok(Ok(Err(e))) => anyhow::bail!("the {kind} interface could not start: {e}"),
            Ok(Ok(Ok(()))) => anyhow::bail!("the {kind} interface stopped immediately"),
            _ => {}
        }
        mounted.push((kind, handle));
    }

    if mounted.is_empty() {
        anyhow::bail!("no interfaces mounted; the daemon would be unreachable");
    }

    // Line up declared standing agents with whatever is still running before
    // anything else can look at them.
    engine.reconcile_agents().await;

    // And any OpenShell sandbox a run left behind (`#218`), once the store
    // can say which runs are still active.
    engine.reconcile_openshell().await;

    // Listen for whatever a runtime pushes on its own, before falling back to
    // the poll below as the floor underneath it.
    engine.watch_runtimes().await;

    // Tasks stored as `failed` before `#122` go to blocked before anything
    // reads them -- the scheduler, a workflow's node overlay, the board.
    engine.migrate_failed_tasks().await;

    // Reconcile persisted workflow decisions only after runtimes and standing
    // agents are available. Recovery reuses task ids recorded before a crash.
    engine.recover_workflows().await;
    for finding in recovery_journal::import(&engine).await { tracing::warn!("recovery journal: {finding}"); }
    engine.recover_workspaces().await;
    // The one place a bench attempt's gate actually runs -- started before
    // recovery below, so anything it enqueues has a consumer immediately.
    engine.spawn_bench_judge();
    // `#118`'s verifier, and the runs a restart caught mid-verification.
    engine.spawn_verifier();
    engine.recover_verifications().await;
    // `#179`'s admission queue: a run ending wakes whatever is waiting on
    // its (scope, agent) right away, rather than only on the next tick.
    engine.spawn_capacity_release_worker();
    // The same, for bench runs still `running` when the daemon last stopped.
    engine.recover_bench_runs().await;
    engine.reconcile_run_deployments().await;

    let sched = tokio::spawn(scheduler::run(engine.clone(), shutdown_rx.clone()));
    let benchmarks = tokio::spawn(bench::run(engine.clone(), shutdown_rx.clone()));
    // GitHub receipt is independent of dispatch: network or authentication
    // trouble must never hold up the scheduler.
    let github_intake = tokio::spawn(github_intake::run(engine.clone(), shutdown_rx.clone()));
    // Its own loop rather than a slot in the scheduler's: a backup can take
    // minutes, and nothing the scheduler fires should wait behind one.
    let backups = tokio::spawn(backup::run(engine.clone(), shutdown_rx.clone()));
    // The health checks of every declared environment (`#185`), on their
    // own loop for the same reason: a check can take its whole timeout.
    let health = tokio::spawn(environments::run(engine.clone(), shutdown_rx.clone()));
    // `sandbox: openshell` prerequisites (`#234`): gateway, image, profiles,
    // providers, smoke. Its own loop, started after the reconcile above,
    // and never awaited: a build takes minutes and a gateway start seconds,
    // and neither may hold up startup or any dispatch but its own agent's.
    let provisioning = tokio::spawn(provision::run(engine.clone(), shutdown_rx.clone()));
    let recovery_receipts = tokio::spawn(recovery_journal::run(engine.clone(), shutdown_rx.clone()));
    let important_dates = tokio::spawn(renewals::run(engine.clone(), shutdown_rx.clone()));

    engine.shared.bus.publish(Event::DaemonStarted {
        at: chrono::Utc::now(),
        instance: factory.config.instance.name.clone(),
    });
    tracing::info!(
        interfaces = %mounted.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", "),
        "factory is up"
    );

    // A daemon is as likely to be stopped by a service manager as by a person
    // at a terminal, and a SIGTERM that skipped the shutdown path would leave
    // its socket behind.
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate())?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => tracing::info!("interrupted"),
            _ = term.recv() => tracing::info!("terminated"),
        }
    }
    tracing::info!("shutting down");
    let _ = shutdown_tx.send(true);
    // Before the loops stop: nothing will be watching the agents from here, so
    // say so in the record rather than leaving a span open across the gap.
    engine.close_liveness().await;

    for (kind, handle) in mounted {
        if tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .is_err()
        {
            tracing::warn!("{kind} interface did not stop in time");
        }
    }
    sched.abort();
    benchmarks.abort();
    github_intake.abort();
    backups.abort();
    health.abort();
    provisioning.abort();
    recovery_receipts.abort();
    important_dates.abort();
    engine.shared.registry.shutdown().await;
    Ok(())
}

/// Run one interface, reporting how it ended the moment it ends.
async fn watch_interface(
    name: String,
    serving: impl std::future::Future<Output = factory_core::error::Result<()>>,
    ready: tokio::sync::oneshot::Sender<factory_core::error::Result<()>>,
) {
    let outcome = serving.await;
    match &outcome {
        Err(e) => tracing::error!("{name} interface: {e}"),
        Ok(()) => tracing::info!("{name} interface stopped"),
    }
    let _ = ready.send(outcome);
}

/// Where the agent's callback binary lives. Next to this one is the answer that
/// survives a `cargo build`; PATH is not trustworthy inside an agent's pane.
fn factory_bin() -> PathBuf {
    if let Ok(p) = std::env::var("FACTORY_BIN") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("factory");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("factory")
}
