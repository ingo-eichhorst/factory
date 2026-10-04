//! `sandbox: openshell`, set up by declaring it (`#234`). For every agent
//! that declares it, this keeps the prerequisites of its runs in place in
//! the background -- at startup, when the declarations change, and every few
//! minutes -- so that a missing one is found when it goes missing, not when
//! Monday's run is due:
//!
//! 1. the OpenShell gateway answers, and is started when it does not;
//! 2. the image: Factory's own, built from the recipe this daemon carries
//!    whenever that recipe or the installed `factory` CLI changes, into a
//!    path named by both, with the previous one kept; or the block's own
//!    `image`, which is only ever checked, never built;
//! 3. the provider profiles Factory ships, imported under this instance's
//!    suffix;
//! 4. every provider declared with a credential source, created when
//!    missing and updated when the source's value changes;
//! 5. a no-model smoke run in a throwaway sandbox after any of that
//!    changed, and only then `ready`.
//!
//! Each agent ends a pass `ready`, `preparing` or `needs <one thing>`, with
//! the command that supplies it. Dispatch reads that (`Engine::sandbox_gate`)
//! and fails closed on anything but `ready`; nothing here ever lets a run
//! start on the host.
//!
//! Three rules hold throughout:
//!
//! - **Values never leave memory except into one child's environment.** A
//!   credential is read from its source, handed to `openshell provider
//!   create/update` as `--credential KEY` with `KEY` set in that child's
//!   environment only, and dropped. Its digest, salted per process, is what
//!   tells a rotation apart; nothing about it is written down.
//! - **The daemon writes only what is this instance's.** Every profile it
//!   imports and every provider it creates or updates carries the
//!   instance's suffix (`openshell::owned_by_instance`); a name without it
//!   is never written, so a throwaway instance on the same gateway cannot
//!   touch the live one's.
//! - **Nothing here can hold up the daemon.** Passes run in a task of their
//!   own, one at a time, off the dispatch and startup paths; a panic in one
//!   is a logged `JoinError`, and the image build is a child process whose
//!   failure is a readiness line.

use crate::engine::Engine;
use chrono::{DateTime, Utc};
use factory_core::config::Sandbox;
use factory_core::openshell::{
    self as os, CredentialSource, GatewayRow, ManagedProvider, OpenshellConfig, ProviderDecl, Readiness,
    ReadinessState, SmokeProvider,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::process::Command;

/// How often the declarations are compared with the last pass's.
const TICK: Duration = Duration::from_secs(15);
/// How often a pass runs when nothing changed.
const FULL_PASS: Duration = Duration::from_secs(5 * 60);
const QUICK: Duration = Duration::from_secs(30);
/// How long the daemon waits for a gateway it started to answer.
const GATEWAY_START: Duration = Duration::from_secs(45);
/// At most one start attempt per gateway in this long.
const START_BACKOFF: Duration = Duration::from_secs(120);
const SMOKE_CREATE: Duration = Duration::from_secs(10 * 60);
const SMOKE_EXEC_SECS: u64 = 120;
/// A failed build is retried after this long, or as soon as its key moves.
const BUILD_RETRY: Duration = Duration::from_secs(60 * 60);
const BUILD_DEADLINE: Duration = Duration::from_secs(60 * 60);
/// How long a dispatch waits for an agent that is still `preparing`. Well
/// inside the ack timeout's default (180s), which also has to cover the
/// sandbox's own create.
pub(crate) const GATE_WAIT: Duration = Duration::from_secs(45);
/// The Homebrew service's launchd label, where the gateway runs on a Mac.
pub(crate) const GATEWAY_SERVICE: &str = "sh.brew.openshell";

/// A credential value. Its `Debug` never prints it, so no `{:?}` of
/// anything holding one -- a log line, a test assertion -- can leak it.
pub(crate) struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl Secret {
    fn expose(&self) -> &str {
        &self.0
    }
}

/// `(scope, agent)`.
pub(crate) type AgentKey = (String, String);

/// What one pass needs to know about one agent.
#[derive(Clone)]
pub(crate) struct Declared {
    pub key: AgentKey,
    pub config: OpenshellConfig,
    pub git: Option<String>,
}

/// What a dispatch resolves from a `ready` agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub image: String,
    pub providers: Vec<String>,
}

/// Paths a test replaces; production uses the defaults.
#[derive(Debug, Clone)]
pub(crate) struct Tools {
    pub launchctl: PathBuf,
    /// Runs in place of the recipe's `build-image.sh rootfs`, with `OUT`
    /// set. Tests use it to make a small image without building one.
    pub build_script: Option<PathBuf>,
    /// Where `build-image.sh` finds Factory's source to cross-compile the
    /// in-image CLI from.
    pub source: Option<PathBuf>,
    /// How long a gateway the daemon started gets to answer.
    pub gateway_wait: Duration,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            launchctl: PathBuf::from("/bin/launchctl"),
            build_script: None,
            source: default_source(),
            gateway_wait: GATEWAY_START,
        }
    }
}

/// The Factory source tree: `FACTORY_OPENSHELL_SOURCE`, else the workspace
/// this daemon was built from, when it is still there.
fn default_source() -> Option<PathBuf> {
    let candidate = std::env::var_os("FACTORY_OPENSHELL_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let candidate = candidate.canonicalize().ok()?;
    candidate.join("crates/factory-cli/Cargo.toml").is_file().then_some(candidate)
}

#[derive(Default)]
struct BuildState {
    /// The key being built now.
    running: Option<(String, DateTime<Utc>)>,
    /// The last failure: key, when, why.
    failed: Option<(String, Instant, String)>,
}

#[derive(Default)]
struct GatewayState {
    row: Option<GatewayRow>,
    last_start: Option<Instant>,
}

/// A digest per gateway (its command prefix) and object name.
type Digests = BTreeMap<(Vec<String>, String), [u8; 32]>;

/// Everything the daemon knows about its sandboxes' prerequisites. Lives on
/// the `Engine`; every field is in memory and rebuilt by the first pass
/// after a restart.
pub(crate) struct Provisioner {
    pub(crate) tools: Tools,
    readiness: Mutex<BTreeMap<AgentKey, Readiness>>,
    gateways: Mutex<BTreeMap<Vec<String>, GatewayState>>,
    /// Per gateway and provider: the salted digest of the value last given.
    given: Mutex<Digests>,
    /// Per gateway and profile: the digest of the shipped YAML imported by
    /// this process.
    profiles: Mutex<Digests>,
    /// Per agent: the fingerprint its last passing smoke ran against.
    smoked: Mutex<BTreeMap<AgentKey, String>>,
    build: Mutex<BuildState>,
    salt: [u8; 16],
    pass: tokio::sync::Mutex<()>,
    /// Asks the loop for a pass now.
    pub(crate) wake: tokio::sync::Notify,
    /// Told after every pass, for a dispatch waiting on one.
    settled: tokio::sync::Notify,
}

impl Default for Provisioner {
    fn default() -> Self {
        Self::new(Tools::default())
    }
}

impl Provisioner {
    pub(crate) fn new(tools: Tools) -> Self {
        Self {
            tools,
            readiness: Mutex::default(),
            gateways: Mutex::default(),
            given: Mutex::default(),
            profiles: Mutex::default(),
            smoked: Mutex::default(),
            build: Mutex::default(),
            salt: *uuid::Uuid::new_v4().as_bytes(),
            pass: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
            settled: tokio::sync::Notify::new(),
        }
    }

    pub(crate) fn readiness(&self, key: &AgentKey) -> Option<Readiness> {
        lock(&self.readiness).get(key).cloned()
    }

    pub(crate) fn all(&self) -> BTreeMap<AgentKey, Readiness> {
        lock(&self.readiness).clone()
    }

    pub(crate) fn gateway_rows(&self) -> Vec<GatewayRow> {
        lock(&self.gateways).values().filter_map(|g| g.row.clone()).collect()
    }

    fn set(&self, key: &AgentKey, mut next: Readiness) {
        let mut all = lock(&self.readiness);
        if let Some(previous) = all.get(key) {
            if previous.state == next.state && previous.thing == next.thing {
                next.since = previous.since;
            }
        }
        all.insert(key.clone(), next);
    }

    fn digest(&self, scope: &[String], name: &str, value: &Secret) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(self.salt);
        for word in scope {
            hash.update(word.as_bytes());
            hash.update([0]);
        }
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(value.expose().as_bytes());
        hash.finalize().into()
    }
}

#[cfg(test)]
impl Provisioner {
    pub(crate) fn set_for_test(&self, key: &AgentKey, readiness: Readiness) {
        self.set(key, readiness);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The loop `main` spawns. A pass at once, then one whenever the
/// declarations change, something asks (`wake`), or `FULL_PASS` passes.
pub(crate) async fn run(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last: Option<(String, Instant)> = None;
    loop {
        let due = {
            let print = fingerprint(&declared(&engine));
            let due = match &last {
                None => true,
                Some((before, at)) => *before != print || at.elapsed() >= FULL_PASS,
            };
            if due {
                last = Some((print, Instant::now()));
            }
            due
        };
        if due {
            pass_isolated(&engine).await;
        }
        tokio::select! {
            _ = ticker.tick() => {}
            _ = engine.provision.wake.notified() => {
                pass_isolated(&engine).await;
                if let Some((_, at)) = last.as_mut() { *at = Instant::now(); }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { return; }
            }
        }
    }
}

/// One pass in a task of its own: a panic in it is logged, never the
/// daemon's end.
pub(crate) async fn pass_isolated(engine: &Arc<Engine>) {
    let engine = engine.clone();
    if let Err(e) = tokio::spawn(async move { engine.provision_pass().await }).await {
        tracing::error!("openshell provisioning pass failed: {e}");
    }
}

/// Every declared `sandbox: openshell` agent, now.
pub(crate) fn declared(engine: &Engine) -> Vec<Declared> {
    let factory = engine.factory_snapshot();
    let mut out = Vec::new();
    for name in factory.scope_names() {
        let Ok(scope) = factory.scope(&name) else { continue };
        for agent in scope.declared_agents() {
            if agent.sandbox != Sandbox::Openshell {
                continue;
            }
            if let Some(config) = agent.openshell.clone() {
                out.push(Declared { key: (scope.name.clone(), agent.name()), config, git: scope.git.clone() });
            }
        }
    }
    out
}

fn fingerprint(declared: &[Declared]) -> String {
    declared
        .iter()
        .map(|d| format!("{:?}{}", d.key, serde_json::to_string(&d.config).unwrap_or_default()))
        .collect()
}

/// `openshell` and `-g <gateway>`, as every command starts.
fn base_for(cli: &str, config: &OpenshellConfig) -> Vec<String> {
    let mut base = vec![cli.to_string()];
    if let Some(gateway) = &config.gateway {
        base.push("-g".into());
        base.push(gateway.clone());
    }
    base
}

/// Why an agent is not ready: the one missing thing and how to supply it,
/// or what is being done about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unready {
    Needs { thing: String, command: Option<String> },
    Preparing(String),
}

fn needs(thing: impl Into<String>, command: Option<String>) -> Unready {
    Unready::Needs { thing: thing.into(), command }
}

impl Engine {
    /// One reconcile of every declared sandboxed agent. One at a time.
    pub(crate) async fn provision_pass(self: &Arc<Self>) {
        let _pass = self.provision.pass.lock().await;
        let agents = declared(self);
        let live: BTreeSet<AgentKey> = agents.iter().map(|d| d.key.clone()).collect();
        lock(&self.provision.readiness).retain(|key, _| live.contains(key));
        lock(&self.provision.smoked).retain(|key, _| live.contains(key));
        // Two agents naming one managed provider differently would update
        // it back and forth; both are told instead.
        let conflicts = conflicting_providers(&agents);
        for agent in &agents {
            let now = Utc::now();
            let mut notes = Vec::new();
            let mut image = None;
            let outcome = match conflicts.get(&agent.key) {
                Some(conflict) => Err(needs(conflict.clone(), None)),
                None => Box::pin(self.provision_agent(agent, &mut notes, &mut image)).await,
            };
            let today = now.date_naive();
            let expiring = agent
                .config
                .providers
                .iter()
                .filter_map(|p| match p {
                    ProviderDecl::Managed(m) => m.expiring(today).map(|days_left| os::Expiring {
                        provider: m.name.clone(),
                        expires: m.expires.unwrap_or(today),
                        days_left,
                        source: m.credential.describe(),
                    }),
                    ProviderDecl::Named(_) => None,
                })
                .collect();
            let (state, thing, command) = match outcome {
                Ok(()) => (ReadinessState::Ready, None, None),
                Err(Unready::Preparing(what)) => (ReadinessState::Preparing, Some(what), None),
                Err(Unready::Needs { thing, command }) => (ReadinessState::Needs, Some(thing), command),
            };
            let previous = self.provision.readiness(&agent.key);
            let changed = previous.as_ref().is_none_or(|p| p.state != state || p.thing != thing);
            self.provision.set(
                &agent.key,
                Readiness { state, thing, command, since: now, checked_at: now, image, notes, expiring },
            );
            if changed {
                let readiness = self.provision.readiness(&agent.key);
                if let Some(r) = readiness {
                    tracing::info!(scope = %agent.key.0, agent = %agent.key.1, state = r.state.as_str(), "openshell readiness: {}", r.reason());
                }
            }
        }
        self.provision.settled.notify_waiters();
    }

    /// Steps 1-5 for one agent. `Ok` is ready; `notes` and `image` are
    /// filled in on the way.
    async fn provision_agent(
        self: &Arc<Self>,
        agent: &Declared,
        notes: &mut Vec<String>,
        image: &mut Option<String>,
    ) -> Result<(), Unready> {
        let config = &agent.config;
        if let Some(problem) = config.problems().into_iter().next() {
            return Err(needs(format!("an openshell: block that works (this one {problem})"), None));
        }
        let cli = crate::openshell::resolve_cli(config.cli.as_deref()).map_err(|_| {
            needs(
                "the openshell CLI",
                Some("curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | OPENSHELL_VERSION=v0.1.2 sh".into()),
            )
        })?;
        let base = base_for(&cli, config);
        self.ensure_gateway(&base).await?;
        let factory = self.factory_snapshot();
        let callback = config.callback_target(self.http_bind(&factory).as_deref()).map_err(|e| {
            needs(format!("a way to report back ({e})"), Some("add an http interface to daemon.interfaces".into()))
        })?;

        // The image.
        let image_path = match &config.image {
            Some(explicit) => {
                check_explicit_image(explicit)?;
                explicit.clone()
            }
            None => {
                let (path, note) = self.ensure_image(&factory.factory_dir())?;
                if let Some(note) = note {
                    notes.push(note);
                }
                path
            }
        };
        *image = Some(image_path.clone());

        // Profiles and providers.
        let suffix = os::instance_suffix(&factory.config.instance.id);
        let listed = list_providers(&base).await.map_err(|e| needs(format!("the gateway's provider list ({e})"), None))?;
        let mut smoke = Vec::new();
        let mut managed_any = false;
        for provider in &config.providers {
            match provider {
                ProviderDecl::Named(name) => {
                    if !listed.contains_key(name) {
                        return Err(needs(
                            format!("the OpenShell provider {name}"),
                            Some(format!("openshell provider create --name {name} --type <profile> --from-existing")),
                        ));
                    }
                }
                ProviderDecl::Managed(managed) => {
                    managed_any = true;
                    let env = self.ensure_profile(&base, managed, &suffix).await?;
                    self.ensure_provider(&base, managed, &suffix, &env, &listed).await?;
                    smoke.push(SmokeProvider { name: managed.name.clone(), kind: managed.kind.clone(), env });
                }
            }
        }

        // The smoke, when anything it proves is the daemon's.
        if !(managed_any || config.image.is_none()) {
            return Ok(());
        }
        let gateway_names: Vec<String> = config.providers.iter().map(|p| p.gateway_name(&suffix)).collect();
        let policy = os::smoke_policy(config, &callback, &smoke)
            .map_err(|e| needs(format!("a policy the smoke can be made with ({e})"), None))?;
        let script = os::smoke_script(&smoke, agent.git.as_deref());
        let print = {
            let given = lock(&self.provision.given);
            let mut hash = Sha256::new();
            for part in [image_path.as_str(), policy.as_str(), script.as_str()] {
                hash.update(part.as_bytes());
                hash.update([0]);
            }
            for name in &gateway_names {
                hash.update(name.as_bytes());
                if let Some(digest) = given.get(&(base.clone(), name.clone())) {
                    hash.update(digest);
                }
            }
            hex(&hash.finalize())
        };
        if lock(&self.provision.smoked).get(&agent.key) == Some(&print) {
            return Ok(());
        }
        let instance = factory.config.instance.id.clone();
        let state_root = factory.factory_dir().join("openshell-provision");
        match smoke_run(&base, &instance, &state_root, &image_path, &gateway_names, &policy, &script).await {
            Ok(more) => {
                notes.extend(more);
                lock(&self.provision.smoked).insert(agent.key.clone(), print);
                Ok(())
            }
            Err(reason) => Err(needs(format!("a passing smoke run ({reason})"), smoke_hint(config, &reason))),
        }
    }

    /// Step 1: the gateway answers, or is started and then answers.
    async fn ensure_gateway(&self, base: &[String]) -> Result<(), Unready> {
        let label = base.get(2).cloned().unwrap_or_else(|| "(active)".into());
        let (status, mut row) = gateway_status(base, &label).await;
        if status {
            self.record_gateway(base, row);
            return Ok(());
        }
        let local = row.server.as_deref().is_none_or(is_local_server);
        let may_start = {
            let gateways = lock(&self.provision.gateways);
            gateways.get(base).and_then(|g| g.last_start).is_none_or(|t| t.elapsed() >= START_BACKOFF)
        };
        let command = format!("launchctl kickstart gui/{}/{GATEWAY_SERVICE}", unsafe { libc::getuid() });
        if local && may_start {
            lock(&self.provision.gateways).entry(base.to_vec()).or_default().last_start = Some(Instant::now());
            match start_gateway(&self.provision.tools.launchctl).await {
                Ok(how) => {
                    tracing::warn!(gateway = %label, "the OpenShell gateway was {}; started it with `{how}`", row.status);
                    let deadline = Instant::now() + self.provision.tools.gateway_wait;
                    loop {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        let (up, again) = gateway_status(base, &label).await;
                        row = again;
                        if up || Instant::now() >= deadline {
                            row.started_at = Some(Utc::now());
                            row.started_with = Some(how.clone());
                            let up_now = up;
                            self.record_gateway(base, row.clone());
                            if up_now {
                                return Ok(());
                            }
                            break;
                        }
                    }
                }
                Err(e) => {
                    row.detail = Some(format!("could not start it: {e}"));
                }
            }
        }
        let status = row.status.clone();
        self.record_gateway(base, row);
        Err(needs(format!("the OpenShell gateway {label}, which is {status}"), Some(command)))
    }

    fn record_gateway(&self, base: &[String], mut row: GatewayRow) {
        let mut gateways = lock(&self.provision.gateways);
        let entry = gateways.entry(base.to_vec()).or_default();
        if row.started_at.is_none() {
            if let Some(previous) = &entry.row {
                row.started_at = previous.started_at;
                row.started_with = previous.started_with.clone();
            }
        }
        entry.row = Some(row);
    }

    /// Step 2 for Factory's own image: the current key's, else the newest
    /// previous one while the current builds, else `preparing`. A note says
    /// when a rebuild is under way.
    fn ensure_image(self: &Arc<Self>, factory_dir: &Path) -> Result<(String, Option<String>), Unready> {
        let dir = images_dir(factory_dir);
        let cli_digest = file_digest(&self.factory_bin).unwrap_or_else(|| "unreadable".into());
        let key = os::image_key(&cli_digest);
        let current = dir.join(&key).join(os::IMAGE_FILE);
        if current.is_file() {
            gc_images(&dir, &key);
            return Ok((current.display().to_string(), None));
        }
        let started = self.start_build(&dir, &key);
        let previous = newest_image(&dir, &key);
        let failure = lock(&self.provision.build).failed.clone().filter(|(k, _, _)| *k == key);
        match (previous, failure) {
            (Some(previous), None) => Ok((
                previous.display().to_string(),
                Some(format!("a new image ({key}) is being built in the background; runs use {} until it is ready", previous.display())),
            )),
            (Some(previous), Some((_, _, reason))) => Ok((
                previous.display().to_string(),
                Some(format!("the new image ({key}) did not build ({reason}); runs use {}", previous.display())),
            )),
            (None, Some((_, _, reason))) => Err(needs(
                format!("Factory's sandbox image, whose build failed: {reason}"),
                Some(self.build_command(&dir)),
            )),
            (None, None) => match started {
                Err(reason) => Err(needs(format!("Factory's sandbox image ({reason})"), None)),
                Ok(()) => Err(Unready::Preparing(format!(
                    "building Factory's sandbox image (log: {})",
                    dir.join(format!("{key}.log")).display()
                ))),
            },
        }
    }

    fn build_command(&self, dir: &Path) -> String {
        match &self.provision.tools.source {
            Some(source) => format!("OUT={} {}/examples/openshell/build-image.sh rootfs", dir.display(), source.display()),
            None => "set FACTORY_OPENSHELL_SOURCE to a Factory checkout, or set openshell.image".into(),
        }
    }

    /// Start the build of `key` in the background unless it is running or
    /// failed recently.
    fn start_build(self: &Arc<Self>, dir: &Path, key: &str) -> Result<(), String> {
        {
            let mut build = lock(&self.provision.build);
            if build.running.is_some() {
                return Ok(());
            }
            if let Some((failed, at, _)) = &build.failed {
                if failed == key && at.elapsed() < BUILD_RETRY {
                    return Ok(());
                }
            }
            build.running = Some((key.to_string(), Utc::now()));
        }
        let engine = self.clone();
        let dir = dir.to_path_buf();
        let key = key.to_string();
        tokio::spawn(async move {
            let outcome = build_image(&engine.provision.tools, &dir, &key).await;
            {
                let mut build = lock(&engine.provision.build);
                build.running = None;
                match &outcome {
                    Ok(()) => build.failed = None,
                    Err(reason) => build.failed = Some((key.clone(), Instant::now(), reason.clone())),
                }
            }
            match outcome {
                Ok(()) => tracing::info!(image = %key, "built Factory's OpenShell image"),
                Err(reason) => tracing::warn!(image = %key, "Factory's OpenShell image did not build: {reason}"),
            }
            engine.provision.wake.notify_one();
        });
        Ok(())
    }

    /// Step 3: the profile a managed provider's type names, imported under
    /// this instance's suffix when Factory ships it. Answers the variable
    /// the profile reads its credential from.
    async fn ensure_profile(&self, base: &[String], managed: &ManagedProvider, suffix: &str) -> Result<String, Unready> {
        let Some(yaml) = os::profile_for_import(&managed.kind, suffix) else {
            // Not one of Factory's: it has to be on the gateway already.
            let listed = list_profiles(base).await.map_err(|e| needs(format!("the gateway's profile list ({e})"), None))?;
            let profile = listed.iter().find(|p| p.get("id").and_then(|v| v.as_str()) == Some(managed.kind.as_str()));
            return match profile.and_then(os::profile_credential_env) {
                Some(env) => Ok(env),
                None if profile.is_none() => Err(needs(
                    format!("the OpenShell provider profile {}", managed.kind),
                    Some(format!("openshell profile import -f <{}.yaml>", managed.kind)),
                )),
                None => Err(needs(format!("a credential variable in the profile {}", managed.kind), None)),
            };
        };
        let id = os::profile_id(&managed.kind, suffix);
        let env = os::shipped_credential_env(&managed.kind)
            .ok_or_else(|| needs(format!("a credential variable in Factory's profile {}", managed.kind), None))?;
        let digest: [u8; 32] = Sha256::digest(yaml.as_bytes()).into();
        let key = (base.to_vec(), id.clone());
        if lock(&self.provision.profiles).get(&key) == Some(&digest) {
            return Ok(env);
        }
        if !os::owned_by_instance(&id, suffix) {
            return Err(needs(format!("a profile id this instance owns (not {id})"), None));
        }
        let listed = list_profiles(base).await.map_err(|e| needs(format!("the gateway's profile list ({e})"), None))?;
        let present = listed.iter().any(|p| p.get("id").and_then(|v| v.as_str()) == Some(id.as_str()));
        let file = tempfile_with(&yaml).map_err(|e| needs(format!("a place to write the profile ({e})"), None))?;
        let path = file.0.display().to_string();
        let mut argv = base.to_vec();
        if present {
            argv.extend(["profile", "update", "-f", &path].map(String::from));
        } else {
            argv.extend(["profile", "import", "-f", &path].map(String::from));
        }
        exec(&argv, None, QUICK, "importing the provider profile", &[])
            .await
            .map_err(|e| needs(format!("the provider profile {id} on the gateway ({e})"), None))?;
        lock(&self.provision.profiles).insert(key, digest);
        Ok(env)
    }

    /// Step 4: the provider exists with the source's current value.
    async fn ensure_provider(
        &self,
        base: &[String],
        managed: &ManagedProvider,
        suffix: &str,
        env: &str,
        listed: &BTreeMap<String, String>,
    ) -> Result<(), Unready> {
        let name = os::managed_name(&managed.name, suffix);
        if !os::owned_by_instance(&name, suffix) {
            return Err(needs(format!("a provider name this instance owns (not {name})"), None));
        }
        let value = resolve(&managed.credential).await.map_err(|reason| {
            needs(
                format!("the credential for {} from {} ({reason})", managed.name, managed.credential.describe()),
                Some(supply_hint(managed)),
            )
        })?;
        let digest = self.provision.digest(base, &name, &value);
        let key = (base.to_vec(), name.clone());
        let profile = os::profile_id(&managed.kind, suffix);
        let existing = listed.get(&name);
        if existing == Some(&profile) && lock(&self.provision.given).get(&key) == Some(&digest) {
            return Ok(());
        }
        let mut argv = base.to_vec();
        match existing {
            Some(kind) if *kind == profile => {
                argv.extend(["provider", "update", &name, "--credential", env].map(String::from));
            }
            Some(_) => {
                // Its type changed: it is this instance's, so it is made again.
                let mut delete = base.to_vec();
                delete.extend(["provider", "delete", &name].map(String::from));
                exec(&delete, None, QUICK, "replacing the provider", &[])
                    .await
                    .map_err(|e| needs(format!("the provider {name} replaced with type {profile} ({e})"), None))?;
                argv.extend(["provider", "create", "--name", &name, "--type", &profile, "--credential", env].map(String::from));
            }
            None => {
                argv.extend(["provider", "create", "--name", &name, "--type", &profile, "--credential", env].map(String::from));
            }
        }
        exec(&argv, Some((env, &value)), QUICK, "giving the provider its credential", &[value.expose()])
            .await
            .map_err(|e| needs(format!("the provider {name} on the gateway ({e})"), None))?;
        tracing::info!(provider = %name, "openshell provider {} from {}", if existing.is_some() { "updated" } else { "created" }, managed.credential.describe());
        lock(&self.provision.given).insert(key, digest);
        Ok(())
    }

    /// What dispatch asks before it makes a run's sandbox: `Ok` with what
    /// the run is made from, or the reason it must not start. An agent whose
    /// block names nothing the daemon keeps is not asked about at all --
    /// the run's own preflight checks it as it always has.
    pub(crate) async fn sandbox_gate(self: &Arc<Self>, key: &AgentKey, config: &OpenshellConfig) -> std::result::Result<Resolved, String> {
        let factory = self.factory_snapshot();
        let suffix = os::instance_suffix(&factory.config.instance.id);
        let providers: Vec<String> = config.providers.iter().map(|p| p.gateway_name(&suffix)).collect();
        let managed = config.image.is_none() || config.providers.iter().any(|p| matches!(p, ProviderDecl::Managed(_)));
        if !managed {
            return Ok(Resolved { image: config.image.clone().unwrap_or_default(), providers });
        }
        let deadline = Instant::now() + GATE_WAIT;
        loop {
            let settled = self.provision.settled.notified();
            match self.provision.readiness(key) {
                Some(r) if r.state == ReadinessState::Ready => {
                    let image = config.image.clone().or(r.image.clone()).unwrap_or_default();
                    return Ok(Resolved { image, providers });
                }
                Some(r) if r.state == ReadinessState::Needs => return Err(r.reason()),
                other => {
                    if Instant::now() >= deadline {
                        return Err(other.map(|r| r.reason()).unwrap_or_else(|| {
                            "the sandbox's prerequisites have not been checked yet; the daemon is checking them now".into()
                        }));
                    }
                    self.provision.wake.notify_one();
                    let _ = tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), settled).await;
                }
            }
        }
    }
}

/// Two agents declaring one managed provider name with different types or
/// sources, on the same gateway.
fn conflicting_providers(agents: &[Declared]) -> BTreeMap<AgentKey, String> {
    let mut seen: BTreeMap<(Option<String>, String), (AgentKey, String, CredentialSource)> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for agent in agents {
        for provider in &agent.config.providers {
            let ProviderDecl::Managed(m) = provider else { continue };
            let key = (agent.config.gateway.clone(), m.name.clone());
            match seen.get(&key) {
                Some((other, kind, source)) if *kind != m.kind || *source != m.credential => {
                    let why = format!(
                        "one declaration of the provider {} (scope {} agent {} declares it differently)",
                        m.name, other.0, other.1
                    );
                    out.insert(agent.key.clone(), why.clone());
                    out.insert(other.clone(), format!(
                        "one declaration of the provider {} (scope {} agent {} declares it differently)",
                        m.name, agent.key.0, agent.key.1
                    ));
                }
                Some(_) => {}
                None => {
                    seen.insert(key, (agent.key.clone(), m.kind.clone(), m.credential.clone()));
                }
            }
        }
    }
    out
}

fn check_explicit_image(image: &str) -> Result<(), Unready> {
    let local = image.starts_with('/') || image.starts_with("~/");
    if !local {
        return Ok(());
    }
    let path = expand_home(image);
    if path.is_file() {
        Ok(())
    } else {
        Err(needs(
            format!("the image file {image}"),
            Some(format!("OUT={} examples/openshell/build-image.sh rootfs", path.parent().map(|p| p.display().to_string()).unwrap_or_default())),
        ))
    }
}

fn is_local_server(server: &str) -> bool {
    let host = server.split("://").nth(1).unwrap_or(server);
    let host = host.split('/').next().unwrap_or(host);
    let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// `openshell status -o json`: whether it is connected, and the row L1 shows.
async fn gateway_status(base: &[String], label: &str) -> (bool, GatewayRow) {
    let mut argv = base.to_vec();
    argv.extend(["status", "-o", "json"].map(String::from));
    let mut row = GatewayRow {
        gateway: label.to_string(),
        cli: base[0].clone(),
        status: "unreachable".into(),
        server: None,
        version: None,
        checked_at: Utc::now(),
        started_at: None,
        started_with: None,
        detail: None,
    };
    match exec(&argv, None, QUICK, "asking the gateway", &[]).await {
        Ok(out) => match serde_json::from_str::<serde_json::Value>(out.trim()) {
            Ok(v) => {
                row.status = v.get("status").and_then(|s| s.as_str()).unwrap_or("unknown").to_string();
                row.server = v.get("server").and_then(|s| s.as_str()).map(str::to_string);
                row.version = v.get("version").and_then(|s| s.as_str()).map(str::to_string);
                if let Some(name) = v.get("gateway").and_then(|s| s.as_str()) {
                    if label == "(active)" {
                        row.gateway = name.to_string();
                    }
                }
            }
            Err(_) => row.detail = Some(out.chars().take(300).collect()),
        },
        Err(e) => row.detail = Some(e),
    }
    (row.status.eq_ignore_ascii_case("connected"), row)
}

/// `launchctl kickstart` of the Homebrew service -- not `-k`, which would
/// restart a gateway that is up and take its sandboxes with it. A job that
/// is not loaded is bootstrapped from its plist first.
async fn start_gateway(launchctl: &Path) -> std::result::Result<String, String> {
    let uid = unsafe { libc::getuid() };
    let target = format!("gui/{uid}/{GATEWAY_SERVICE}");
    let launchctl = launchctl.display().to_string();
    let loaded = exec(&[launchctl.clone(), "print".into(), target.clone()], None, QUICK, "looking for the gateway's service", &[]).await;
    if loaded.is_err() {
        let plist = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(format!("Library/LaunchAgents/{GATEWAY_SERVICE}.plist"));
        if !plist.is_file() {
            return Err(format!("no launchd job {target} and no {}", plist.display()));
        }
        exec(
            &[launchctl.clone(), "bootstrap".into(), format!("gui/{uid}"), plist.display().to_string()],
            None,
            QUICK,
            "loading the gateway's service",
            &[],
        )
        .await?;
    }
    exec(&[launchctl.clone(), "kickstart".into(), target.clone()], None, QUICK, "starting the gateway", &[]).await?;
    Ok(format!("launchctl kickstart {target}"))
}

/// `provider list -o json`, as name -> type.
async fn list_providers(base: &[String]) -> std::result::Result<BTreeMap<String, String>, String> {
    let mut argv = base.to_vec();
    argv.extend(["provider", "list", "-o", "json"].map(String::from));
    let out = exec(&argv, None, QUICK, "listing providers", &[]).await?;
    let v: serde_json::Value = serde_json::from_str(out.trim()).map_err(|e| format!("listing providers: {e}"))?;
    Ok(v.get("providers")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some((p.get("name")?.as_str()?.to_string(), p.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string()))
        })
        .collect())
}

async fn list_profiles(base: &[String]) -> std::result::Result<Vec<serde_json::Value>, String> {
    let mut argv = base.to_vec();
    argv.extend(["profile", "list", "-o", "json"].map(String::from));
    let out = exec(&argv, None, QUICK, "listing profiles", &[]).await?;
    let v: serde_json::Value = serde_json::from_str(out.trim()).map_err(|e| format!("listing profiles: {e}"))?;
    Ok(v.as_array().cloned().unwrap_or_default())
}

/// One child: a deadline, nothing on stdin, `secret` (if any) in its
/// environment only, and a failure reason that never contains stdout's
/// secrets -- every string in `redact` is cut out of it.
async fn exec(
    argv: &[String],
    secret: Option<(&str, &Secret)>,
    deadline: Duration,
    what: &str,
    redact: &[&str],
) -> std::result::Result<String, String> {
    let (program, args) = argv.split_first().ok_or_else(|| format!("{what}: nothing to run"))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .env("NO_COLOR", "1")
        .env("PATH", augmented_path())
        .kill_on_drop(true);
    if let Some((key, value)) = secret {
        command.env(key, value.expose());
    }
    let output = match tokio::time::timeout(deadline, command.output()).await {
        Err(_) => return Err(format!("{what}: `{}` did not finish within {}s", short(argv), deadline.as_secs())),
        Ok(Err(e)) => return Err(format!("{what}: could not run {program}: {e}")),
        Ok(Ok(output)) => output,
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut reason = one_line(&stderr);
    if reason.is_empty() && secret.is_none() && redact.is_empty() {
        reason = one_line(&stdout);
    }
    for word in redact.iter().filter(|w| !w.is_empty()) {
        reason = reason.replace(word, "[redacted]");
    }
    Err(format!("{what}: {} ({})", if reason.is_empty() { "no output".into() } else { reason }, output.status))
}

fn one_line(text: &str) -> String {
    let joined = text
        .lines()
        .map(|l| l.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '×' | '│' | '╰' | '─' | '╭' | '|')).trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    joined.chars().take(400).collect()
}

fn short(argv: &[String]) -> String {
    argv.iter().take(4).cloned().collect::<Vec<_>>().join(" ")
}

/// The daemon's PATH, then Homebrew's, `/usr/local`'s, cargo's and the
/// system's: a daemon started by launchd has only the last, and `gh`,
/// `crane` and `cargo` live in the others.
pub(crate) fn augmented_path() -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut extra: Vec<PathBuf> = vec!["/opt/homebrew/bin".into(), "/usr/local/bin".into()];
    if let Some(home) = home {
        extra.push(home.join(".cargo/bin"));
    }
    extra.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from));
    for dir in extra {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
        None => PathBuf::from(path),
    }
}

/// Read a credential from its source. The reason on failure says what was
/// wrong with the source and never contains a value.
pub(crate) async fn resolve(source: &CredentialSource) -> std::result::Result<Secret, String> {
    let value = match source {
        CredentialSource::Env { name } => std::env::var(name).map_err(|_| format!("{name} is not set in the daemon's environment"))?,
        CredentialSource::File { path } => read_private_file(&expand_home(path))?,
        CredentialSource::Command { run } => {
            command_value(&["/bin/sh".to_string(), "-c".to_string(), run.clone()], "it").await?
        }
        CredentialSource::Keychain { service, account } => {
            let mut argv = vec!["/usr/bin/security".to_string(), "find-generic-password".into(), "-s".into(), service.clone()];
            if let Some(account) = account {
                argv.extend(["-a".to_string(), account.clone()]);
            }
            argv.push("-w".into());
            command_value(&argv, "the Keychain lookup").await?
        }
    };
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err("it is empty".into());
    }
    Ok(Secret(value))
}

/// The file's whole content, when it is a regular file of this user's that
/// nobody else can read or write.
fn read_private_file(path: &Path) -> std::result::Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(format!("{} is not owned by the daemon's user", path.display()));
    }
    let mode = meta.mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(format!(
            "{} is readable or writable by others (mode {mode:04o}); make it owner-only with chmod 600",
            path.display()
        ));
    }
    if meta.len() > 64 * 1024 {
        return Err(format!("{} is larger than a credential", path.display()));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{} cannot be read: {e}", path.display()))
}

/// A command's stdout. Its failure names the exit status and stderr --
/// with anything it printed on stdout cut out -- but never stdout.
async fn command_value(argv: &[String], what: &str) -> std::result::Result<String, String> {
    let (program, args) = argv.split_first().ok_or("nothing to run")?;
    let child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .env("PATH", augmented_path())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(QUICK, child).await {
        Err(_) => return Err(format!("{what} did not finish within {}s", QUICK.as_secs())),
        Ok(Err(e)) => return Err(format!("{what} could not be run: {e}")),
        Ok(Ok(output)) => output,
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if output.status.success() {
        return Ok(stdout);
    }
    let mut stderr = one_line(&String::from_utf8_lossy(&output.stderr));
    for line in stdout.lines().map(str::trim).filter(|l| l.len() >= 4) {
        stderr = stderr.replace(line, "[redacted]");
    }
    Err(format!("{what} failed ({}){}", output.status, if stderr.is_empty() { String::new() } else { format!(": {stderr}") }))
}

/// The exact command that puts a value at a managed provider's source.
fn supply_hint(managed: &ManagedProvider) -> String {
    let claude = managed.kind.contains("claude");
    match &managed.credential {
        CredentialSource::File { path } if claude => {
            format!("claude setup-token, then (umask 077; cat > {path}) and paste the token")
        }
        CredentialSource::File { path } => format!("(umask 077; cat > {path}) and paste the credential"),
        CredentialSource::Env { name } => {
            format!("add {name} to the daemon's launchd plist EnvironmentVariables and restart the daemon")
        }
        CredentialSource::Command { run } => format!("make `{run}` print the credential as the daemon's user"),
        CredentialSource::Keychain { service, account } => format!(
            "security add-generic-password -s {service}{} -w",
            account.as_ref().map(|a| format!(" -a {a}")).unwrap_or_default()
        ),
    }
}

fn smoke_hint(config: &OpenshellConfig, reason: &str) -> Option<String> {
    if reason.contains("rejected") {
        let source = config.providers.iter().find_map(|p| match p {
            ProviderDecl::Managed(m) if reason.contains(&format!("provider {}", m.name)) => Some(supply_hint(m)),
            _ => None,
        });
        if source.is_some() {
            return source;
        }
    }
    Some("openshell logs <sandbox> --source sandbox | grep -i denied".into())
}

/// Step 5: a throwaway sandbox from the image, the agent's policy (plus
/// `os::SMOKE_RULE`) and its providers; the script; the verdict; delete.
/// Labelled as this instance's with a smoke run id, so the startup
/// reconcile deletes one a crash left behind.
async fn smoke_run(
    base: &[String],
    instance: &str,
    state_root: &Path,
    image: &str,
    providers: &[String],
    policy: &str,
    script: &str,
) -> std::result::Result<Vec<String>, String> {
    let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let name = format!("factory-s{id}");
    let dir = state_root.join(&name);
    write_private(&dir, "policy.yaml", policy).map_err(|e| format!("writing the smoke policy: {e}"))?;
    let mut create = base.to_vec();
    create.extend(
        ["sandbox", "create", "--name", &name, "--from", image, "--policy", &dir.join("policy.yaml").display().to_string()]
            .map(String::from),
    );
    for provider in providers {
        create.extend(["--provider".to_string(), provider.clone()]);
    }
    create.extend([
        "--label".to_string(),
        format!("{}={instance}", os::LABEL_INSTANCE),
        "--label".to_string(),
        format!("{}=smoke-{id}", os::LABEL_RUN),
        "--no-auto-providers".into(),
        "--detach".into(),
        "-o".into(),
        "json".into(),
    ]);
    let outcome = async {
        exec(&create, None, SMOKE_CREATE, "making the smoke sandbox", &[]).await?;
        let mut run = base.to_vec();
        run.extend(
            ["sandbox", "exec", "-n", &name, "--no-login-shell", "--timeout", &SMOKE_EXEC_SECS.to_string(), "--", "sh", "-c", script]
                .map(String::from),
        );
        let out = exec(&run, None, Duration::from_secs(SMOKE_EXEC_SECS + 30), "running the smoke", &[]).await?;
        os::judge_smoke(&out)
    }
    .await;
    let mut delete = base.to_vec();
    delete.extend(["sandbox", "delete", &name].map(String::from));
    if let Err(e) = exec(&delete, None, QUICK, "deleting the smoke sandbox", &[]).await {
        tracing::warn!(sandbox = %name, "{e}; the next daemon start deletes it");
    }
    let _ = std::fs::remove_dir_all(&dir);
    outcome
}

fn write_private(dir: &Path, name: &str, contents: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(dir.join(name))?;
    std::io::Write::write_all(&mut file, contents.as_bytes())
}

/// A file that removes itself.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn tempfile_with(contents: &str) -> std::io::Result<TempFile> {
    let dir = std::env::temp_dir().join(format!("factory-provision-{}", uuid::Uuid::new_v4().simple()));
    write_private(&dir, "profile.yaml", contents)?;
    let path = dir.join("profile.yaml");
    Ok(TempFile(path))
}

pub(crate) fn images_dir(factory_dir: &Path) -> PathBuf {
    factory_dir.join("openshell-images")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn file_digest(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex(&Sha256::digest(&bytes)))
}

fn is_key(name: &str) -> bool {
    name.len() == 16 && name.chars().all(|c| c.is_ascii_hexdigit())
}

/// The newest built image other than `key`'s.
fn newest_image(dir: &Path, key: &str) -> Option<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| is_key(n) && n != key))
        .filter_map(|e| {
            let image = e.path().join(os::IMAGE_FILE);
            let modified = image.metadata().ok()?.modified().ok()?;
            Some((modified, image))
        })
        .collect();
    found.sort();
    found.pop().map(|(_, path)| path)
}

/// Keep the current image and the newest one before it -- a run created
/// from that one may still be starting -- and remove every older one.
fn gc_images(dir: &Path, key: &str) {
    let keep = newest_image(dir, key).and_then(|p| p.parent().map(Path::to_path_buf));
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        if is_key(&name) && name != key && Some(entry.path()) != keep {
            let _ = std::fs::remove_dir_all(entry.path());
            let _ = std::fs::remove_file(dir.join(format!("{name}.log")));
        }
    }
}

/// Build image `key` into `dir/<key>/`: the recipe this daemon carries,
/// written out and run in `rootfs` mode into `dir/<key>.partial`, which is
/// renamed into place only when the image is complete. Its output goes to
/// `dir/<key>.log`. The CLI is cross-compiled from `tools.source` into a
/// target directory kept under `dir`, never inside the source tree.
async fn build_image(tools: &Tools, dir: &Path, key: &str) -> std::result::Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).map_err(|e| format!("making {}: {e}", dir.display()))?;
    let partial = dir.join(format!("{key}.partial"));
    let recipe = dir.join(format!("{key}.recipe"));
    let _ = std::fs::remove_dir_all(&partial);
    let _ = std::fs::remove_dir_all(&recipe);
    std::fs::create_dir_all(&recipe).map_err(|e| format!("making {}: {e}", recipe.display()))?;
    for (name, contents) in os::RECIPE {
        std::fs::write(recipe.join(name), contents).map_err(|e| format!("writing the recipe: {e}"))?;
    }
    let _ = std::fs::set_permissions(recipe.join("build-image.sh"), std::fs::Permissions::from_mode(0o755));
    let script = tools.build_script.clone().unwrap_or_else(|| recipe.join("build-image.sh"));
    let source = match (&tools.source, &tools.build_script) {
        (Some(source), _) => source.clone(),
        (None, Some(_)) => PathBuf::new(),
        (None, None) => {
            return Err("no Factory source tree to build the in-image CLI from; set FACTORY_OPENSHELL_SOURCE".into())
        }
    };
    let log_path = dir.join(format!("{key}.log"));
    let log = std::fs::File::create(&log_path).map_err(|e| format!("opening {}: {e}", log_path.display()))?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    let child = Command::new("/bin/sh")
        .arg(&script)
        .arg("rootfs")
        .env("OUT", &partial)
        .env("FACTORY_SOURCE", &source)
        .env("FACTORY_TARGET_DIR", dir.join("cargo-target"))
        .env("PATH", augmented_path())
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .kill_on_drop(true)
        .status();
    let status = tokio::time::timeout(BUILD_DEADLINE, child).await;
    let _ = std::fs::remove_dir_all(&recipe);
    let tail = || {
        std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" / ")
    };
    match status {
        Err(_) => return Err(format!("it did not finish within {}m; see {}", BUILD_DEADLINE.as_secs() / 60, log_path.display())),
        Ok(Err(e)) => return Err(format!("it could not be run: {e}")),
        Ok(Ok(s)) if !s.success() => return Err(format!("{s}: {}; see {}", tail(), log_path.display())),
        Ok(Ok(_)) => {}
    }
    if !partial.join(os::IMAGE_FILE).is_file() {
        return Err(format!("it finished without writing {}; see {}", os::IMAGE_FILE, log_path.display()));
    }
    let target = dir.join(key);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&partial, &target).map_err(|e| format!("moving the image into place: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
