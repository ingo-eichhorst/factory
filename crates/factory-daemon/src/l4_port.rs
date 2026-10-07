//! L4's command port for L5, as `factory-daemon` serves it (#193 phase 6, S10).
//!
//! `L4Port` is what a `Commands<L5, _>` wraps: the daemon's L4 service behind the few commands the Improvement
//! level gives the Process level. L5 reads task and run records as facts (`TaskSnapshotFact`, `RunSnapshotFact`);
//! everything it makes L4 *do* goes through here, so no L5 file names L4 state or an L4 service.
//!
//! **Transitional shape.** Several methods take or return the L4 record types (`Run`, `ContinueOutcome`) because the
//! suggestion ask (S10) still orchestrates a resumed run from L5; the end state is one `ask` command that returns an
//! acknowledgement. They are listed here so each is a visible thing to replace, and `l5_service.rs` is the only
//! caller.
use crate::engine::{ContinueOutcome, Due};
use crate::l4_service::L4Service;
use factory_core::adapter::{Agent, AgentRuntime};
use factory_core::adapter::agent::UpstreamOutput;
use factory_core::error::Result;
use factory_core::run::{FailKind, Run, Trigger};
use factory_core::task::{Task, TaskEntry};
use factory_assurance::remediation::RemediationCommands;
use factory_kernel::{CommandPort, L4};
use std::path::Path;

pub(crate) struct L4Port<'a>(pub(crate) L4Service<'a>);

impl CommandPort for L4Port<'_> {
    type Level = L4;
}

impl L4Port<'_> {
    /// A run token must match the run it is presented for.
    pub(crate) fn check_run_token(&self, run: &Run, given: Option<&str>, task_id: &str) -> Result<()> {
        self.0.check_run_token(run, given, task_id)
    }

    /// Cancel a task's run in progress (the one named, when `expected` is given).
    pub(crate) async fn cancel_task_run(&self, task_id: &str, expected: Option<&str>, kind: FailKind) -> Result<Run> {
        self.0.cancel_task_run(task_id, expected, kind).await
    }

    /// Whether a previous run's conversation can be resumed, and how.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn resolve_continue(
        &self,
        task: &Task,
        names: (&str, &str),
        agent: &dyn Agent,
        runtime: &dyn AgentRuntime,
        prev: &Run,
        scope_path: &Path,
        sandboxed: Option<&factory_core::openshell::OpenshellConfig>,
    ) -> ContinueOutcome {
        self.0.resolve_continue(task, names, agent, runtime, prev, scope_path, sandboxed).await
    }

    /// Dispatch a run of the task, carrying extra upstream notes (the ask question rides in this way).
    pub(crate) async fn dispatch_with(
        &self,
        task_id: &str,
        trigger: Trigger,
        due: Due,
        continue_from: Option<Run>,
        extra_upstream: Vec<UpstreamOutput>,
    ) -> Result<Run> {
        Box::pin(self.0.dispatch_with(task_id, trigger, due, continue_from, extra_upstream)).await
    }

    /// A line in a task's journal.
    pub(crate) async fn journal(&self, task_id: &str, entry: TaskEntry) {
        self.0.entry(task_id, entry).await
    }

    /// Create the improver's remediation task for an intent, through L5's creation command into L4. Returns the
    /// task's id.
    pub(crate) async fn create_remediation(
        &self,
        intent: factory_assurance::remediation::Intent,
    ) -> Result<String> {
        let observer = crate::commands::CreationObserver(self.0.wiring.bus().clone());
        let snapshot = self.0.wiring.snapshot();
        let receipt = crate::commands::assurance_with(
            self.0.state.store.as_ref(),
            self.0.wiring.registry(),
            &self.0.state.workflows,
            &snapshot,
            &observer,
        )
        .remediate(intent)
        .await?;
        Ok(receipt.id)
    }

    /// Create a bench attempt's task, born with its bench origin and the id L5 already recorded for it.
    pub(crate) async fn create_bench_task(
        &self,
        new: factory_core::task::NewTask,
        origin: factory_core::bench::BenchOrigin,
        id: String,
    ) -> Result<Task> {
        self.0.create_bench_task(new, origin, id).await
    }

    /// Start a run of the task, awaited until the dispatch has been handled.
    pub(crate) async fn start_run(&self, task_id: &str, trigger: Trigger) {
        self.0.start_run(task_id, trigger).await
    }

    /// Release the workspaces recorded for these paths (their worktrees are gone).
    pub(crate) async fn release_workspaces(&self, paths: &[std::path::PathBuf]) {
        let _ = crate::assignments::release(&self.0.state.workspaces, paths).await;
    }
}
