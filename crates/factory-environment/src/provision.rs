//! The command port from L3 to L2 (#193 phase 6, S8b): what Agent asks of Environment.
//!
//! A level may command only the level directly below it. L3 asks L2 to gate, prepare,
//! discard, release and reconcile an agent's OpenShell sandbox through `Provision`; L4
//! reaches the sandbox only through L3 (`factory_agents::dispatch::Environments`), never
//! L2 itself. What L2 does that the caller journals is handed back as `Note`s through a
//! `Notes` capability the caller supplies, at the moment each thing happens, so the
//! journal reads as it always has and L2 never names a task store.
//!
//! A port at any other level cannot implement `Provision`:
//!
//! ```compile_fail
//! use factory_environment::provision::{Provision, Notes, Prepared, PrepareRequest, Resolved, RunLedger};
//! use factory_kernel::{CommandPort, L1};
//! struct Wrong;
//! impl CommandPort for Wrong { type Level = L1; }
//! #[async_trait::async_trait]
//! impl Provision for Wrong {
//!     async fn gate(&self, _: &(String, String), _: &factory_environment::openshell::OpenshellConfig) -> Result<Resolved, String> { unreachable!() }
//!     async fn prepare(&self, _: PrepareRequest<'_>, _: &dyn Notes) -> factory_kernel::Result<Prepared> { unreachable!() }
//!     async fn discard(&self, _: &factory_environment::openshell::Plan) {}
//!     fn release(&self, _: &std::collections::BTreeMap<String, String>, _: String, _: String, _: std::sync::Arc<dyn Notes>) {}
//!     async fn reconcile(&self, _: factory_environment::provision::Reconcile<'_>, _: &dyn RunLedger, _: &dyn Notes) {}
//!     fn forget_preserved(&self, _: &std::path::Path, _: &str) {}
//! }
//! ```
use crate::openshell::{CallbackTarget, OpenshellConfig, Plan};
use crate::sandbox_runtime::{RestoreOutcome, Teardown};
use factory_kernel::error::{FactoryError, Result};
use factory_kernel::{CommandPort, LaunchSpec, L2};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a dispatch resolves from a `ready` agent: the image and the provider names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub image: String,
    pub providers: Vec<String>,
}

/// One thing that happened that the caller records on the run: the task and run it is
/// for, the words, and (for the ready line) the structured detail. Always a `sandbox` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub task: String,
    pub run: String,
    pub message: String,
    pub data: Option<serde_json::Value>,
}
impl Note {
    pub fn new(task: &str, run: &str, message: String) -> Self {
        Self { task: task.to_string(), run: run.to_string(), message, data: None }
    }
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// Where L2 puts a `Note`, supplied by the caller. Awaited at the point the thing happens.
#[async_trait::async_trait]
pub trait Notes: Send + Sync {
    async fn note(&self, note: Note);
}

/// The runs that have not finished, read lazily by the reconcile: only when there is
/// something to reconcile, as before.
#[async_trait::async_trait]
pub trait RunLedger: Send + Sync {
    async fn active_runs(&self) -> Result<BTreeSet<String>>;
}

/// Everything `prepare` needs, as plain data.
pub struct PrepareRequest<'a> {
    pub instance_id: &'a str,
    pub root: PathBuf,
    pub factory_dir: PathBuf,
    pub guides_dir: PathBuf,
    pub task_id: &'a str,
    pub scope: &'a str,
    pub agent: &'a str,
    pub run_id: &'a str,
    pub cwd: &'a Path,
    /// What a *fresh* dispatch would launch with.
    pub launch: &'a LaunchSpec,
    pub prompt: &'a str,
    /// `Some` only for a tentatively-resuming sandboxed run (`#274`): the resumed launch
    /// and prompt, and the preserved conversation's local directory.
    pub resume: Option<(&'a LaunchSpec, &'a str, &'a Path)>,
    pub config: &'a OpenshellConfig,
    pub callback: &'a CallbackTarget,
    pub image: &'a str,
    pub providers: &'a [String],
}

/// A prepared sandbox: its plan, the teardown to keep on the session, and what happened to
/// the preserved conversation.
pub struct Prepared {
    pub plan: Plan,
    pub teardown: Teardown,
    pub restore: RestoreOutcome,
}

impl Prepared {
    /// What the session carries so its close can find the teardown (`META_KEY`).
    pub fn session_meta(&self) -> BTreeMap<String, String> {
        BTreeMap::from([(crate::sandbox_runtime::META_KEY.to_string(), self.teardown.to_meta())])
    }
}

/// What a reconcile of leaked sandboxes is asked over: the current declarations' configs.
pub struct Reconcile<'a> {
    pub instance_id: &'a str,
    pub factory_dir: &'a Path,
    pub configs: Vec<OpenshellConfig>,
}

#[async_trait::async_trait]
pub trait Provision: CommandPort<Level = L2> + Send + Sync {
    /// What dispatch asks before it makes a run's sandbox: what the run is made from, or
    /// the reason it must not start. The key is `(canonical scope, agent name)`.
    async fn gate(&self, key: &(String, String), config: &OpenshellConfig) -> std::result::Result<Resolved, String>;
    /// Plan the run's sandbox, make it, and say so through `notes`. An `Err` fails the run
    /// with the reason; nothing here ever falls back to the host.
    async fn prepare(&self, request: PrepareRequest<'_>, notes: &dyn Notes) -> Result<Prepared>;
    /// No session will ever carry this sandbox to its teardown: delete it now.
    async fn discard(&self, plan: &Plan);
    /// After the pane is gone: if the session carries a sandbox teardown, preserve what is
    /// worth keeping, delete the sandbox, and say what was done, in the background (claimed
    /// per sandbox, so a run closed twice is torn down once). Returns at once.
    fn release(&self, session_meta: &BTreeMap<String, String>, task: String, run: String, notes: Arc<dyn Notes>);
    /// On start: delete every sandbox this instance made whose run is no longer active.
    async fn reconcile(&self, request: Reconcile<'_>, ledger: &dyn RunLedger, notes: &dyn Notes);
    /// A task's preserved conversation is released with the task, never by a sweep.
    fn forget_preserved(&self, sessions_root: &Path, task: &str);
}

/// Plan the run's sandbox, make it, and say so. `launch`/`prompt` are always what a fresh
/// dispatch would use; the resumed pair switches in only once the preserved conversation
/// actually uploads (`#274`), so a failed upload can never launch a sandbox with
/// `--resume` pointing at a session that is not there.
pub async fn prepare_sandbox(request: PrepareRequest<'_>, notes: &dyn Notes) -> Result<Prepared> {
    let PrepareRequest {
        instance_id, root, factory_dir, guides_dir, task_id, scope, agent, run_id, cwd,
        launch, prompt, resume, config, callback, image, providers,
    } = request;

    let cli = crate::sandbox_runtime::resolve_cli(config.cli.as_deref())?;
    let state_dir = factory_dir.join("openshell").join(run_id);
            let plan = crate::openshell::plan(&crate::openshell::PlanInput {
        config,
        cli: &cli,
        image: image,
        providers: providers,
        instance_id: instance_id,
        run_id,
        task_id: task_id,
        cwd,
        guides_dir: &guides_dir,
        state_dir: &state_dir,
        launch,
        prompt,
        callback,
    })?;
    // The resumed variant is planned too -- identical in everything but
    // `launch.sh`/`prompt.md`'s content, which is all `prepare` needs
    // of it (`#274`).
    let restore = match resume {
        Some((resume_launch, resume_prompt, local_dir)) => {
            let resume_plan = crate::openshell::plan(&crate::openshell::PlanInput {
                config,
                cli: &cli,
                image: image,
                providers: providers,
                instance_id: instance_id,
                run_id,
                task_id: task_id,
                cwd,
                guides_dir: &guides_dir,
                state_dir: &state_dir,
                launch: resume_launch,
                prompt: resume_prompt,
                callback,
            })?;
            let override_files: Vec<_> = resume_plan
                .stage_files
                .into_iter()
                .filter(|(path, _, _)| {
                    path.file_name().and_then(|n| n.to_str()) == Some("launch.sh")
                        || path.file_name().and_then(|n| n.to_str()) == Some("prompt.md")
                })
                .collect();
            // Never true today -- the claude branch `plan()` takes
            // always stages both -- but an empty override would mean
            // `prepare` reports `Restored` while the sandbox actually
            // launches with the fresh (unresumed) files it already
            // staged, and the run row would wrongly go on claiming a
            // resume that never happened. Fail the dispatch outright
            // rather than let that silently drift.
            if override_files.is_empty() {
                return Err(FactoryError::Other(anyhow::anyhow!(
                    "the resumed plan staged neither launch.sh nor prompt.md; refusing to claim a resume prepare cannot actually stage"
                )));
            }
            Some(crate::sandbox_runtime::Restore { local_dir: local_dir.to_path_buf(), override_files })
        }
        None => None,
    };
    notes
        .note(Note::new(task_id, run_id, format!("creating OpenShell sandbox {} from {}", plan.sandbox, image)))
        .await;
    let mut teardown = crate::sandbox_runtime::Teardown::of(&plan, cwd, config.fast_forward, task_id);
    teardown.service_evidence = Some(crate::service_observations::CaptureContext {
        root: root.to_path_buf(), instance: instance_id.to_string(),
        scope: scope.to_string(), agent: agent.to_string(), task: task_id.to_string(),
        run: run_id.to_string(), base: plan.base.clone(),
    });
    crate::sandbox_runtime::Pending {
        instance: instance_id.to_string(),
        run: run_id.to_string(),
        task: task_id.to_string(),
        base: plan.base.clone(),
        teardown: teardown.clone(),
    }.save().map_err(|e| FactoryError::BadRequest(format!("could not persist OpenShell cleanup record: {e}")))?;
    let restore_outcome = Box::pin(crate::sandbox_runtime::prepare(&plan, providers, restore.as_ref())).await?;
    if let crate::sandbox_runtime::RestoreOutcome::FellBack(reason) = &restore_outcome {
        notes
            .note(Note::new(task_id, run_id, format!("preserved conversation not restored: {reason}; launching fresh")))
            .await;
    }
    notes
        .note(Note::new(task_id, run_id, format!("sandbox {} is ready; the harness runs in {}", plan.sandbox, plan.workdir)).with_data(serde_json::json!({
            "sandbox": plan.sandbox,
            "image": image,
            "providers": providers,
            "workdir": plan.workdir,
        })))
        .await;
    Ok(Prepared { plan, teardown, restore: restore_outcome })
}

/// On start: delete every OpenShell sandbox this instance made whose run is no longer
/// active -- one a crash, a lost session or a failed delete left behind (`#218`).
pub async fn reconcile_sandboxes(request: Reconcile<'_>, ledger: &dyn RunLedger, notes: &dyn Notes) {
    let Reconcile { instance_id, factory_dir, configs } = request;
    let pending = match crate::sandbox_runtime::pending(&factory_dir.join("openshell"), instance_id) {
        Ok(pending) => pending,
        Err(e) => {
            tracing::warn!("openshell reconcile: could not read cleanup records: {e}");
            Vec::new()
        }
    };
    if configs.is_empty() && pending.is_empty() {
        return;
    }
    let active: std::collections::BTreeSet<String> = match ledger.active_runs().await {
        Ok(active) => active,
        Err(e) => {
            tracing::warn!("openshell reconcile: could not list active runs: {e}");
            return;
        }
    };
    let mut bases: std::collections::BTreeSet<Vec<String>> = pending.iter().map(|record| record.base.clone()).collect();
    for config in configs {
        let Ok(cli) = crate::sandbox_runtime::resolve_cli(config.cli.as_deref()) else { continue };
        let mut base = vec![cli];
        if let Some(gateway) = &config.gateway {
            base.push("-g".into());
            base.push(gateway.clone());
        }
        bases.insert(base);
    }
    for base in bases {
        let ours = match crate::sandbox_runtime::list_ours(&base, instance_id).await {
            Ok(ours) => ours,
            Err(e) => {
                tracing::warn!("openshell reconcile: {e}");
                continue;
            }
        };
        // A successful authoritative list also settles a create that
        // never reached the gateway, or a delete completed just before
        // the previous daemon exited. Recovery output is never erased.
        for record in pending.iter().filter(|record| record.base == base && !active.contains(&record.run)) {
            if !ours.iter().any(|(name, run)| name == &record.teardown.sandbox && run == &record.run)
                && !record.teardown.state_dir.join("recovery").exists() {
                let _ = std::fs::remove_dir_all(&record.teardown.state_dir);
            }
        }
        for (name, run_id) in ours {
            if active.contains(&run_id) {
                continue;
            }
            let Some(_claim) = crate::sandbox_runtime::Claim::take(&name) else { continue };
            if let Some(record) = pending.iter().find(|record| record.base == base && record.run == run_id
                && record.teardown.sandbox == name && !record.teardown.state_dir.join("recovery").exists()) {
                for note in Box::pin(crate::sandbox_runtime::finish(&record.teardown)).await {
                    notes.note(Note::new(&record.task, &record.run, note)).await;
                }
                continue;
            }
            match crate::sandbox_runtime::delete(&base, &name).await {
                Ok(()) => {
                    tracing::info!(sandbox = %name, run = %run_id, "deleted an OpenShell sandbox its run left behind");
                    let state = factory_dir.join("openshell").join(&run_id);
                    if !state.join("recovery").exists() {
                        let _ = std::fs::remove_dir_all(state);
                    }
                }
                Err(e) => tracing::warn!(sandbox = %name, "openshell reconcile: {e}"),
            }
        }
    }
}
