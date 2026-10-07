//! The command ports from L4 to L3 (#193 phase 6, S8a): what Process asks of Agent.
//!
//! A level may command only the level directly below it (`Commands<L4, P>` with `P`
//! at L3). These traits are what L3 offers; `factory-daemon` implements them over its
//! L3 service. Nothing here names a task, a run row or a store: an assignment is plain
//! data (the `AgentContext` the adapters already take, a launch, a session to start), so
//! L3 never reaches up and L4 never reaches an adapter or a runtime.
//!
//! The sandbox is not here yet. Between `launch` and `start` L4 still prepares an OpenShell
//! sandbox itself; the L3 -> L2 provision command (S8b) moves that behind `start`.
//!
//! A port at any other level cannot implement these:
//!
//! ```compile_fail
//! use factory_agents::dispatch::Assignments;
//! use factory_kernel::{CommandPort, L2};
//! struct Wrong;
//! impl CommandPort for Wrong { type Level = L2; }
//! #[async_trait::async_trait]
//! impl Assignments for Wrong {
//!     async fn launch(&self, _: &str, _: &factory_agents::adapter::AgentContext, _: &[String],
//!         _: Option<&factory_agents::roster::ScopeAgent>) -> factory_kernel::Result<factory_kernel::LaunchSpec> { unreachable!() }
//!     async fn prompt(&self, _: &str, _: &factory_agents::adapter::AgentContext) -> factory_kernel::Result<String> { unreachable!() }
//!     async fn start(&self, _: factory_agents::dispatch::SessionStart) -> factory_kernel::Result<factory_kernel::SessionRef> { unreachable!() }
//!     async fn submit(&self, _: &str, _: &factory_kernel::SessionRef, _: &str) -> factory_kernel::Result<()> { unreachable!() }
//! }
//! ```
use crate::adapter::AgentContext;
use crate::harness::HeldTask;
use crate::roster::ScopeAgent;
use factory_kernel::{CommandPort, LaunchSpec, Result, SessionRef, L3};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// A session L4 asks L3 to open for a run.
#[derive(Debug, Clone)]
pub struct SessionStart {
    /// The runtime adapter's name (the task's `runtime`).
    pub runtime: String,
    pub run_id: String,
    /// The scope's canonical name: what the workspace and the herdr agent are keyed on.
    pub scope: String,
    pub agent: String,
    /// The task's title, truncated by L3 into the session's label.
    pub title: String,
    pub cwd: PathBuf,
    pub launch: LaunchSpec,
    /// Carried on the opened session (a sandbox's teardown record), set once it is up.
    pub meta: BTreeMap<String, String>,
}

/// Hand an agent an assignment and open its session.
#[async_trait::async_trait]
pub trait Assignments: CommandPort<Level = L3> + Send + Sync {
    /// The adapter's launch for `ctx`, with any resume arguments leading and the
    /// declaration's own arguments after, as every dispatch has built it.
    async fn launch(
        &self,
        adapter: &str,
        ctx: &AgentContext,
        resume_args: &[String],
        declared: Option<&ScopeAgent>,
    ) -> Result<LaunchSpec>;
    /// The prompt the adapter phrases for `ctx`.
    async fn prompt(&self, adapter: &str, ctx: &AgentContext) -> Result<String>;
    /// Open the session. The session reference is the acknowledgement.
    async fn start(&self, session: SessionStart) -> Result<SessionRef>;
    /// Type the prompt into an opened session.
    async fn submit(&self, runtime: &str, session: &SessionRef, prompt: &str) -> Result<()>;
}

/// What L4 tells L3 about a task it holds on an unhealthy harness (`#131`, D6). The
/// verdict itself is L3's `HarnessHealthFact`; these are the commands that go with it.
pub trait HarnessCommands: CommandPort<Level = L3> + Send + Sync {
    /// A task is now held on `binary` (the list the Infrastructure page shows).
    fn hold(&self, binary: &str, task: HeldTask);
    /// What the last recheck found still held.
    fn set_held(&self, now_held: BTreeMap<String, Vec<HeldTask>>);
    /// A reason to doubt the cached answer for `binary`: the next ask probes again.
    fn doubt(&self, binary: &str);
    /// At most one recheck per interval, and one at a time. `true` means "go ahead".
    fn claim_recheck(&self, every: Duration) -> bool;
    fn recheck_done(&self);
    /// The owner's opt-in repair: once per unhealthy stretch, in the background.
    fn auto_repair(&self, harness: &str, binary: &str, enabled: bool, script: Option<String>);
}

/// The standing-agent watchdog tick.
#[async_trait::async_trait]
pub trait Supervision: CommandPort<Level = L3> + Send + Sync {
    async fn supervise(&self);
}

/// What L4 asks L3 about an agent's environment (OpenShell sandbox), which L3 passes to L2
/// (`Commands<L3, L2Port>`): L4 never names an L2 port. The caller's `Notes` capability
/// receives what happened, at the moment it happens.
#[async_trait::async_trait]
pub trait Environments: CommandPort<Level = L3> + Send + Sync {
    async fn gate_environment(
        &self,
        key: &(String, String),
        config: &factory_environment::openshell::OpenshellConfig,
    ) -> std::result::Result<factory_environment::provision::Resolved, String>;
    async fn prepare_environment(
        &self,
        request: factory_environment::provision::PrepareRequest<'_>,
        notes: &dyn factory_environment::provision::Notes,
    ) -> Result<factory_environment::provision::Prepared>;
    async fn discard_environment(&self, plan: &factory_environment::openshell::Plan);
    /// Tear down the sandbox a closed run's session carries, in the background.
    fn release_environment(
        &self,
        session_meta: &BTreeMap<String, String>,
        task: String,
        run: String,
        notes: std::sync::Arc<dyn factory_environment::provision::Notes>,
    );
    async fn reconcile_environments(
        &self,
        ledger: &dyn factory_environment::provision::RunLedger,
        notes: &dyn factory_environment::provision::Notes,
    );
    fn forget_preserved(&self, task: &str);
    /// A run now exists: keep the host awake for it (L3 -> L2 -> L1).
    async fn keep_awake(&self, run_id: &str);
    /// The run ended, however it ended: let the host sleep again.
    async fn allow_sleep(&self, run_id: &str);
}
