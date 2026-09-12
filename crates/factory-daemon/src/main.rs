//! The daemon. Loads an instance, registers adapters, mounts interfaces, and
//! runs until it is told to stop.

mod access;
mod agents;
mod discovery;
mod engine;
mod interfaces;
mod occupancy;
mod production;
mod scheduler;
mod schedule;
mod site;
mod stores;
mod ui;
mod worktree;

use anyhow::Context;
use clap::{Parser, Subcommand};
use factory_core::adapter::interface::{Interface, InterfaceContext};
use factory_core::adapter::TaskStore;
use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
use factory_core::event::Event;
use factory_plugins::registry::Registry;
use factory_plugins::SqliteStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use engine::Engine;
use interfaces::{HttpInterface, SocketInterface};

#[derive(Parser)]
#[command(name = "factory-daemon", about = "The Factory daemon", version)]
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
        /// Register this path as the first scope.
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
    std::fs::create_dir_all(dir.join("plugins"))?;

    let name = name.unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "factory".into())
    });
    let scope_name = if scope == PathBuf::from(".") {
        name.clone()
    } else {
        scope
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "scope".into())
    };

    let config = Config {
        version: 1,
        instance: Instance {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.clone(),
        },
        daemon: DaemonConfig::default(),
        scopes: vec![Scope {
            name: scope_name,
            path: scope,
            agent: None,
            agents: Vec::new(),
            runtime: None,
            git: None,
            declared: true,
            // A fresh instance has one engine, so the scope it writes names
            // none of its own and takes the instance default.
            task_store: None,
        }],
        roles: Default::default(),
        plugins_dir: None,
    };
    std::fs::write(&config_path, serde_yaml_ng::to_string(&config)?)?;
    println!("wrote {}", config_path.display());
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
    // Every directory under the root becomes a scope here, once, before
    // anything else looks at `factory.config.scopes` -- the engine, the
    // interfaces, the scheduler all read that list as if it always held
    // every scope there could be, and this is what makes that true.
    let discovery_started = std::time::Instant::now();
    discovery::apply(&mut factory);
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

    let engine = Arc::new(Engine::new(
        factory.clone(),
        registry,
        store,
        factory_bin(),
        interface_names,
    ));

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

    // Listen for whatever a runtime pushes on its own, before falling back to
    // the poll below as the floor underneath it.
    engine.watch_runtimes().await;

    let sched = tokio::spawn(scheduler::run(engine.clone(), shutdown_rx.clone()));

    engine.bus.publish(Event::DaemonStarted {
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
    engine.registry.shutdown().await;
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
