//! L4 Process's service, the run's settling path (#193 phase 6, slice S9a part 3): `report`, `cancel_task_run`,
//! `finish_run`, the retry settlement, `fail_run`, `mirror_to_task`, `close_session`, `place_run` and the L4 half of
//! `resolve_continue` (the preserved-conversation reads).
//!
//! What the moved code still needs from `Engine` (workflows, verification, bench and deployment settling, in S9b and
//! S10) it reaches through `L4Service::core`; `level_ownership_tests` counts those call sites and lets them only shrink.
use chrono::Utc;
use factory_core::adapter::agent::{
    knowledge_hints_path, run_guide_path, run_hook_settings_path, run_shell_script_path,
    upstream_output_path,
};
use factory_core::adapter::runtime::RuntimeStatus;
use factory_core::adapter::{Agent, AgentRuntime};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::run::{BlockSource, FailKind, Run, RunPatch, RunStatus, RunTaskStatus};
use factory_core::task::{
    PendingRetry, RetryPolicy, Task, TaskEntry, TaskFailure, TaskPatch,
    TaskReport, TaskStatus,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::{preserved_conversation_has, ContinueOutcome, ResumePlan, Workspace};
use crate::l4_service::L4Service;
use crate::worktree;
use factory_agents::dispatch::Environments;

impl L4Service<'_> {
    /// Whether a continuation or feedback round's (`#178`) previous run can
    /// really be resumed, and with what. Every fallback below ends in
    /// `ContinueOutcome::Fresh`, never an error: a `--continue` that cannot
    /// continue still dispatches, exactly as a plain `task run` would,
    /// journaled with the one reason it fell back rather than left to a
    /// person to guess from a session that just looks fresh.
    /// `pub(crate)`: `suggestions.rs`'s `ask_agent` (`#275`) asks this
    /// directly, before deciding whether to dispatch at all.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn resolve_continue(
        &self,
        task: &Task,
        (agent_name, adapter_name): (&str, &str),
        agent: &dyn Agent,
        runtime: &dyn AgentRuntime,
        prev: &Run,
        scope_path: &Path,
        sandboxed: Option<&factory_core::openshell::OpenshellConfig>,
    ) -> ContinueOutcome {
        // Rule 5: the previous run's agent is not necessarily this
        // dispatch's -- the task may have been edited since. Resuming a
        // `codex` conversation with `claude-code`'s launch args makes no
        // sense, so this is checked before anything else.
        if prev.agent != agent_name || prev.adapter != adapter_name {
            return ContinueOutcome::Fresh {
                reason: format!(
                    "the task's agent changed since the previous run (was {}/{}, now {agent_name}/{adapter_name})",
                    prev.agent, prev.adapter
                ),
            };
        }

        // The previous run's newest usage snapshot entry for this adapter,
        // or -- failing that -- the session id a `turn-ended` hook recorded
        // on it. Never "the latest session in this directory" (rule 6).
        let snapshots = self.state.store.usage_snapshots(&prev.id).await.unwrap_or_default();
        let mut session_id = factory_core::usage::newest_session_id_for_adapter(&snapshots, adapter_name)
            .or_else(|| prev.turn_ended_session_id.clone());
        // `#274`: a sandboxed herdr run has no usage snapshots, and a run
        // that timed out mid-turn never fired its `turn-ended` hook -- yet
        // that is exactly the infrastructure failure `--continue` exists
        // for. The preserve step did read the id off the conversation it
        // saved, so take it from there, but only when that copy provably
        // belongs to `prev`: the record names this run, and the
        // `<session_id>.jsonl` it names is really in the preserved tree.
        // This is not rule 6's "latest session in this directory": a run's
        // sandbox HOME belongs to that run alone, so the one conversation
        // in it is that run's, never another's.
        let mut from_preserved = false;
        if session_id.is_none() && sandboxed.is_some() {
            let sessions_root = self.wiring.snapshot().factory_dir().join("openshell").join("sessions");
            session_id = crate::openshell::load_preserved(&sessions_root, &task.id)
                .filter(|preserved| preserved.run == prev.id)
                .and_then(|preserved| preserved.session_id)
                .filter(|id| preserved_conversation_has(&crate::openshell::preserved_dir(&sessions_root, &task.id), id));
            from_preserved = session_id.is_some();
        }
        let Some(session_id) = session_id else {
            return ContinueOutcome::Fresh { reason: "no session id was recorded for the previous run".into() };
        };

        let Some(resume) = agent.resume_spec(&session_id) else {
            return ContinueOutcome::Fresh { reason: format!("the {adapter_name} adapter declares no resume") };
        };

        let workspace = if task.worktree {
            match &prev.worktree_path {
                Some(path) if worktree::is_registered(scope_path, Path::new(path)).await => {
                    let Some(branch) = &prev.worktree_branch else {
                        return ContinueOutcome::Fresh {
                            reason: "the previous run's worktree branch was not recorded".into(),
                        };
                    };
                    if !crate::resume::on_branch(Path::new(path), branch).await {
                        return ContinueOutcome::Fresh {
                            reason: "the previous run's worktree is no longer on its recorded branch".into(),
                        };
                    }
                    Some((PathBuf::from(path), branch.clone()))
                }
                _ => {
                    return ContinueOutcome::Fresh { reason: "the previous run's worktree is gone".into() };
                }
            }
        } else {
            None
        };

        // Rule 1: never run two processes on one conversation. An error
        // asking is treated the same as "not confirmed gone" -- silence is
        // not the same as a `Gone` answer.
        let confirmed_gone = match &prev.last_session {
            Some(session) => matches!(runtime.status(session).await, Ok(RuntimeStatus::Gone)),
            None => false,
        };
        if !confirmed_gone {
            return ContinueOutcome::Fresh {
                reason: "the previous run's session is not confirmed gone".into(),
            };
        }

        // `#274`: a sandboxed run's conversation does not live on this host
        // at all -- it lived in the previous run's sandbox, deleted when
        // that run ended -- so resuming needs a preserved copy that matches
        // this exact continuation in every way a live, on-host conversation
        // never has to prove: the right task, the right run, the same
        // session id this function just resolved, and the same sandbox
        // working directory the new dispatch would use. `prepare_sandbox`
        // still has to actually upload it; this only says the attempt is
        // worth making.
        let sandbox_restore = if let Some(config) = sandboxed {
            let sessions_root = self.wiring.snapshot().factory_dir().join("openshell").join("sessions");
            let Some(preserved) = crate::openshell::load_preserved(&sessions_root, &task.id) else {
                return ContinueOutcome::Fresh {
                    reason: "no preserved sandbox conversation was found for this task".into(),
                };
            };
            if preserved.run != prev.id {
                return ContinueOutcome::Fresh {
                    reason: "the preserved sandbox conversation came from an earlier run, not the one being continued".into(),
                };
            }
            if preserved.bytes > crate::openshell::PRESERVE_CAP_BYTES {
                return ContinueOutcome::Fresh {
                    reason: format!(
                        "the preserved sandbox conversation was {} bytes, over the {}-byte cap, so only its size was kept",
                        preserved.bytes, crate::openshell::PRESERVE_CAP_BYTES
                    ),
                };
            }
            if preserved.session_id.as_deref() != Some(session_id.as_str()) {
                return ContinueOutcome::Fresh {
                    reason: "the preserved sandbox conversation's session id does not match the previous run's resolved session id".into(),
                };
            }
            let predicted_cwd = workspace.as_ref().map(|(path, _)| path.as_path()).unwrap_or(scope_path);
            let expected_workdir = match factory_core::openshell::workdir_for(config, predicted_cwd) {
                Ok(workdir) => workdir,
                Err(e) => {
                    return ContinueOutcome::Fresh {
                        reason: format!("the sandbox working directory could not be resolved to check the preserved conversation: {e}"),
                    };
                }
            };
            if preserved.workdir != expected_workdir {
                return ContinueOutcome::Fresh {
                    reason: "the preserved sandbox conversation was captured from a different sandbox working directory".into(),
                };
            }
            Some(crate::openshell::preserved_dir(&sessions_root, &task.id))
        } else {
            None
        };

        ContinueOutcome::Resume(ResumePlan { session_id, resume_args: resume.args, workspace, sandbox_restore, from_preserved })
    }
    /// Where a run actually works: its own worktree, or the scope directly.
    /// Pulled out of `dispatch` so the decision -- and the one way it can
    /// fail -- has no need of a real agent or runtime on the other end of it,
    /// which is what lets it be tested on its own.
    ///
    /// `task.worktree` off is the whole of the "quietly ignored" case this
    /// function refuses to have: it is checked once, here, and every path out
    /// of it either returns the scope path unchanged or a worktree that
    /// `git worktree add` actually made (`Workspace::Fresh`) or that a
    /// previous run already made and the task lifetime (`#178`) is reusing
    /// (`Workspace::Reuse`). There is no other path.
    pub(crate) async fn place_run(&self, task: &Task, run: Run, scope_path: &Path, workspace: Workspace) -> Result<(PathBuf, Run)> {
        if !task.worktree {
            return Ok((scope_path.to_path_buf(), run));
        }
        // A fresh harness conversation does not discard the task's files.
        // Bench attempts remain explicitly run-scoped and reset only on creation.
        let previous = match workspace {
            Workspace::Reuse { path, branch } => Some((path, branch)),
            Workspace::Fresh if task.bench_origin.is_none() => {
                let mut previous = None;
                for attempt in self.state.store.runs(&task.id, u32::MAX).await?.into_iter().filter(|attempt| attempt.id != run.id) {
                    if let (Some(path), Some(branch)) = (attempt.worktree_path, attempt.worktree_branch) {
                        if Path::new(&path).exists() && crate::resume::on_branch(Path::new(&path), &branch).await {
                            previous = Some((PathBuf::from(path), branch)); break;
                        }
                    }
                }
                previous
            },
            Workspace::Fresh => None,
        };
        if let Some((path, _)) = &previous {
            self.require_workspace_quiet(&task.id, path, Some(&run.id)).await?;
        }
        let branch = worktree::branch_name(&task.id, &task.title, run.attempt);
        let dir = self.wiring.snapshot().worktrees_dir().join(&run.id);
        let base = match &task.bench_origin {
            Some(origin) => self.core.bench_case_base(origin).await?,
            None => task
                .workflow_origin
                .as_ref()
                .and_then(|origin| origin.workspace.as_ref())
                .map(|workspace| workspace.base_ref.clone()),
        };
        let assignment = crate::assignments::AssignmentWorkspace {
            spec: factory_kernel::WorkspaceSpec {
                task_id: task.id.clone(),
                workflow_run_id: task.workflow_origin.as_ref().map(|origin| origin.workflow_run_id.clone()),
                lifetime: if task.bench_origin.is_some() { factory_kernel::WorkspaceLifetime::Run } else { factory_kernel::WorkspaceLifetime::Task },
            },
            candidate: dir, branch, base, previous,
        };
        let (placed, reused) = assignment.provision(&self.state.workspaces, scope_path).await
            .map_err(|e| FactoryError::adapter("workspace", e))?;
        if reused { self.require_workspace_quiet(&task.id, &placed.path, Some(&run.id)).await?; }
        let dir = placed.path;
        let branch = placed.branch;
        let run = self
            .state.store
            .update_run(
                &run.id,
                &RunPatch {
                    worktree_path: Some(dir.display().to_string()),
                    worktree_branch: Some(branch.clone()),
                    ..Default::default()
                },
            )
            .await?;
        self.wiring.bus().publish(Event::RunUpdated { run: run.clone() });
        self.entry(
            &run.task_id,
            TaskEntry::new(
                "daemon",
                "worktree",
                format!("{} in {} on {branch}", if reused { "reusing task workspace" } else { "working" }, dir.display()),
            )
            .in_run(&run.id),
        )
        .await;

        // A bench case is never run dirty: its reset command, when it has
        // one, runs in the fresh worktree before the agent is handed
        // anything. A non-zero exit ends the run here -- `fail_run` gives it
        // the daemon's usual terminal handling, and `sync_bench_for_task`
        // reads this exact "reset failed" prefix back off `run.error` to
        // settle the attempt `skipped` rather than `error`, without ever
        // dispatching the agent.
        if let Some(origin) = &task.bench_origin {
            if reused { return Ok((dir, run)); }
            if let Some(reset) = self.core.bench_case_reset(origin).await? {
                if let Err(detail) = self.core.run_bench_reset(&dir, &reset).await {
                    return Err(FactoryError::BadRequest(format!("reset failed: {detail}")));
                }
            }
        }
        Ok((dir, run))
    }
    /// The token is what makes a call about a run come from that run --
    /// the agent's own report, or its harness's turn-end hook -- rather than
    /// from anyone on the socket closing anyone's run.
    pub(crate) fn check_run_token(&self, run: &Run, given: Option<&str>, task_id: &str) -> Result<()> {
        let Some(expected) = &run.token else {
            return Ok(());
        };
        match given {
            Some(given) if given == expected => Ok(()),
            // `#178`: a stale token this exact run's `--continue` replaced,
            // told apart from a plain wrong one -- see `caller_for`'s own
            // copy of this check. Only the digest is ever compared.
            Some(given)
                if run
                    .superseded_token_sha256s
                    .iter()
                    .any(|d| d == &factory_core::run::token_digest(given)) =>
            {
                Err(FactoryError::Denied(format!(
                    "a newer run of task {task_id} exists; use the latest reporting commands"
                )))
            }
            Some(_) => Err(FactoryError::Denied(format!(
                "wrong token for attempt {} of task {task_id}",
                run.attempt
            ))),
            None => Err(FactoryError::Denied(format!(
                "attempt {} of task {task_id} needs its run token; it is FACTORY_TASK_TOKEN in the session, or pass --run-token",
                run.attempt
            ))),
        }
    }
    /// What an agent says about its own run. The token is what makes this a
    /// report rather than anyone on the socket closing anyone's run.
    pub async fn report(&self, task_id: &str, report: TaskReport) -> Result<Run> {
        let run = self.state.store.active_run(task_id).await?.ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "task {task_id} has no run in progress; reports are no longer accepted"
            ))
        })?;

        self.check_run_token(&run, report.token.as_deref(), task_id)?;
        let reporting_task = self.require(task_id).await?;
        if !report.artifacts.is_empty() && report.status != Some(RunStatus::Done) {
            return Err(FactoryError::BadRequest("artifacts may only be attached to a done report".into()));
        }
        let review_report = reporting_task
            .labels
            .contains_key(crate::verification::REVIEW_RUN_LABEL);
        if !review_report {
            self.core.validate_workflow_send_to(task_id, report.status, report.send_to.as_deref())
                .await?;
        } else if report.send_to.is_some() && report.status != Some(RunStatus::Done) {
            return Err(FactoryError::BadRequest(
                "a review rejects only with status done and --send-to".into(),
            ));
        }

        // While its gates run, a run's status is the verifier's to set. The
        // agent may still add a note, or give the run up; nothing else.
        if run.status == RunStatus::Verifying
            && report.status.is_some_and(|s| !matches!(s, RunStatus::Failed | RunStatus::Cancelled))
        {
            return Err(FactoryError::BadRequest(format!(
                "attempt {} of task {task_id} is being verified -- its required steps are running; \
                 the run becomes done or blocked when they finish",
                run.attempt
            )));
        }
        if report.status == Some(RunStatus::Verifying) {
            return Err(FactoryError::BadRequest(
                "verifying is the daemon's to set; report done and it verifies the run".into(),
            ));
        }

        let artifacts = if report.artifacts.is_empty() {
            None
        } else {
            Some(self.capture_artifacts(&run, &reporting_task, &report.artifacts).await?)
        };

        let message = report.message.clone().unwrap_or_else(|| {
            report
                .result
                .clone()
                .or_else(|| report.error.clone())
                .unwrap_or_else(|| "(no message)".into())
        });

        self.entry(
            task_id,
            TaskEntry::new(
                "agent",
                report
                    .status
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_else(|| "note".into()),
                message,
            )
            .in_run(&run.id),
        )
        .await;

        let mut patch = RunPatch {
            artifacts,
            status: report.status,
            result: report.result,
            routed_to: report.send_to.clone(),
            clear_routed_to: report.status == Some(RunStatus::Done) && report.send_to.is_none(),
            error: report.error,
            // The agent is talking, so whatever turn a `Stop` hook said had
            // ended has not -- see `occupancy::settle_turn_end`.
            clear_turn_ended: true,
            ..Default::default()
        };

        if let Some(to) = &report.send_to {
            self.entry(
                task_id,
                TaskEntry::new(
                    "agent",
                    "routed_to",
                    format!("reported done; routed to {to}"),
                )
                .in_run(&run.id)
                .with_data(serde_json::json!({ "routed_to": to })),
            )
            .await;
        }

        // The agent's own report is the one thing that may set or clear
        // `Blocked` honestly for its own sake -- see `AGENTS.md` and issue
        // #7. Reporting `blocked` again while already blocked leaves
        // `blocked_since` alone, so the clock still reads from when the
        // block actually began; reporting anything else always lets go of
        // it, agent-set or not, because this report is the agent speaking.
        // The agent ending its own run is the one terminal report that says
        // why in so many words.
        patch.fail_kind = match report.status {
            Some(RunStatus::Failed) => Some(FailKind::AgentFailed),
            Some(RunStatus::Cancelled) => Some(FailKind::CancelledByAgent),
            _ => None,
        };

        match report.status {
            Some(RunStatus::Blocked) => {
                patch.blocked_source = Some(BlockSource::Agent);
                if run.status != RunStatus::Blocked {
                    patch.blocked_since = Some(Utc::now());
                }
            }
            Some(_) => patch.clear_blocked = true,
            None => {}
        };

        let updated = match report.status {
            // `#118`'s done gate: a run held to required steps is not done on
            // its agent's word. The session stays open -- see
            // `verification.rs`.
            Some(RunStatus::Done) if !run.required_steps.is_empty() => {
                self.core.begin_verification(&run, patch).await?
            }
            Some(status) if status.is_terminal() => {
                if status != RunStatus::Done {
                    self.close_session(&run).await;
                }
                self.finish_run(&run.id, status, patch, &format!("attempt {} ended", run.attempt))
                .await?
            }
            _ => {
                let run = self.state.store.update_run(&run.id, &patch).await?;
                self.wiring.bus().publish(Event::RunUpdated { run: run.clone() });
                self.mirror_to_task(&run).await;
                run
            }
        };
        if review_report && report.status == Some(RunStatus::Done) {
            self.core.settle_review_task(&reporting_task, &updated).await?;
        }
        Ok(updated)
    }
    /// Cancel a task's active run. `kind` says on whose word -- a person, an
    /// agent, or the workflow or bench run above it -- and is recorded on
    /// the run; the journal line stays the same for all three.
    pub(crate) async fn cancel_task_run(&self, task_id: &str, expected: Option<&str>, kind: FailKind) -> Result<Run> {
        let run = self.state.store.active_run(task_id).await?.ok_or_else(|| {
            FactoryError::BadRequest(format!("task {task_id} has no run to cancel"))
        })?;
        // The caller named the attempt it saw. Anything else running now --
        // a retry that started since, a manual run -- is not what it asked
        // to end.
        if let Some(expected) = expected.filter(|e| *e != run.id) {
            return Err(FactoryError::BadRequest(format!(
                "run {expected} is no longer the task's active run (attempt {} is, {}); nothing was cancelled",
                run.attempt,
                run.status.as_str()
            )));
        }
        self.close_session(&run).await;
        self.finish_run(
            &run.id,
            RunStatus::Cancelled,
            RunPatch {
                status: Some(RunStatus::Cancelled),
                fail_kind: Some(kind),
                ..Default::default()
            },
            "cancelled by request",
        ).await
    }
    /// End a run and settle the task behind it. A task with a schedule goes
    /// back to `pending` so the scheduler will pick it up again; one without
    /// keeps the run's own outcome.
    pub(crate) async fn finish_run(
        &self,
        run_id: &str,
        status: RunStatus,
        patch: RunPatch,
        _why: &str,
    ) -> Result<Run> {
        let lifecycle = self.run_lifecycle_lock(run_id);
        let _closing = lifecycle.lock().await;
        if self.require_run(run_id).await?.status.is_terminal() {
            return Err(FactoryError::BadRequest("the run already ended".into()));
        }
        let ended_at = Utc::now();
        if status == RunStatus::Done {
            let mut candidate = self.require_run(run_id).await?;
            if candidate.status.is_terminal() {
                return Err(FactoryError::BadRequest("the run already ended; artifact publication cannot complete it again".into()));
            }
            if let Some(artifacts) = &patch.artifacts {
                candidate.artifacts = artifacts.clone();
            }
            if let Err(error) = self.publish_artifacts(&candidate, ended_at).await {
                // Verification owns this hold. A failed publication must
                // never leave a stopped coordinator stuck in `verifying`.
                if candidate.status == RunStatus::Verifying && self.require_run(run_id).await?.status == RunStatus::Verifying {
                    let blocked = self
                        .state.store
                        .update_run(
                            run_id,
                            &RunPatch {
                                status: Some(RunStatus::Blocked),
                                blocked_since: Some(Utc::now()),
                                blocked_source: Some(BlockSource::Verification),
                                ..Default::default()
                            },
                        )
                        .await?;
                    self.entry(&candidate.task_id, TaskEntry::new("daemon", "blocked",
                        format!("artifact provenance: {error}. Rebuild and report done with --artifact again, or cancel the run.")).in_run(run_id)).await;
                    self.wiring.bus().publish(Event::RunUpdated {
                        run: blocked.clone(),
                    });
                    self.mirror_to_task(&blocked).await;
                }
                return Err(error);
            }
            // Hashing/copy validation can take time. A cancel that landed
            // during it remains authoritative; unpublished rows stay hidden.
            if self.require_run(run_id).await?.status != candidate.status {
                return Err(FactoryError::BadRequest("the run changed while publishing artifacts; completion was not recorded".into()));
            }
            self.close_session(&candidate).await;
        }
        if status != RunStatus::Done {
            // A cancel may have read the row before its slow runtime.start.
            // Close the current session, not that earlier session-less copy.
            let candidate = self.require_run(run_id).await?;
            if candidate.session.is_some() { self.close_session(&candidate).await; }
        }
        // `#178`: what `token` is about to lose to `clear_token` below,
        // carried forward as a digest (`spent_token_sha256`) -- never the
        // token itself, which is read back off the store rather than
        // trusted from a caller only long enough to hash it, since nothing
        // that reaches `finish_run` is handed the run it is closing. A later
        // `--continue`'s new run copies the digest into
        // `superseded_token_sha256s`, which is the only thing that ever
        // reads it; a digest match authorizes nothing.
        let spent_token_sha256 = self
            .state.store
            .get_run(run_id)
            .await
            .ok()
            .flatten()
            .and_then(|r| r.token)
            .map(|t| factory_core::run::token_digest(&t));
        let resume_context = match self.state.store.get_run(run_id).await? {
            Some(previous) if previous.resume_context.is_some() => {
                let context = previous.resume_context.as_ref().expect("checked context");
                let directory = if let Some(path) = previous.worktree_path.as_ref() {
                    Some(PathBuf::from(path))
                } else {
                    self.state.store.get(&previous.task_id).await?.and_then(|task|
                        self.wiring.snapshot().scope_path(&task.scope).ok())
                };
                match directory {
                    Some(directory) => Some(crate::resume::checkpoint(
                        &directory, context.fingerprint.clone(), context.resumes).await),
                    None => Some(context.clone()),
                }
            }
            _ => None,
        };
        let run = self
            .state.store
            .update_run(
                run_id,
                &RunPatch {
                    status: Some(status),
                    clear_session: true,
                    clear_token: true,
                    spent_token_sha256,
                    resume_context,
                    // A run that has ended is not waiting on anybody, so the
                    // block's own clock and the runtime's standing guess both
                    // go with the session -- `blocked_since` is documented to
                    // be `None` whenever the status is not `Blocked`, and a
                    // finished run is the one path that could otherwise leave
                    // it set. Forced here rather than left to `patch`: every
                    // terminal status comes through this function, and only
                    // the agent's own report remembered to clear it.
                    clear_blocked: true,
                    clear_block_suspicion: true,
                    // Likewise a held `Stop` turn end: nothing is left to
                    // settle once the run is over.
                    clear_turn_ended: true,
                    ended_at: Some(ended_at),
                    ..patch
                },
            )
            .await?;
        self.wiring.bus().publish(Event::RunUpdated { run: run.clone() });
        // The run's session is released here, so nothing will poll it again.
        // Close its liveness span now or the chart draws the agent as still
        // working, forever.
        if let Ok(Some(task)) = self.state.store.get(&run.task_id).await {
            self.record_gone(&format!("run:{}", run.id), &task.scope, &run.agent)
                .await;
            // A slot just freed: wake whatever is waiting on this (scope,
            // agent) -- `close_session`, called just before this by every
            // caller, only closes the pane; the run is still non-terminal
            // (and so still counted as using a slot) until this write lands,
            // which is why the wakeup goes here and not there (`#179`).
            self.enqueue_capacity_release(&task.scope, &run.agent);
        }
        self.mirror_to_task(&run).await;
        self.settle_retry(&run).await;
        self.core.settle_run_deployments(&run).await;
        self.core.settle_suggestion_ask(&run).await;
        self.sweep_workspaces().await;
        if status != RunStatus::Done {
            if let Ok(Some(task)) = self.state.store.get(&run.task_id).await {
                if let Some(subject) = task.labels.get(crate::verification::REVIEW_RUN_LABEL) {
                    self.core.enqueue_verification(subject);
                }
            }
        }
        Ok(run)
    }
    /// After the task's mirror is updated, decide what a scheduled task's
    /// retry state should be. Only a task with a `schedule` retries at all --
    /// a one-off task that fails is left blocked on the failure by
    /// `mirror_to_task` (`#122`), and nothing here touches it.
    ///
    /// A run that finished by succeeding or by being cancelled ends any
    /// retry streak in progress: `pending_retry` is the mirror `AGENTS.md`
    /// warns about, and leaving it set past the run it describes would
    /// misreport a task that just succeeded as still mid-retry, or -- worse
    /// -- let a later, unrelated failure resume counting from someone else's
    /// streak instead of starting its own. A run that failed either queues
    /// the next retry, if the effective policy allows one, or lets the
    /// streak end where it stands.
    async fn settle_retry(&self, run: &Run) {
        let Ok(Some(task)) = self.state.store.get(&run.task_id).await else {
            return;
        };
        if task.schedule.is_none() {
            return;
        }

        match run.status {
            RunStatus::Failed => self.queue_or_end_retry(&task, run).await,
            status if status.is_terminal() => {
                // `Failed` is handled above, so this is `Done` or
                // `Cancelled` -- either way, if a streak was in progress it
                // is over now.
                if let Some(pending) = &task.pending_retry {
                    let why = if status == RunStatus::Done {
                        "a retry succeeded"
                    } else {
                        "the pending retry was cancelled"
                    };
                    self.end_retry_streak(&task, pending.resume_at, why).await;
                }
            }
            _ => {}
        }
    }
    /// A scheduled task's run just failed. Either queue its next retry --
    /// moving `next_run_at` to `now + backoff` without disturbing the regular
    /// firing this streak is standing in front of -- or, if the policy
    /// forbids retrying at all or this streak has used up its attempts, let
    /// it end and fall back to that regular firing.
    async fn queue_or_end_retry(&self, task: &Task, run: &Run) {
        let policy = task
            .retry
            .unwrap_or(self.wiring.snapshot().config.daemon.default_retry);
        let (max_attempts, backoff_seconds) = match policy {
            RetryPolicy::None => {
                match &task.pending_retry {
                    // A streak already in progress when the policy changes to
                    // `none` (an edit landed mid-streak) is honoured
                    // immediately rather than firing the retry it no longer
                    // wants.
                    Some(pending) => {
                        self.end_retry_streak(task, pending.resume_at, "the task's retry policy is now `none`")
                            .await;
                    }
                    // The ordinary case: no retry was ever queued, so
                    // `mirror_to_task` deliberately left this task without a
                    // "what happens next" line -- see its own comment on why
                    // it stays silent for a `Failed` recurring task -- and
                    // this is the one place that knows the answer: nothing
                    // will try again before its next regular firing.
                    None => {}
                }
                self.block_on_failure(&task.id, &format!("attempt {} failed and the retry policy is `none`", run.attempt))
                    .await;
                return;
            }
            RetryPolicy::Backoff { max_attempts, backoff_seconds } => (max_attempts, backoff_seconds),
        };

        // The regular firing this streak must never disturb. Captured once,
        // the first time a run in this streak fails, from `next_run_at` as
        // `advance_schedule` (or, mid-streak, `resume_from_retry`) had
        // already left it before this attempt was dispatched -- every later
        // failure in the same streak carries it forward from `pending_retry`
        // rather than reading `next_run_at` again, which by then holds an
        // earlier retry time, not the regular slot.
        let resume_at = task
            .pending_retry
            .as_ref()
            .map(|p| p.resume_at)
            .or(task.next_run_at)
            .unwrap_or_else(Utc::now);
        let attempts = task.pending_retry.as_ref().map(|p| p.attempts).unwrap_or(0) + 1;

        if attempts > max_attempts {
            let why = format!(
                "retries exhausted after {max_attempts} attempt{}",
                if max_attempts == 1 { "" } else { "s" }
            );
            self.end_retry_streak(task, resume_at, &why).await;
            self.block_on_failure(&task.id, &why).await;
            return;
        }

        let retry_at = Utc::now() + chrono::Duration::seconds(backoff_seconds as i64);
        self.entry(
            &task.id,
            TaskEntry::new(
                "daemon",
                "retrying",
                format!(
                    "attempt {} failed; retrying at {} (retry {attempts} of {max_attempts}), \
                     without touching the regular firing at {}",
                    run.attempt,
                    retry_at.to_rfc3339(),
                    resume_at.to_rfc3339(),
                ),
            )
            .in_run(&run.id),
        )
        .await;

        // Back to `Pending` from the block `mirror_to_task` left it in: a
        // retry is coming, and the task shows that it is retrying.
        let patch = TaskPatch {
            status: Some(TaskStatus::Pending),
            next_run_at: Some(retry_at),
            pending_retry: Some(PendingRetry { attempts, resume_at }),
            ..Default::default()
        };
        if let Ok(updated) = self.state.store.update(&task.id, &patch).await {
            self.wiring.bus().publish(Event::TaskUpdated { task: updated });
        }
    }
    /// End a retry streak: clear `pending_retry` and restore `next_run_at` to
    /// the regular firing the streak was standing in front of. A no-op patch
    /// is avoided when there was nothing to clear -- the common case, a task
    /// whose very first failure got no retry at all, in which `next_run_at`
    /// already holds `resume_at` untouched and `pending_retry` is already
    /// `None`.
    async fn end_retry_streak(&self, task: &Task, resume_at: chrono::DateTime<Utc>, why: &str) {
        if task.pending_retry.is_none() && task.next_run_at == Some(resume_at) {
            return;
        }
        self.entry(
            &task.id,
            TaskEntry::new(
                "daemon",
                "retry_settled",
                format!("{why}; resuming the regular schedule at {}", resume_at.to_rfc3339()),
            ),
        )
        .await;

        let patch = TaskPatch {
            next_run_at: Some(resume_at),
            clear_pending_retry: true,
            ..Default::default()
        };
        if let Ok(updated) = self.state.store.update(&task.id, &patch).await {
            self.wiring.bus().publish(Event::TaskUpdated { task: updated });
        }
    }
    /// The task's own row carries the latest run's outcome, so a list does not
    /// have to read every run.
    ///
    /// `pub(crate)`: `occupancy::record_run_liveness` mirrors a run it just
    /// moved into or out of `Blocked` the same way `report` does here --
    /// the same pattern as `record_gone`, which already crosses this
    /// boundary the other way.
    pub(crate) async fn mirror_to_task(&self, run: &Run) {
        let recurring = self
            .state.store
            .get(&run.task_id)
            .await
            .ok()
            .flatten()
            .and_then(|t| t.schedule)
            .is_some();

        // A failed run blocks its task, recurring or not (`#122`). For a
        // recurring one that is the safe side of what `settle_retry` --
        // which `finish_run` calls right after this -- decides next: a
        // queued retry puts it back to `Pending` (`queue_or_end_retry`), and
        // anything else leaves it blocked. A crash in between leaves it
        // blocked, never in Scheduled looking healthy.
        let status = if run.status.is_terminal() && recurring && run.status != RunStatus::Failed {
            TaskStatus::Pending
        } else {
            run.status.as_task_status()
        };

        // A `Failed` recurring task does not necessarily get its next real
        // turn next -- `finish_run` calls `settle_retry` right after this,
        // and that may queue a retry sooner than the schedule's own next
        // slot. Saying "waiting for its next turn" here and then, a moment
        // later, "retrying in 5 minutes" would leave the journal
        // contradicting itself on every retry, so this stays silent for
        // `Failed` and leaves the "what happens next" line to whichever of
        // `queue_or_end_retry` or `end_retry_streak` actually decides it.
        if run.status.is_terminal() && recurring && run.status != RunStatus::Failed {
            self.entry(
                &run.task_id,
                TaskEntry::new(
                    "daemon",
                    "rearmed",
                    "recurring task is pending again, waiting for its next turn",
                ),
            )
            .await;
        }

        // The task mirrors the newest run, not the union of every run: an
        // attempt that succeeded must not leave the previous one's error
        // standing next to its own result -- nor its failure, nor a close
        // record from before it ran (`#122`).
        let failed = run.status == RunStatus::Failed;
        let patch = TaskPatch {
            status: Some(status),
            result: run.result.clone(),
            routed_to: run.routed_to.clone(),
            clear_result: run.result.is_none(),
            clear_routed_to: run.routed_to.is_none(),
            error: run.error.clone(),
            clear_error: run.error.is_none(),
            failure: failed.then(|| TaskFailure {
                kind: run.fail_kind,
                run_id: Some(run.id.clone()),
                attempt: Some(run.attempt),
                at: run.ended_at.unwrap_or_else(Utc::now),
            }),
            clear_failure: !failed,
            clear_closure: true,
            ..Default::default()
        };
        if let Ok(task) = self.state.store.update(&run.task_id, &patch).await {
            self.wiring.bus().publish(Event::TaskUpdated { task });
        }
    }
    /// A scheduled task whose failure nothing will retry stays blocked on it
    /// (`#122`) -- `mirror_to_task` already put it there -- and this says
    /// so. Its schedule keeps firing: the intent still stands, and the next
    /// run that succeeds clears the block. It is never left in Scheduled
    /// looking like a task that works.
    async fn block_on_failure(&self, task_id: &str, why: &str) {
        self.entry(
            task_id,
            TaskEntry::new(
                "daemon",
                "blocked_on_failure",
                format!("{why}; blocked until someone looks -- the schedule keeps firing, and a run that succeeds clears it"),
            ),
        )
        .await;
    }
    /// A run that will never report back. `kind` is the reason as a fact --
    /// every caller has to name one, which is what keeps "why do runs fail"
    /// answerable without reading `why`'s prose back apart.
    pub async fn fail_run(&self, run_id: &str, kind: FailKind, why: &str) {
        let Ok(run) = self.require_run(run_id).await else {
            return;
        };
        if run.status.is_terminal() { return; }
        self.close_session(&run).await;
        self.entry(
            &run.task_id,
            TaskEntry::new("daemon", "failed", why.to_string()).in_run(run_id),
        )
        .await;
        let _ = self
            .finish_run(
                run_id,
                RunStatus::Failed,
                RunPatch {
                    error: Some(why.to_string()),
                    fail_kind: Some(kind),
                    ..Default::default()
                },
                why,
            )
            .await;
        self.core.record_workflow_task_state(&run.task_id).await;
        self.core.record_bench_task_state(&run.task_id).await;
    }
    /// Keep the last of what the agent saw, then let the session go. Every
    /// path that ends a run -- a terminal report, a cancel, a task deleted
    /// out from under an active run, or the watchdog giving up on it --
    /// comes through here, which is what makes this the one place that
    /// actually knows a run is over rather than guessing from one caller's
    /// reason for closing it.
    pub(crate) async fn close_session(&self, run: &Run) {
        // First and unconditional, ahead of the early `return` below for a
        // run that never got as far as a session (a dispatch failure) --
        // `power.release` is the counterpart to `dispatch`'s own
        // `power.acquire`, and every run that reaches this function reaches
        // it regardless of whether it ever had a session to close.
        crate::commands::l3(self.core).port().allow_sleep(&run.id).await;

        // The guide file, if this run's harness wrote one, is named after the
        // task rather than the run and nothing else removes it. It cannot be
        // deleted right after launch: a harness may read its configured
        // instruction file after launch, not only at startup (opencode
        // resolves instruction paths from config, and claude may read the
        // file after `herdr agent start` returns), so the file has to outlive
        // the launch and is removed only once the run is over. One that
        // carried the guide as inline text or not at all (`codex`, `shell`)
        // never wrote a file, so this is a harmless no-op for those, and a
        // run whose session never even came up still gets whatever
        // `launch_spec` managed to write before it failed cleaned up.
        let guide = run_guide_path(&self.wiring.snapshot().guides_dir(), &run.task_id);
        let _ = std::fs::remove_file(guide);
        // Same story for the upstream-outputs file (`ShellAgent` writes it
        // and exports its path as `FACTORY_UPSTREAM_FILE`; a harness agent
        // never writes one at all, since it renders the same data inline
        // instead): named after the task, nothing else removes it, harmless
        // to remove when this run never wrote one.
        let upstream = upstream_output_path(&self.wiring.snapshot().guides_dir(), &run.task_id);
        let _ = std::fs::remove_file(upstream);
        // And the knowledge-hints file, the same way for the same reader.
        let knowledge = knowledge_hints_path(&self.wiring.snapshot().guides_dir(), &run.task_id);
        let _ = std::fs::remove_file(knowledge);
        // The shell agent's generated wrapper script, keyed by *run* id
        // rather than task id (see `run_shell_script_path`'s own comment) --
        // a retry's fresh run must never lose its script to this cleanup of
        // an earlier attempt's. Unlinking a file the pane's shell is still
        // sourcing is safe on Unix: the shell holds the file open, so
        // removing the directory entry does not disturb it, and a shell
        // reads a sourced file's content in rather than re-opening it line
        // by line, so there is no window where this could cut a run off
        // mid-script.
        let script = run_shell_script_path(&self.wiring.snapshot().guides_dir(), &run.id);
        let _ = std::fs::remove_file(script);
        // And the harness settings file carrying a `claude` run's turn-end
        // hooks, keyed by run id for the same reason. Claude Code has
        // already read it at startup; a hook that fires as the session
        // closes runs from what it loaded then, and finds no run to end.
        let hooks = run_hook_settings_path(&self.wiring.snapshot().guides_dir(), &run.id);
        let _ = std::fs::remove_file(hooks);

        let Some(session) = &run.session else { return };
        if let Ok(runtime) = self.wiring.registry().runtime(&session.runtime) {
            if let Ok(text) = runtime.read(session, 400).await {
                if !text.trim().is_empty() {
                    self.entry(
                        &run.task_id,
                        TaskEntry::new("daemon", "transcript", "final terminal output")
                            .in_run(&run.id)
                            .with_data(serde_json::json!({ "text": text })),
                    )
                    .await;
                }
            }
            // The last reading, while the session is still there to ask.
            Box::pin(self.snapshot_usage(run, factory_core::usage::SnapshotPoint::RunEnd)).await;
            // `#178`: a failed stop used to be silently discarded here (`let
            // _ =`); journaled instead, since `resolve_continue`'s "never
            // run two processes on one conversation" check relies on the
            // runtime's own word that this session is gone, and a stop that
            // did not take is exactly what would make that check right to
            // refuse a later `--continue`.
            if let Err(e) = runtime.stop(session).await {
                self.entry(
                    &run.task_id,
                    TaskEntry::new("daemon", "stop_failed", format!("could not stop the session: {e}"))
                        .in_run(&run.id),
                )
                .await;
            }
        }
        // `sandbox: openshell` (`#218`): after the pane is gone, so nothing
        // is still writing in the sandbox while its tree comes back -- and
        // off this path, in the background. Every caller of `close_session`
        // writes the run's terminal status only after it returns; a
        // download and a delete taking seconds in between would leave a run
        // that already reported `done` looking active with its pane gone,
        // which the watchdog fails as `session_gone`. Claimed per sandbox,
        // so a run closed twice (a report racing a cancel) is torn down once.
        crate::commands::l3(self.core).port().release_environment(
            &session.meta,
            run.task_id.clone(),
            run.id.clone(),
            Arc::new(crate::commands::EntryNotes::of(self.core)),
        );
    }
    /// The daemon's own http bind, when it serves the http interface at
    /// all -- what a sandboxed run reports back to.
    /// `#274`: a task's preserved sandbox conversation, when it has one,
    /// is released the same moment its other per-task state is -- closing
    /// or deleting the task, never a periodic sweep of its own.
    pub(crate) fn remove_preserved_session(&self, task_id: &str) {
        crate::commands::l3(self.core).port().forget_preserved(task_id);
    }
}
