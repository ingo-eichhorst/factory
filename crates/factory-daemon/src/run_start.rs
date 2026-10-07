//! L4 Process's service, the run-start path (#193 phase 6, slice S9a part 2): `start_run*`, `dispatch`,
//! `dispatch_with`, the waiting-slot releases and the dispatch-only helpers (`upstream_outputs`,
//! `agent_exit_context`, `knowledge_hints`, the run lifecycle lock).
//!
//! The moved code reaches the parts of the daemon that have not moved yet (report/finish/retry in part 3, workflows and
//! verification in S9b, quality and goals in L5/L6) through `L4Service::core`, a transitional handle on `Engine`.
//! It is used only for `Engine` methods, never for a level's state, and goes away as those callers move.
use chrono::Utc;
use factory_core::adapter::agent::{
    truncate_tail, AgentContext, AgentExitContext, TaskBinding, UpstreamOutput,
    UPSTREAM_RESULT_BYTE_CAP,
};
use factory_core::config::Sandbox;
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::run::{BlockSource, FailKind, NewRun, Run, RunPatch, RunStatus, Trigger};
use factory_core::task::{
    SlotWait, Task, TaskEntry, TaskFailure, TaskPatch, TaskStatus,
};
use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::{feedback_round_title, ContinueOutcome, Due};
use crate::l4_service::L4Service;
use factory_agents::dispatch::{Assignments, Environments};
use crate::engine::{sandbox_request, Workspace};

impl L4Service<'_> {
    /// Start one attempt at a task and hand it to an agent, due now -- what
    /// a workflow node, an agent's request and a bench attempt all mean by
    /// starting one: the moment the daemon asks is the moment it became
    /// eligible. See `start_run_due` for one that became due earlier.
    pub async fn start_run(&self, task_id: &str, trigger: Trigger) {
        self.start_run_due(task_id, trigger, Due::now()).await
    }
    /// Start one attempt at a task that became due at `due` -- a schedule's
    /// slot, a retry's backoff running out, a request that arrived before
    /// the dispatch got going. Failures here end the run rather than
    /// escaping, because nobody is waiting on the answer.
    pub async fn start_run_due(&self, task_id: &str, trigger: Trigger, due: Due) {
        self.start_run_due_inner(task_id, trigger, due, None).await
    }
    /// `factory task run --continue` (`#178`): the same dispatch path, with
    /// the run being resumed carried through to `Engine::dispatch`, which
    /// asks `resolve_continue` what to do with it. A thin wrapper rather
    /// than a new parameter on `start_run_due` itself, so its dozen other
    /// callers -- the scheduler, retries, waiting-slot releases -- need no
    /// change at all.
    pub async fn start_run_due_continue(&self, task_id: &str, due: Due, continue_from: Run) {
        self.start_run_due_inner(task_id, Trigger::Manual, due, Some(continue_from)).await
    }
    async fn start_run_due_inner(&self, task_id: &str, trigger: Trigger, due: Due, continue_from: Option<Run>) {
        // However it was asked for, an item still in intake is not started --
        // and not failed either, which is what a dispatch error below would
        // do to it (`#119`).
        if let Ok(Some(task)) = self.state.store.get(task_id).await {
            if task.status == TaskStatus::Intake {
                self.entry(
                    task_id,
                    TaskEntry::new("daemon", "intake_held", "not started: the task is still in intake"),
                )
                .await;
                return;
            }
            match self.dependency_blockers(&task).await {
                Ok(blockers) if !blockers.is_empty() => {
                    self.entry(
                        task_id,
                        TaskEntry::new(
                            "daemon",
                            "dependency_held",
                            format!("not started: waiting for {}", blockers.join(", ")),
                        ),
                    )
                    .await;
                    return;
                }
                Err(error) => {
                    tracing::warn!(task = task_id, "could not check dependencies: {error}");
                    return;
                }
                _ => {}
            }
        }
        let run = match Box::pin(self.dispatch(task_id, trigger, due, continue_from)).await {
            Ok(run) => run,
            Err(FactoryError::DispatchSuperseded(reason)) => {
                tracing::debug!(task = task_id, "dispatch superseded: {reason}");
                return;
            }
            // Blocked, not failed: `harness_gate` has already said why on
            // the task, and there is no run to close.
            Err(FactoryError::HarnessUnhealthy(reason)) => {
                tracing::warn!(task = task_id, "held on its harness: {reason}");
                self.core.record_workflow_task_state(task_id).await;
                self.core.record_bench_task_state(task_id).await;
                return;
            }
            // Held on `max_sessions` (`#179`), not failed either -- the task
            // stays `Pending`, waiting for a slot, rather than `Blocked`
            // waiting for a person. A repeat hold on the same wait (the tick
            // sweep, another trigger arriving while it already waits) keeps
            // the original `slot_wait` rather than restarting its clock or
            // spamming the journal every time.
            Err(FactoryError::CapacityHeld { agent, in_use, max }) => {
                let reason = format!("waiting for a {agent} slot ({in_use}/{max} in use)");
                if let Ok(Some(task)) = self.state.store.get(task_id).await {
                    let already_this_wait = task
                        .slot_wait
                        .as_ref()
                        .is_some_and(|w| w.agent == agent && w.scope == task.scope);
                    // The tick sweep re-tries every waiting task every few
                    // seconds; only the first hold is worth an info line.
                    if already_this_wait {
                        tracing::debug!(task = task_id, "{reason}");
                    } else {
                        tracing::info!(task = task_id, "{reason}");
                        let wait = SlotWait {
                            agent,
                            scope: task.scope.clone(),
                            trigger,
                            queued_at: due.queued_at,
                            scheduled_for: due.scheduled_for,
                            since: Utc::now(),
                        };
                        // Forced to `Pending` even from `Blocked` (a
                        // scheduled task that exhausted its retries, tried
                        // again on its next regular slot, and hit capacity):
                        // `waiting_tasks` only ever lists `Pending` rows, so
                        // a wait left sitting on `Blocked` would never be
                        // seen by a release or the tick sweep, and `fires()`
                        // already keeps the schedule from retrying it in the
                        // meantime. `failure` stays -- `TaskFailure`'s own
                        // doc note is that `Pending` with a failure still
                        // set is exactly what a queued retry already looks
                        // like, and `create_run` clears it once this
                        // actually dispatches.
                        let _ = self
                            .state.store
                            .update(
                                task_id,
                                &TaskPatch {
                                    status: Some(TaskStatus::Pending),
                                    slot_wait: Some(wait),
                                    ..Default::default()
                                },
                            )
                            .await;
                        self.entry(task_id, TaskEntry::new("daemon", "capacity_held", reason)).await;
                        self.publish_task(task_id).await;
                    }
                }
                self.core.record_workflow_task_state(task_id).await;
                self.core.record_bench_task_state(task_id).await;
                return;
            }
            Err(e) => {
                // The run may or may not exist yet; if it does, close it.
                if let Ok(Some(run)) = self.state.store.active_run(task_id).await {
                    self.fail_run(&run.id, FailKind::DispatchFailed, &format!("dispatch failed: {e}"))
                        .await;
                } else {
                    self.entry(
                        task_id,
                        TaskEntry::new("daemon", "failed", format!("dispatch failed: {e}")),
                    )
                    .await;
                    // Refused before any run existed, so there is no run
                    // to mirror: the task is blocked on the failure itself
                    // (`#122`), exactly as if a run had failed to dispatch.
                    let _ = self
                        .state.store
                        .update(
                            task_id,
                            &TaskPatch {
                                status: Some(TaskStatus::Blocked),
                                error: Some(e.to_string()),
                                clear_result: true,
                                failure: Some(TaskFailure {
                                    kind: Some(FailKind::DispatchFailed),
                                    run_id: None,
                                    attempt: None,
                                    at: Utc::now(),
                                }),
                                clear_closure: true,
                                clear_pending_retry: true,
                                ..Default::default()
                            },
                        )
                        .await;
                    self.publish_task(task_id).await;
                }
                self.core.record_workflow_task_state(task_id).await;
                self.core.record_bench_task_state(task_id).await;
                return;
            }
        };
        tracing::info!(task = task_id, run = %run.id, attempt = run.attempt, "dispatched");
        self.core.record_workflow_task_state(task_id).await;
        self.core.record_bench_task_state(task_id).await;
    }
    pub(crate) async fn dispatch(&self, task_id: &str, trigger: Trigger, due: Due, continue_from: Option<Run>) -> Result<Run> {
        Box::pin(self.dispatch_with(task_id, trigger, due, continue_from, Vec::new())).await
    }
    /// The same, with extra `UpstreamOutput` entries appended to whatever
    /// this dispatch would otherwise show -- `suggestions.rs`'s `ask_agent`
    /// (`#275`) is the one caller that needs this: the question it is
    /// asking has nowhere else to reach the resumed prompt. Every ordinary
    /// caller goes through `dispatch` above, which passes none.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn dispatch_with(
        &self,
        task_id: &str,
        trigger: Trigger,
        due: Due,
        continue_from: Option<Run>,
        extra_upstream: Vec<UpstreamOutput>,
    ) -> Result<Run> {
        let task = self.require(task_id).await?;
        let workflow_node = if let Some(origin) = &task.workflow_origin {
            self.state.workflows.get_run(&origin.workflow_run_id).await?.and_then(|workflow| {
                let node = workflow.nodes.iter().find(|node| node.node_id == origin.node_id
                    && node.task_id.as_deref() == Some(task.id.as_str()))?.clone();
                let policy = workflow.definition.nodes.iter().find(|node| node.id == origin.node_id)?.session;
                Some((node, policy))
            })
        } else { None };
        let feedback_round = workflow_node.as_ref().map_or(0, |(node, _)| node.round);
        let continue_from = if continue_from.is_none() && trigger == Trigger::Workflow && feedback_round > 0 {
            self.state.store.runs(task_id, 1).await?.into_iter().next().filter(|previous| previous.status.is_terminal())
        } else { continue_from };
        // Resolve again rather than trusting what was written down: the config
        // may have changed since the task was created.
        let (agent_name, adapter_name, declaration) =
            self.core.resolve_agent(&task.scope, &task.agent)?;
        let agent = self.wiring.registry().agent(&adapter_name)?;
        let runtime = self.wiring.registry().runtime(&task.runtime)?;
        // Before anything else exists (`#131`): a harness that does not
        // start blocks the task here, with no run row and no session, rather
        // than dispatching into a pane nobody answers and failing it as an
        // `ack_timeout` three minutes later.
        self.core.harness_gate(&task, agent.as_ref(), trigger).await?;
        let factory = self.wiring.snapshot();
        let provider_account = declaration
            .as_ref()
            .and_then(|declared| factory.config.infrastructure.provider_for(declared))
            .map(|(provider, _)| provider.name.clone())
            .or_else(|| {
                factory
                    .config
                    .infrastructure
                    .providers
                    .iter()
                    .find(|provider| provider.harnesses.iter().any(|harness| harness == &adapter_name))
                    .map(|provider| provider.name.clone())
            });
        let scope_path = factory.scope_path(&task.scope)?;
        if !scope_path.is_dir() {
            return Err(FactoryError::BadRequest(format!(
                "scope {:?} points at {}, which is not a directory",
                task.scope,
                scope_path.display()
            )));
        }

        let role_name = self.core.effective_role_for(&task.scope, &agent_name).await;
        let role = self.wiring.roles_for(&task.scope).get(&role_name).cloned();
        let policy_frameworks = factory_core::policy::frameworks_in_chain(&self.wiring.policy_chain(&task.scope));
        let goal = self.wiring.intent().goal_context(task.labels.get("goal").cloned()).await;
        let quality = self.core.quality_context(&task.scope).await;
        let probe = agent.health_probe();
        let version = probe.as_ref().and_then(|probe| self.core.harness_version_of(&adapter_name, probe));
        // Hash guide inputs, code and the observed binary version, never tokens.
        let binary_stamp = probe.as_ref().and_then(|probe| crate::harness_health::resolve(probe.program()))
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|metadata| (metadata.len(), metadata.modified().ok()));
        let guide_inputs = format!("{:?}", (
            &role, &policy_frameworks, &goal, &quality, &declaration,
            &task.runtime, &task.instructions, &task.title, (&task.scope, &scope_path),
            &task.worktree, (&version, &binary_stamp),
            include_str!("../../factory-core/src/adapter/agent.rs"),
        ));
        let fingerprint = factory_core::run::token_digest(&guide_inputs);
        let continue_outcome = match &continue_from {
            Some(_) if workflow_node.as_ref().is_some_and(|(_, policy)| *policy == factory_core::workflow::SessionPolicy::Fresh) => {
                Some(ContinueOutcome::Fresh { reason: "the workflow node declares session: fresh".into() })
            }
            Some(previous) if previous.resume_context.as_ref().is_some_and(|context| context.resumes >= 8) => Some(ContinueOutcome::Fresh {
                reason: "the eight-round conversation resume cap was reached; resetting with a fresh conversation".into(),
            }),
            Some(_) if probe.is_some() && version.is_none() => Some(ContinueOutcome::Fresh {
                reason: "the harness version is not known, so resume compatibility cannot be verified".into(),
            }),
            Some(previous) if previous.resume_context.as_ref().is_none_or(|context| context.fingerprint != fingerprint) => Some(ContinueOutcome::Fresh {
                reason: "the guide, role, agent declaration or harness version changed (or no compatibility checkpoint was recorded)".into(),
            }),
            // `#274`: the conversation lived in the previous run's sandbox,
            // deleted when that run ended -- except a sandboxed claude-code
            // run now preserves it first, so this falls through to
            // `resolve_continue` like everything else; it decides, from
            // what was actually preserved, whether resuming is possible.
            Some(prev) => Some(
                self.resolve_continue(
                    &task,
                    (&agent_name, &adapter_name),
                    agent.as_ref(),
                    runtime.as_ref(),
                    prev,
                    &scope_path,
                    declaration
                        .as_ref()
                        .filter(|d| d.sandbox == Sandbox::Openshell)
                        .and_then(|d| d.openshell.as_ref()),
                )
                .await,
            ),
            None => None,
        };

        // What `done` will need, fixed now (`#118`) -- before the run row
        // exists, so a plan that cannot be resolved fails the dispatch
        // rather than letting the work start unplanned.
        let required_steps = self.required_steps_for_task(&task, &agent_name).await?;

        // An approval-held run already exists but has never launched. Once
        // approved, resume that exact frozen snapshot instead of creating a
        // second attempt that would ask for the same approval again.
        let existing = self.state.store.active_run(task_id).await?.filter(|run| {
            run.status == RunStatus::Blocked
                && run.blocked_source == Some(BlockSource::Verification)
                && run.session.is_none()
                && run
                    .required_steps
                    .iter()
                    .any(|s| s.kind == factory_core::control_plan::StepKind::Approval)
        });
        let resumed = existing.is_some();
        let token = existing
            .as_ref()
            .and_then(|r| r.token.clone())
            .unwrap_or_else(factory_core::new_token);
        // Reserve capacity under the same lock for a fresh attempt and an
        // approved held run. An approval-held run has no session and is not
        // counted as occupying a slot until it resumes dispatch.
        let run = {
            let _admission = self.state.admission_lock.lock().await;
            let admitted_task = self.require(task_id).await?;
            if trigger == Trigger::Workflow && admitted_task.status != TaskStatus::Pending && !resumed {
                return Err(FactoryError::DispatchSuperseded("this workflow task is no longer pending admission".into()));
            }
            if trigger == Trigger::Dependency && admitted_task.runs > 0 {
                return Err(FactoryError::DispatchSuperseded("this one-shot dependency already has an attempt".into()));
            }
            if let Some(origin) = &admitted_task.workflow_origin {
                let ended = self.workflow_run(&origin.workflow_run_id).await?.status.is_terminal();
                let unadmitted = admitted_task.runs == 0 || admitted_task.closure.as_ref()
                    .is_some_and(|closure| closure.reason == factory_core::task::CloseReason::NotPlanned);
                // An approval-held run was already admitted. Ending the
                // workflow stops new automatic work, not a person's
                // decision on that frozen run or an explicit continuation.
                if ended && !resumed && (trigger != Trigger::Manual || unadmitted) {
                    return Err(FactoryError::DispatchSuperseded("the workflow ended before admission".into()));
                }
            }
            if admitted_task.after.is_some() {
                if trigger != Trigger::Dependency || admitted_task.workflow_origin.is_some()
                    || !self.dependency_blockers(&admitted_task).await?.is_empty() {
                    return Err(FactoryError::DispatchSuperseded("the upstream trigger has not been released".into()));
                }
                self.state.store.update(task_id, &TaskPatch { clear_after: true, ..Default::default() }).await?;
            }
            // The API's earlier active-run check is not an admission claim.
            // Recheck under the reservation lock before creating/launching,
            // including approval-held runs and stale continuation requests.
            if let Some(active) = self.state.store.active_run(task_id).await? {
                let held = existing.as_ref().is_some_and(|held| held.id == active.id)
                    && active.status == RunStatus::Blocked
                    && active.session.is_none();
                if !held {
                    return Err(FactoryError::DispatchSuperseded(format!(
                        "task {task_id} already has active run {}", active.id)));
                }
            } else if existing.is_some() {
                return Err(FactoryError::DispatchSuperseded("the approval-held run already ended".into()));
            }
            if let Some(previous) = &continue_from {
                if self.state.store.runs(task_id, 1).await?.first().is_none_or(|latest| latest.id != previous.id) {
                    return Err(FactoryError::DispatchSuperseded("a newer attempt replaced the requested continuation".into()));
                }
            }
            if trigger == Trigger::Workflow && !resumed
                && self.state.store.runs(task_id, 1).await?.first().is_some_and(|latest| latest.workflow_round >= feedback_round) {
                return Err(FactoryError::DispatchSuperseded("this feedback round already has an attempt".into()));
            }
            let agent_max = declaration.as_ref().and_then(|d| d.max_sessions);
            let cap = self.capacity_for(&task.scope, &agent_name, agent_max).await?;
            if cap.held() {
                let (in_use, max) = cap.holding_pair();
                return Err(FactoryError::CapacityHeld { agent: agent_name.clone(), in_use, max });
            }
            match existing {
                Some(run) => self.state.store.update_run(
                    &run.id,
                    &RunPatch { status: Some(RunStatus::Dispatching), clear_blocked: true, ..Default::default() },
                ).await?,
                None => self.state.store.create_run(&NewRun {
                    task_id: task.id.clone(),
                    trigger,
                    agent: agent_name.clone(),
                    adapter: adapter_name.clone(),
                    runtime: task.runtime.clone(),
                    token: token.clone(),
                    queued_at: Some(due.queued_at),
                    scheduled_for: due.scheduled_for,
                }).await?,
            }
        };
        if resumed && task.slot_wait.is_some() {
            self.state.store.update(task_id, &TaskPatch {
                clear_slot_wait: true,
                ..Default::default()
            }).await?;
        }
        let mut initial_patch = RunPatch {
            original_estimate: task.effective_estimate(),
            provider_account,
            workflow_round: Some(feedback_round),
            feedback: workflow_node.as_ref().and_then(|(node, _)| node.rework_request.clone()),
            ..Default::default()
        };
        // `#178`: recorded whether or not the continuation actually managed
        // to resume -- `continued_from` and the voided-token chain both hold
        // regardless, since this run is `--continue`'s answer either way;
        // `resumed_session` only when it truly resumed.
        if let Some(prev) = &continue_from {
            let mut superseded = prev.superseded_token_sha256s.clone();
            if let Some(spent) = &prev.spent_token_sha256 {
                superseded.push(spent.clone());
            }
            initial_patch.superseded_token_sha256s = superseded;
            initial_patch.continued_from = Some(prev.id.clone());
        }
        if let Some(ContinueOutcome::Resume(plan)) = &continue_outcome {
            initial_patch.resumed_session = Some(plan.session_id.clone());
        }
        let run = self.state.store.update_run(&run.id, &initial_patch).await?;
        if let Some(ContinueOutcome::Resume(plan)) = &continue_outcome {
            if plan.from_preserved {
                self.entry(
                    task_id,
                    TaskEntry::new(
                        "daemon",
                        "continue_session_source",
                        format!("resuming session {}: session id from the preserved sandbox conversation (the previous run recorded none)", plan.session_id),
                    )
                    .in_run(&run.id),
                )
                .await;
            }
        }
        if let Some(ContinueOutcome::Fresh { reason }) = &continue_outcome {
            self.entry(
                task_id,
                TaskEntry::new(
                    "daemon",
                    "continue_fallback",
                    format!("continuing attempt {}'s session was not possible: {reason}; dispatching fresh", run.attempt.saturating_sub(1)),
                )
                .in_run(&run.id),
            )
            .await;
        }

        // The run exists from here on, so it holds its share of the power
        // assertion from here on too -- every `?` below this point ends the
        // run through `start_run`'s own error handling, which reaches
        // `fail_run` (since `active_run` now finds this row) and so
        // `close_session`, the one place this is ever released. Acquired
        // before the worktree and the agent/runtime calls rather than after
        // them: those are exactly the slow, fallible steps a sleeping host
        // could stall inside, which is the case this issue is about.
        let run = if resumed || required_steps.is_empty() {
            run
        } else {
            self.state.store
                .update_run(&run.id, &RunPatch { required_steps: Some(required_steps), ..Default::default() })
                .await?
        };

        if resumed {
            self.wiring.bus().publish(Event::RunUpdated { run: run.clone() });
        } else {
            self.wiring.bus().publish(Event::RunStarted { run: run.clone() });
        }
        self.publish_task(task_id).await;
        if !resumed {
            self.entry(
                task_id,
                TaskEntry::new(
                    "daemon",
                    "started",
                    format!("attempt {} started ({})", run.attempt, trigger.as_str()),
                )
                .in_run(&run.id),
            )
            .await;
        }
        if !resumed && !run.required_steps.is_empty() {
            let steps: Vec<String> = run
                .required_steps
                .iter()
                .map(|s| match s.required_by.is_empty() {
                    true => s.step.clone(),
                    false => format!("{} ({})", s.step, s.required_by.join(", ")),
                })
                .collect();
            self.entry(
                task_id,
                TaskEntry::new(
                    "daemon",
                    "plan",
                    format!(
                        "planned as {}; done needs: {}",
                        factory_core::control_plan::effective_category(task.category.as_deref()),
                        steps.join(", ")
                    ),
                )
                .in_run(&run.id)
                .with_data(serde_json::json!({ "required_steps": run.required_steps })),
            )
            .await;
        }

        let approval_evidence = self.state.run_evidence.step_attestations(&run.id).await?;
        let pending_approval = run
            .required_steps
            .iter()
            .filter(|s| s.kind == factory_core::control_plan::StepKind::Approval)
            .find(|approval| {
                !approval_evidence
                    .iter()
                    .rev()
                    .find(|a| {
                        a.step == approval.step
                            && a.kind == factory_core::control_plan::StepKind::Approval
                    })
                    .is_some_and(|a| {
                        a.verdict == factory_core::control_plan::AttestationVerdict::Pass
                    })
            });
        if let Some(approval) = pending_approval {
                let held = self
                    .state.store
                    .update_run(
                        &run.id,
                        &RunPatch {
                            status: Some(RunStatus::Blocked),
                            blocked_since: Some(Utc::now()),
                            blocked_source: Some(BlockSource::Verification),
                            ..Default::default()
                        },
                    )
                    .await?;
                self.entry(
                    task_id,
                    TaskEntry::new(
                        "daemon",
                        "blocked",
                        format!("approval {} is required before dispatch", approval.step),
                    )
                    .in_run(&run.id)
                    .with_data(
                        serde_json::json!({ "approval": approval.step, "actor": approval.actor }),
                    ),
                )
                .await;
                self.wiring.bus().publish(Event::RunUpdated { run: held.clone() });
                self.mirror_to_task(&held).await;
                return Ok(held);
        }

        // Approval has passed (or none was required); only now does this run
        // acquire its liveness assertion and create an outward agent session.
        let l3 = crate::commands::l3(self.core);
        l3.port().keep_awake(&run.id).await;

        // A worktree of its own, made now rather than left to the harness --
        // the run row already exists, so it is named after it. Nothing below
        // this point may hand the agent the scope itself when the checkbox is
        // on: a failure here ends the run right here, with git's own
        // complaint, rather than quietly falling back to the scope. A
        // A compatible conversation carries its exact reuse hint. Otherwise
        // the workspace owner applies task lifetime independently of the
        // fresh conversation; only bench attempts are fresh per run.
        let workspace = match &continue_outcome {
            Some(ContinueOutcome::Resume(plan)) => match &plan.workspace {
                Some((path, branch)) => Workspace::Reuse { path: path.clone(), branch: branch.clone() },
                None => Workspace::Fresh,
            },
            _ => Workspace::Fresh,
        };
        let (cwd, run) = self.place_run(&task, run, &scope_path, workspace).await?;
        let resumes = if run.resumed_session.is_some() {
            continue_from.as_ref().and_then(|previous| previous.resume_context.as_ref())
                .map_or(1, |previous| previous.resumes.saturating_add(1))
        } else { 0 };
        let checkpoint = crate::resume::checkpoint(&cwd, fingerprint, resumes).await;
        let run = self.state.store.update_run(&run.id, &RunPatch {
            resume_context: Some(checkpoint.clone()), ..Default::default()
        }).await?;

        // Resolved the same way `caller_for` resolves it for every other
        // request, off the agent this run actually landed on rather than
        // whatever the task's own record says -- `resolve_agent` may have
        // fallen back to a bare adapter name the task did not ask for.

        // Direct parents only, computed now rather than when the node's task
        // was created (`create_workflow_task`) -- so a restart's recovery
        // pass, which dispatches through this same function, needs no
        // change of its own to pick this up.
        let mut upstream = self.upstream_outputs(&task).await;
        upstream.extend(extra_upstream);
        if run.resumed_session.is_some() {
            if let Some(previous) = continue_from.as_ref().and_then(|previous| previous.resume_context.as_ref()) {
                upstream.push(UpstreamOutput {
                    node_id: "resume-context".into(), task_id: task.id.clone(),
                    title: "What changed since your previous run".into(),
                    result: Some(crate::resume::summary(&cwd, previous, &checkpoint).await),
                });
            }
        }
        if feedback_round > 0 {
            let integration_rounds = workflow_node.as_ref().map_or(0, |(node, _)| node.integration_rounds);
            upstream.push(UpstreamOutput {
                node_id: "feedback-round".into(), task_id: task.id.clone(),
                title: feedback_round_title(feedback_round, integration_rounds),
                result: Some("Feedback on the prior attempt is recorded as a new run of the same task.".into()),
            });
        }
        // `#274`: what a sandboxed resume falls back to, if its preserved
        // conversation does not actually upload -- the same upstream a
        // fresh dispatch would have built, minus the one entry that only
        // makes sense for a conversation that is truly continuing.
        let upstream_fresh: Vec<UpstreamOutput> =
            upstream.iter().filter(|u| u.node_id != "resume-context").cloned().collect();
        let agent_exits = self.agent_exit_context(&task).await;
        let knowledge = self.knowledge_hints(&task, &run.id).await;
        // Same chain the L6 tab and `policy attest` fold against
        // (`Engine::policy_chain`), reduced to just the names the guide
        // names -- see `factory_core::policy::frameworks_in_chain`.
        // The one sentence the guide says about a `goal=` label -- resolved
        // once here, the same as `policy_frameworks`, never re-read once the
        // guide is built.
        // The scope's H-importance quality attributes (`#107`), judged now
        // and never again for this run -- the same once-at-dispatch rule.

        // `sandbox: openshell` (`#218`): the run's harness starts inside a
        // sandbox, so everything that names Factory to the agent -- the CLI
        // in hooks and the contract, how it reaches the daemon -- has to be
        // what exists in there, not on this host. Resolved before the
        // context is built, so the adapters need no idea it is happening.
        let openshell = match declaration.as_ref() {
            Some(declared) if declared.sandbox == Sandbox::Openshell => {
                let config = declared.openshell.clone().ok_or_else(|| {
                    FactoryError::BadRequest(format!(
                        "{agent_name} declares sandbox: openshell without an openshell: block; the run was not started on the host instead"
                    ))
                })?;
                let callback = config.callback_target(crate::facts::selected_http_bind(&factory).as_deref())?;
                Some((config, callback))
            }
            _ => None,
        };

        let ctx = AgentContext {
            scope: task.scope.clone(),
            agent_name: agent_name.clone(),
            // What the agent is told its working directory is: inside a
            // sandbox, the host path names nothing.
            cwd: match &openshell {
                Some((config, _)) => PathBuf::from(factory_core::openshell::workdir_for(config, &cwd)?),
                None => cwd.clone(),
            },
            factory_bin: match &openshell {
                Some((config, _)) => PathBuf::from(&config.factory_bin),
                None => self.wiring.factory_bin().to_path_buf(),
            },
            socket: factory.socket_path(),
            callback_url: openshell.as_ref().map(|(_, callback)| callback.url()),
            guides_dir: factory.guides_dir(),
            task: Some(TaskBinding {
                task: (&task).try_into()?,
                run_id: run.id.clone(),
                attempt: run.attempt,
                token,
                worktree_branch: run.worktree_branch.clone(),
                resumed_session: run.resumed_session.clone(),
                upstream,
                knowledge,
                required_steps: run.required_steps.clone(),
                agent_exits,
            }),
            identity_token: None,
            role,
            policy_frameworks,
            goal,
            quality,
        };

        // Process asks Agent through its command port (#193, S8a): the adapter's launch with
        // the `#178` resume args leading and the declaration's args after.
        let resume_args: Vec<String> = match &continue_outcome {
            Some(ContinueOutcome::Resume(plan)) => plan.resume_args.clone(),
            _ => Vec::new(),
        };
        let resume_args_len = resume_args.len();
        let mut launch = l3.port().launch(&adapter_name, &ctx, &resume_args, declaration.as_ref()).await?;
        // `#274`: `Some` only for a sandboxed claude-code run whose
        // preserved conversation matched this continuation -- the local
        // directory `prepare_sandbox` tries to restore before trusting
        // `launch`'s `--resume` for real.
        let sandbox_restore = match &continue_outcome {
            Some(ContinueOutcome::Resume(plan)) => plan.sandbox_restore.clone(),
            _ => None,
        };
        // A task's stored `scope` can still be a scope's legacy bare name --
        // canonicalize it the same way `start_agent` does, so a legacy-named
        // task's run lands in the same workspace as that scope's standing
        // agents rather than a second one keyed on the old name.
        let canonical_scope = factory.canonical_scope_name(&task.scope);

        // The sandbox, made now -- after the run row exists, so a failure
        // here fails this run with the reason, and before the session, so
        // nothing ever starts on the host in its place. Its launch replaces
        // the harness's own, and the prompt goes in with it: typed into a
        // TUI through a pty, a multi-line prompt would submit at its first
        // newline.
        let notes = crate::commands::EntryNotes::of(self.core);
        let sandboxed = match &openshell {
            // Its own boxed future: everything the sandbox needs lives in
            // that frame, not in this one, which is already deep.
            Some((config, callback)) => {
                // `#234`: what the daemon keeps for this agent -- its image,
                // its providers -- is ready, or the run fails with the one
                // thing it needs. Never a fallback to the host.
                let key = (factory.canonical_scope_name(&task.scope), agent_name.clone());
                let resolved = l3.port().gate_environment(&key, config).await.map_err(|reason| {
                    FactoryError::BadRequest(format!("{reason}. The run was not started on the host instead"))
                })?;
                let prompt = l3.port().prompt(&adapter_name, &ctx).await?;
                let prepared = match &sandbox_restore {
                    // `#274`: a tentative resume plans the *fresh* pair as
                    // the primary -- what the sandbox launches with unless
                    // the preserved conversation actually uploads -- and
                    // the already-built resumed `launch`/`prompt` as the
                    // overlay `prepare` switches to only on that success.
                    Some(local_dir) => {
                        let mut ctx_fresh = ctx.clone();
                        if let Some(binding) = ctx_fresh.task.as_mut() {
                            binding.resumed_session = None;
                            binding.upstream = upstream_fresh.clone();
                        }
                        let mut launch_fresh = launch.clone();
                        launch_fresh.args.drain(0..resume_args_len);
                        let prompt_fresh = l3.port().prompt(&adapter_name, &ctx_fresh).await?;
                        Box::pin(l3.port().prepare_environment(
                            sandbox_request(
                                &factory, &task, &run.id, &cwd, &launch_fresh, &prompt_fresh,
                                Some((&launch, prompt.as_str(), local_dir.as_path())),
                                config, callback, &resolved,
                            ),
                            &notes,
                        ))
                        .await?
                    }
                    None => Box::pin(l3.port().prepare_environment(
                        sandbox_request(&factory, &task, &run.id, &cwd, &launch, &prompt, None, config, callback, &resolved),
                        &notes,
                    ))
                    .await?,
                };
                let meta = prepared.session_meta();
                let (plan, restore_outcome) = (prepared.plan, prepared.restore);
                launch = plan.launch.clone();
                Some((meta, plan, restore_outcome))
            }
            None => None,
        };

        // `#274`: the run row and the sandboxed prompt/upstream already
        // tentatively claim a resume -- corrected here, the one place that
        // knows for certain, when the preserved conversation did not
        // actually make it into the new sandbox. `resumed_session` is
        // otherwise set once at dispatch and never patched again; this is
        // the one case that can discover, after the fact, that it was not
        // really true.
        if let Some((_, _, crate::openshell::RestoreOutcome::FellBack(reason))) = &sandboxed {
            let reverted = factory_core::run::ResumeContext { resumes: 0, ..checkpoint.clone() };
            self.state.store
                .update_run(
                    &run.id,
                    &RunPatch {
                        clear_resumed_session: true,
                        resume_context: Some(reverted),
                        ..Default::default()
                    },
                )
                .await?;
            self.entry(
                task_id,
                TaskEntry::new(
                    "daemon",
                    "continue_fallback",
                    format!(
                        "continuing attempt {}'s session was not possible: {reason}; dispatching fresh",
                        run.attempt.saturating_sub(1)
                    ),
                )
                .in_run(&run.id),
            )
            .await;
        }

        // A close during a slow launch must not leave a new pane attached to
        // an already-ended run. Serialize just this run's launch/report close.
        let lifecycle = self.run_lifecycle_lock(&run.id);
        let _launching = lifecycle.lock().await;
        if self.require_run(&run.id).await?.status.is_terminal() {
            if let Some((_, plan, _)) = &sandboxed { l3.port().discard_environment(plan).await; }
            return Err(FactoryError::DispatchSuperseded("the run ended before its session was launched".into()));
        }
        let started = l3
            .port()
            .start(factory_agents::dispatch::SessionStart {
                runtime: task.runtime.clone(),
                run_id: run.id.clone(),
                scope: canonical_scope.clone(),
                agent: agent_name.clone(),
                title: task.title.clone(),
                cwd,
                launch,
                // A sandbox's teardown rides on the session so `close_session` can find it.
                meta: sandboxed.as_ref().map(|(meta, _, _)| meta.clone()).unwrap_or_default(),
            })
            .await;
        let session = match (started, &sandboxed) {
            (Ok(session), _) => session,
            // No session will ever carry this sandbox to `close_session`.
            (Err(e), Some((_, plan, _))) => {
                l3.port().discard_environment(plan).await;
                return Err(e);
            }
            (Err(e), None) => return Err(e),
        };
        // `#274`: only now, with the session actually up, is the preserved
        // copy's job done -- an earlier delete would lose the only copy to
        // a failure between the restore upload and here (a failed `create`
        // retry, the launch itself failing) that this function cannot see.
        if let Some((_, _, crate::openshell::RestoreOutcome::Restored { local_dir })) = &sandboxed {
            let _ = std::fs::remove_dir_all(local_dir);
        }

        let run = self
            .state.store
            .update_run(
                &run.id,
                &RunPatch {
                    session: Some(session.clone()),
                    ..Default::default()
                },
            )
            .await?;
        self.wiring.bus().publish(Event::RunUpdated { run: run.clone() });

        // The baseline, before the task is handed over: whatever the session
        // had already used -- a pane an earlier run left behind, a harness
        // that spent tokens coming up -- is not this run's (#117). Never a
        // `?`: a runtime with no usage to give must not fail the run.
        Box::pin(self.snapshot_usage(&run, factory_core::usage::SnapshotPoint::Dispatch)).await;
        drop(_launching);
        let current = self.require_run(&run.id).await?;
        if current.status.is_terminal() { return Ok(current); }

        // A sandboxed run was handed its prompt at launch.
        if sandboxed.is_none() {
            let prompt = l3.port().prompt(&adapter_name, &ctx).await?;
            l3.port().submit(&task.runtime, &session, &prompt).await?;
        }

        self.entry(
            task_id,
            TaskEntry::new(
                "daemon",
                "dispatched",
                format!("handed to {agent_name} ({adapter_name}) on {}", session.runtime),
            )
            .in_run(&run.id)
            .with_data(serde_json::json!({ "session": session })),
        )
        .await;
        Ok(run)
    }
    /// Try to admit every waiting task whose own wait names `scope` or
    /// `agent` -- oldest first. Each attempt is an ordinary `start_run_due`,
    /// which re-checks live capacity under `admission_lock` on its own
    /// terms: one run ending frees at most what it frees, so a task that
    /// still does not fit is simply held again, at no cost beyond the check
    /// itself. `dispatch` will see every earlier admission this loop already
    /// made, so the order tried is the order admitted.
    pub(crate) async fn release_waiting(&self, scope: &str, agent: &str) {
        for task in self.waiting_tasks().await {
            let Some(wait) = task.slot_wait.clone() else { continue };
            if wait.scope != scope && wait.agent != agent {
                continue;
            }
            let due = Due { queued_at: wait.queued_at, scheduled_for: wait.scheduled_for };
            self.start_run_due(&task.id, wait.trigger, due).await;
        }
    }
    /// The scheduler tick's sweep, next to `recheck_harnesses`: try every
    /// waiting task, not just the ones a specific release names. Covers what
    /// a single `(scope, agent)` wakeup cannot -- a restart (nothing sent
    /// anything, but the wait is still on disk), a limit raised while tasks
    /// were already waiting, and a wakeup this channel dropped.
    pub(crate) async fn recheck_capacity(&self) {
        for task in self.waiting_tasks().await {
            let Some(wait) = task.slot_wait.clone() else { continue };
            let due = Due { queued_at: wait.queued_at, scheduled_for: wait.scheduled_for };
            self.start_run_due(&task.id, wait.trigger, due).await;
        }
    }
    pub(crate) fn run_lifecycle_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.state.run_lifecycle_locks.lock().expect("run lifecycle mutex");
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(id).and_then(std::sync::Weak::upgrade) { return lock; }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(id.to_string(), Arc::downgrade(&lock));
        lock
    }
    /// This task's direct dependencies -- decomposition predecessors first,
    /// then workflow parents in definition order -- with the result each
    /// finished with. A store or workflow-run read failure is logged and
    /// treated as "nothing found" rather than failing dispatch.
    pub(crate) async fn upstream_outputs(&self, task: &Task) -> Vec<UpstreamOutput> {
        let mut outputs = Vec::new();
        for parent_task_id in &task.depends_on {
            match self.state.store.get(parent_task_id).await {
                Ok(Some(parent)) => outputs.push(UpstreamOutput {
                    node_id: parent.decomposition_part.clone().unwrap_or_else(|| "dependency".into()),
                    task_id: parent.id.clone(),
                    title: parent.title.clone(),
                    result: parent
                        .result
                        .as_deref()
                        .map(|result| truncate_tail(result, UPSTREAM_RESULT_BYTE_CAP).into_owned()),
                }),
                Ok(None) => tracing::warn!(
                    task = task.id,
                    parent_task = parent_task_id,
                    "dependency task no longer exists"
                ),
                Err(error) => tracing::warn!(
                    task = task.id,
                    parent_task = parent_task_id,
                    "reading dependency output: {error}"
                ),
            }
        }
        let Some(origin) = &task.workflow_origin else {
            return outputs;
        };
        let run = match self.state.workflows.get_run(&origin.workflow_run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => {
                tracing::warn!(
                    task = task.id,
                    workflow_run = origin.workflow_run_id,
                    "workflow run not found; dispatching without upstream outputs"
                );
                return outputs;
            }
            Err(error) => {
                tracing::warn!(
                    task = task.id,
                    workflow_run = origin.workflow_run_id,
                    "reading workflow run for upstream outputs: {error}"
                );
                return outputs;
            }
        };

        for edge in run.definition.edges.iter().filter(|edge| edge.to == origin.node_id) {
            let Some(parent_task_id) = run
                .nodes
                .iter()
                .find(|node| node.node_id == edge.from)
                .and_then(|node| node.task_id.clone())
            else {
                // The parent node never got a task (denied, or the run
                // failed before it was its turn) -- nothing to report.
                continue;
            };
            // A decomposition's dependency is usually its workflow parent
            // too; say what it reported once.
            if outputs.iter().any(|output| output.task_id == parent_task_id) {
                continue;
            }
            match self.state.store.get(&parent_task_id).await {
                Ok(Some(parent)) => outputs.push(UpstreamOutput {
                    node_id: edge.from.clone(),
                    task_id: parent.id.clone(),
                    title: parent.title.clone(),
                    result: parent
                        .result
                        .as_deref()
                        .map(|r| truncate_tail(r, UPSTREAM_RESULT_BYTE_CAP).into_owned()),
                }),
                Ok(None) => tracing::warn!(
                    task = task.id,
                    parent_task = parent_task_id,
                    "parent task for upstream output no longer exists"
                ),
                Err(error) => tracing::warn!(
                    task = task.id,
                    parent_task = parent_task_id,
                    "reading parent task for upstream output: {error}"
                ),
            }
        }
        // Work sent back here comes with what the node that sent it said:
        // the result from `done --send-to`, or the error/results retained by
        // an in-flight run loaded from the legacy failure-routing format.
        let request = run
            .nodes
            .iter()
            .find(|node| node.node_id == origin.node_id && node.task_id.as_deref() == Some(task.id.as_str()))
            .and_then(|node| node.rework_request.clone());
        if let Some(request) = request {
            if let Some(feedback) = request.feedback.as_deref() {
                let sender = if request.from_node == "integration" {
                    "Integration".to_owned()
                } else {
                    run.definition.nodes.iter().find(|node| node.id == request.from_node)
                        .map(|node| node.task.title.clone()).unwrap_or_else(|| request.from_node.clone())
                };
                outputs.push(UpstreamOutput {
                    node_id: request.from_node.clone(),
                    task_id: request.from_task.clone(),
                    title: format!(
                        "{} sent this work back -- rework round {} of {}",
                        sender, request.round, request.max_rounds
                    ),
                    result: Some(truncate_tail(feedback, UPSTREAM_RESULT_BYTE_CAP).into_owned()),
                });
            } else {
                match self.state.store.get(&request.from_task).await {
                    Ok(Some(reviewer)) => {
                        let said: Vec<&str> = [reviewer.error.as_deref(), reviewer.result.as_deref()]
                            .into_iter()
                            .flatten()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .collect();
                        outputs.push(UpstreamOutput {
                            node_id: request.from_node.clone(),
                            task_id: reviewer.id.clone(),
                            title: format!(
                                "{} sent this work back -- rework round {} of {}",
                                reviewer.title, request.round, request.max_rounds
                            ),
                            result: (!said.is_empty())
                                .then(|| truncate_tail(&said.join("\n\n"), UPSTREAM_RESULT_BYTE_CAP).into_owned()),
                        });
                    }
                    Ok(None) => tracing::warn!(task = task.id, from_task = request.from_task, "rework source no longer exists"),
                    Err(error) => tracing::warn!(task = task.id, from_task = request.from_task, "reading rework source: {error}"),
                }
            }
        }
        outputs
    }
    /// Agent-selectable exits for this exact workflow-node round. This is
    /// computed once at dispatch so the reporting contract cannot change
    /// underneath a running agent.
    async fn agent_exit_context(&self, task: &Task) -> Vec<AgentExitContext> {
        let Some(origin) = &task.workflow_origin else {
            return Vec::new();
        };
        let Ok(Some(run)) = self.state.workflows.get_run(&origin.workflow_run_id).await else {
            return Vec::new();
        };
        let used = run
            .nodes
            .iter()
            .find(|node| node.node_id == origin.node_id)
            .map_or(0, |node| node.exit_rounds());
        run.definition
            .nodes
            .iter()
            .find(|node| node.id == origin.node_id)
            .into_iter()
            .flat_map(|node| &node.exits)
            .filter_map(|exit| {
                exit.agent.as_ref().map(|rule| AgentExitContext {
                    to: exit.to.clone(),
                    rule: rule.clone(),
                    max_rounds: exit.max_rounds,
                    rounds_used: used,
                })
            })
            .collect()
    }
    /// The knowledge pages a run of `task` is handed, when the task asks for
    /// them: one search over its title and instructions, from its own scope,
    /// at most `KNOWLEDGE_HINTS_LIMIT` pages. What was handed over is
    /// written to the run's journal, so the prompt can be explained after
    /// the vault has moved on. A search that fails is journaled too and the
    /// run goes ahead without hints -- they are a help, never a reason not
    /// to start.
    pub(crate) async fn knowledge_hints(
        &self,
        task: &Task,
        run_id: &str,
    ) -> Option<factory_core::adapter::KnowledgeHints> {
        if !task.knowledge_hints {
            return None;
        }
        let (root, name) = {
            let factory = self.wiring.snapshot();
            (factory.root, factory.config.daemon.knowledge_provider)
        };
        let query = factory_core::adapter::KnowledgeQuery {
            text: format!("{}\n{}", task.title, task.instructions),
            tags: Vec::new(),
            scope: Some(task.scope.clone()),
            limit: factory_core::adapter::knowledge::KNOWLEDGE_HINTS_LIMIT,
        };
        let searched = match self.wiring.registry().knowledge(&name) {
            Ok(provider) => provider.search(&root, &query).await,
            Err(e) => Err(e),
        };
        let entry = match &searched {
            Ok(hits) if hits.is_empty() => {
                TaskEntry::new("daemon", "knowledge", format!("no knowledge page matched ({name})"))
            }
            Ok(hits) => TaskEntry::new(
                "daemon",
                "knowledge",
                format!(
                    "handed {} knowledge page(s) ({name}): {}",
                    hits.len(),
                    hits.iter().map(|h| h.page.as_str()).collect::<Vec<_>>().join(", ")
                ),
            )
            .with_data(serde_json::json!({ "provider": name, "hits": hits })),
            Err(e) => TaskEntry::new(
                "daemon",
                "knowledge",
                format!("knowledge search failed, so this run has no hints ({name}): {e}"),
            ),
        };
        self.entry(&task.id, entry.in_run(run_id)).await;
        let hits = searched.ok().filter(|hits| !hits.is_empty())?;
        Some(factory_core::adapter::KnowledgeHints {
            vault: factory_core::knowledge::vault_root(&root).display().to_string(),
            hits,
        })
    }
}
