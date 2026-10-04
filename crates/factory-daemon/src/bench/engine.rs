//! Starting, advancing, judging, cancelling, cleaning and recovering bench
//! runs. Follows `workflows/engine.rs`'s own shape throughout: one attempt is
//! one ordinary Factory task, `bench_edit` serializes a run's own
//! read-modify-write the way `workflow_edit` does, and recovery reconciles
//! persisted decisions rather than replaying anything.
//!
//! **Judging an attempt never trusts a live call site's own idea of what
//! happened.** `sync_bench_for_task` reads the settled `Task`/`Run` back off
//! the store and derives the verdict from that alone -- whether the agent
//! itself reported (a `TaskEntry` with `source: "agent"` proves it), what
//! `run.error` says, and whether the case has a gate. That is what makes it
//! safe to call from four different places (a report, a cancel, a dispatch
//! failure, and restart recovery) and safe to call twice: the moment an
//! attempt already carries a verdict, every call after the first is a no-op.

use crate::engine::Engine;
use crate::worktree;
use chrono::Utc;
use factory_core::bench::{
    derive_config, tail_4kib, BenchAttempt, BenchOrigin, BenchRun, BenchRunStatus, Verdict,
};
use factory_core::dataset::{Case, Dataset};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::run::{Run, RunStatus, Trigger};
use factory_core::task::{NewTask, Task, TaskEntry, TaskStatus};
use std::path::Path;
use std::sync::Arc;

fn missing(kind: &str, id: &str) -> FactoryError {
    FactoryError::BadRequest(format!("no such {kind}: {id}"))
}

/// 10 minutes -- the reset timeout the issue names, and the gate's own
/// fallback when a case sets no `timeout_seconds`. The same number
/// `#118`'s required steps fall back to, from the same shared runner.
const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = crate::verification::DEFAULT_GATE_TIMEOUT_SECS;

use crate::verification::run_shell_capture;

/// Resolve `rev` to the full 40-hex commit SHA it names in `scope_path`, or
/// `None` when it does not name one there -- an unknown revision, or `rev`
/// itself not even shaped like one. Checked here as well as at write time
/// (`Case::validate`'s `is_git_revish`): a hand-edited dataset file never
/// goes through `validate()` at all, and `--end-of-options` on top of that
/// keeps `git` from ever reading `rev` as a flag (a `base` of `--detach`
/// would otherwise be parsed as one) even if both checks were somehow
/// skipped. Recording the resolved SHA rather than `rev` itself is what
/// lets `worktree::create` hand `git` a value it can never mistake for
/// anything but a commit.
async fn resolve_commit(scope_path: &Path, rev: &str) -> Option<String> {
    if !factory_core::dataset::is_git_revish(rev) {
        return None;
    }
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(scope_path)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{rev}^{{commit}}"))
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

impl Engine {
    // -- starting ------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn start_bench_run(
        self: &Arc<Self>,
        dataset_name: &str,
        agents: Vec<String>,
        attempts_per_case: u32,
        concurrency: u32,
        case_ids: Option<Vec<String>>,
    ) -> Result<BenchRun> {
        crate::datasets::refuse_bad_name(dataset_name)?;
        if agents.is_empty() {
            return Err(FactoryError::BadRequest(
                "a bench run needs at least one --agent".into(),
            ));
        }
        let attempts_per_case = attempts_per_case.max(1);
        let concurrency = concurrency.max(1);

        let factory = self.factory_snapshot();
        let dataset = Dataset::load(&factory.datasets_dir(), dataset_name)?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such dataset: {dataset_name:?}")))?;

        let cases: Vec<Case> = match &case_ids {
            None => dataset.cases.clone(),
            Some(ids) => {
                let mut picked = Vec::with_capacity(ids.len());
                for id in ids {
                    let case = dataset
                        .cases
                        .iter()
                        .find(|c| &c.id == id)
                        .ok_or_else(|| {
                            FactoryError::BadRequest(format!(
                                "dataset {dataset_name:?} has no case {id:?}"
                            ))
                        })?;
                    picked.push(case.clone());
                }
                picked
            }
        };
        if cases.is_empty() {
            return Err(FactoryError::BadRequest(
                "this bench run would attempt no cases at all".into(),
            ));
        }

        // Resolved once, now, to the full commit SHA: every agent's attempt
        // at a case must start from the same commit, so a case without its
        // own `base` gets the scope's HEAD pinned here rather than re-read
        // at each attempt's own dispatch, and `git` downstream only ever
        // sees a SHA, never a case-supplied string. A scope that is not
        // configured at all is left out of both maps -- `resolve_agent`
        // refuses those attempts later, by name, with a message specific to
        // the missing scope. A scope that *is* reachable but whose `base`
        // (given, or the implied `HEAD`) does not resolve to a commit there
        // never dispatches at all: recorded in `unresolved` below, and every
        // attempt at that case is marked `skipped` instead of pending.
        let mut case_bases = std::collections::BTreeMap::new();
        let mut unresolved: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
        for case in &cases {
            let Ok(scope_path) = factory.scope_path(&case.scope) else {
                continue;
            };
            let wanted = case.base.as_deref().unwrap_or("HEAD");
            match resolve_commit(&scope_path, wanted).await {
                Some(sha) => {
                    case_bases.insert(case.id.clone(), sha);
                }
                None => {
                    unresolved.insert(
                        case.id.clone(),
                        format!(
                            "base {wanted:?} does not resolve to a commit in scope {:?}",
                            case.scope
                        ),
                    );
                }
            }
        }

        let now = Utc::now();
        let mut attempts = Vec::new();
        for case in &cases {
            for agent in &agents {
                for n in 1..=attempts_per_case {
                    let mut attempt = BenchAttempt::pending(
                        uuid::Uuid::new_v4().to_string(),
                        case.id.clone(),
                        agent.clone(),
                        n,
                    );
                    if let Some(reason) = unresolved.get(&case.id) {
                        attempt.verdict = Some(Verdict::Skipped);
                        attempt.reason = Some(reason.clone());
                        attempt.started_at = Some(now);
                        attempt.ended_at = Some(now);
                    }
                    attempts.push(attempt);
                }
            }
        }

        let run = BenchRun {
            id: uuid::Uuid::new_v4().to_string(),
            dataset: dataset.name.clone(),
            dataset_revision: dataset.revision,
            cases,
            case_bases,
            agents,
            attempts_per_case,
            concurrency,
            status: BenchRunStatus::Running,
            attempts,
            started_at: Utc::now(),
            ended_at: None,
        };
        self.bench.put_run(&run).await?;
        for attempt in &run.attempts {
            self.bench.put_attempt(&run.id, attempt).await?;
        }
        self.bus.publish(Event::BenchRunUpdated { run: run.clone() });

        self.advance_bench_run(&run.id).await?;
        self.bench
            .get_run(&run.id)
            .await?
            .ok_or_else(|| missing("bench run", &run.id))
    }

    // -- advancing -------------------------------------------------------

    /// Start as many pending attempts as `concurrency` still allows. Called
    /// after a run starts, after any attempt settles, and by recovery.
    pub(crate) async fn advance_bench_run(self: &Arc<Self>, run_id: &str) -> Result<()> {
        let mut to_start = Vec::new();
        {
            let _guard = self.bench_edit.lock().await;
            let Some(mut run) = self.bench.get_run(run_id).await? else {
                return Ok(());
            };
            if run.status != BenchRunStatus::Running {
                return Ok(());
            }

            let in_flight = run.attempts.iter().filter(|a| a.is_in_flight()).count();
            let mut slots = (run.concurrency as usize).saturating_sub(in_flight);

            let cases = run.cases.clone();
            let dataset = run.dataset.clone();
            for attempt in &mut run.attempts {
                if slots == 0 {
                    break;
                }
                if !attempt.is_pending() {
                    continue;
                }
                let Some(case) = cases.iter().find(|c| c.id == attempt.case_id) else {
                    attempt.verdict = Some(Verdict::Error);
                    attempt.reason = Some("the case no longer exists in this run's snapshot".into());
                    attempt.ended_at = Some(Utc::now());
                    continue;
                };
                match self.resolve_agent(&case.scope, &attempt.agent) {
                    Ok((agent_name, adapter_name, declaration)) => {
                        let full_args = declaration.as_ref().map(|d| d.args.clone()).unwrap_or_default();
                        let sandbox = declaration
                            .as_ref()
                            .map(|d| d.sandbox.as_str().to_string())
                            .unwrap_or_else(|| "none".to_string());
                        let config = derive_config(&adapter_name, &full_args, &sandbox);
                        let task_id = uuid::Uuid::new_v4().to_string();
                        attempt.task_id = Some(task_id.clone());
                        attempt.started_at = Some(Utc::now());
                        attempt.config = Some(config);
                        slots -= 1;
                        to_start.push((
                            task_id,
                            case.clone(),
                            agent_name,
                            attempt.attempt,
                            dataset.clone(),
                        ));
                    }
                    Err(e) => {
                        attempt.verdict = Some(Verdict::Skipped);
                        attempt.reason = Some(format!(
                            "agent {:?} does not resolve in scope {:?}: {e}",
                            attempt.agent, case.scope
                        ));
                        attempt.ended_at = Some(Utc::now());
                    }
                }
            }
            for attempt in &run.attempts {
                self.bench.put_attempt(run_id, attempt).await?;
            }
            if run.attempts.iter().all(|a| a.verdict.is_some()) && run.status == BenchRunStatus::Running {
                run.status = BenchRunStatus::Done;
                run.ended_at = Some(Utc::now());
            }
            self.bench.put_run(&run).await?;
            self.bus.publish(Event::BenchRunUpdated { run: run.clone() });
        }

        for (task_id, case, agent_name, attempt_n, dataset) in to_start {
            let origin = BenchOrigin {
                bench_run_id: run_id.to_string(),
                case_id: case.id.clone(),
                agent: agent_name.clone(),
                attempt: attempt_n,
            };
            let new = NewTask {
                title: format!("bench {dataset}/{} · {agent_name} · #{attempt_n}", case.id),
                instructions: case.instructions.clone(),
                scope: Some(case.scope.clone()),
                agent: Some(agent_name),
                timeout_seconds: case.timeout_seconds,
                worktree: Some(true),
                ..Default::default()
            };
            let engine = self.clone();
            let run_id = run_id.to_string();
            let task_id_for_dispatch = task_id.clone();
            tokio::spawn(async move {
                match engine.create_bench_task(new, origin, task_id.clone()).await {
                    Ok(_) => engine.start_run(&task_id_for_dispatch, Trigger::Bench).await,
                    Err(e) => {
                        tracing::warn!(task = task_id, "could not create bench task: {e}");
                        engine.mark_bench_attempt_uncreated(&run_id, &task_id, &e.to_string()).await;
                    }
                }
            });
        }
        Ok(())
    }

    /// The rare case where the task itself could never be created (the store
    /// refused it, an id collision, and so on) -- settle the attempt directly
    /// rather than leaving it "in flight" against a task that will never
    /// exist.
    ///
    /// Deliberately does not itself call `advance_bench_run`: this runs
    /// inside a task `advance_bench_run` spawned, so awaiting -- or spawning
    /// and awaiting -- a call back into it here would ask the compiler to
    /// resolve `advance_bench_run`'s opaque future type in terms of itself, a
    /// cycle `rustc` refuses outright (E0391) rather than get wrong. The
    /// scheduler's own periodic sweep (`scheduler.rs`) is what moves this run
    /// forward again; this failure is rare enough that waiting one tick for
    /// it is the honest cost of not fighting the type system over it.
    async fn mark_bench_attempt_uncreated(self: &Arc<Self>, run_id: &str, task_id: &str, reason: &str) {
        let _guard = self.bench_edit.lock().await;
        let Ok(Some(mut run)) = self.bench.get_run(run_id).await else { return };
        if let Some(attempt) = run
            .attempts
            .iter_mut()
            .find(|a| a.task_id.as_deref() == Some(task_id))
        {
            attempt.verdict = Some(Verdict::Error);
            attempt.reason = Some(format!("could not create the task: {reason}"));
            attempt.ended_at = Some(Utc::now());
            let _ = self.bench.put_attempt(run_id, attempt).await;
            self.bus.publish(Event::BenchRunUpdated { run: run.clone() });
        }
    }

    // -- judging -----------------------------------------------------------

    /// Where a run's own worktree base is pinned, for `place_run`. `None`
    /// when this is not a bench task, or the run is gone.
    pub(crate) async fn bench_case_base(&self, origin: &factory_core::task::OriginRef) -> Result<Option<String>> {
        let origin = BenchOrigin::try_from(origin).map_err(|error| {
            FactoryError::BadRequest(format!("unsupported or invalid benchmark origin: {error}"))
        })?;
        let Some(run) = self.bench.get_run(&origin.bench_run_id).await.ok().flatten() else { return Ok(None) };
        Ok(run.case_bases.get(&origin.case_id).cloned())
    }

    /// The case's reset command, for `place_run` to run before the agent
    /// starts.
    pub(crate) async fn bench_case_reset(&self, origin: &factory_core::task::OriginRef) -> Result<Option<String>> {
        let origin = BenchOrigin::try_from(origin).map_err(|error| {
            FactoryError::BadRequest(format!("unsupported or invalid benchmark origin: {error}"))
        })?;
        let Some(run) = self.bench.get_run(&origin.bench_run_id).await.ok().flatten() else { return Ok(None) };
        Ok(run.cases.iter().find(|c| c.id == origin.case_id).and_then(|c| c.reset.clone()))
    }

    /// Run a case's reset command in its fresh worktree. `Ok(())` only on
    /// exit 0; anything else -- non-zero, a timeout, a process that never
    /// started -- is `Err`, and `place_run` turns that into a run failure
    /// whose `error` this module later reads back as `skipped` rather than
    /// `error`.
    pub(crate) async fn run_bench_reset(&self, dir: &Path, command: &str) -> std::result::Result<(), String> {
        let (exit_code, output) = run_shell_capture(dir, command, DEFAULT_COMMAND_TIMEOUT_SECS).await;
        match exit_code {
            Some(0) => Ok(()),
            Some(code) => Err(format!("exit {code}: {}", tail_4kib(&output))),
            None => Err(output),
        }
    }

    /// Enqueue `task_id` for the judge worker (`spawn_bench_judge`) to look
    /// at once its task's run has settled. Judging -- which means running
    /// the case's own `gate`, for up to its own timeout or the ten-minute
    /// default -- must never run on the caller's own path: this used to
    /// call `judge_bench_attempt` directly and was split from
    /// `sync_bench_for_task` only to dodge an `E0391` compiler cycle
    /// through `advance_bench_run`'s spawned continuations. Enqueueing
    /// instead of calling removes the cycle at its root (nothing here calls
    /// back into `advance_bench_run`'s own opaque future type at all), so
    /// the split is kept only for the two names' separate call-site
    /// histories -- both do exactly the same thing now.
    ///
    /// A no-op when the task carries no `bench_origin`, or when it is
    /// already enqueued or being judged -- which is what makes calling this
    /// (or `sync_bench_for_task`) more than once for the same settle safe,
    /// without ever running the same case's gate command twice.
    pub(crate) async fn record_bench_task_state(self: &Arc<Self>, task_id: &str) {
        self.enqueue_bench_judgement(task_id).await;
    }

    /// The same enqueue. Kept as its own name for the call sites that used
    /// to need it to also advance the run afterward (`TaskReport`,
    /// `TaskCancel`, restart recovery) -- the worker does that unconditionally
    /// now, for every enqueue, so there is nothing left for this to do that
    /// `record_bench_task_state` does not.
    pub(crate) async fn sync_bench_for_task(self: &Arc<Self>, task_id: &str) {
        self.enqueue_bench_judgement(task_id).await;
    }

    async fn enqueue_bench_judgement(&self, task_id: &str) {
        let Ok(Some(task)) = self.store.get(task_id).await else { return };
        if task.bench_origin.is_none() {
            return;
        }
        {
            let mut judging = self.bench_judging.lock().unwrap();
            if !judging.insert(task_id.to_string()) {
                return; // already queued, or the worker is on it right now
            }
        }
        let _ = self.bench_judge_tx.send(task_id.to_string());
    }

    /// The judge worker: the one place a bench attempt's gate actually
    /// runs. Started once, at daemon startup (`main.rs`, right before
    /// `recover_bench_runs`), and processes one task id at a time for the
    /// life of the daemon -- sequential on purpose, so two enqueues of the
    /// same task id can never run its gate twice (the second sees the first
    /// attempt's verdict already set and is a fast no-op in
    /// `judge_bench_attempt`), at the cost of one slow gate delaying every
    /// other bench run's judgement behind it in the queue. A second call
    /// (there should never be one) is a no-op: the receiver is taken once,
    /// the first time, and `None` after.
    pub fn spawn_bench_judge(self: &Arc<Self>) {
        let Some(mut rx) = self.bench_judge_rx.lock().unwrap().take() else {
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(task_id) = rx.recv().await {
                if let Ok(Some(task)) = engine.store.get(&task_id).await {
                    if let Some(origin) = task.bench_origin.clone() {
                        let origin = match BenchOrigin::try_from(&origin) {
                            Ok(origin) => origin,
                            Err(error) => {
                                tracing::warn!(task = %task_id, "could not decode bench origin: {error}");
                                engine.bench_judging.lock().unwrap().remove(&task_id);
                                continue;
                            }
                        };
                        if let Err(e) = engine.judge_bench_attempt(&origin, &task).await {
                            tracing::warn!(task = %task_id, "could not judge bench attempt: {e}");
                        }
                        let _ = engine.advance_bench_run(&origin.bench_run_id).await;
                    }
                }
                engine.bench_judging.lock().unwrap().remove(&task_id);
            }
        });
    }

    async fn judge_bench_attempt(self: &Arc<Self>, origin: &BenchOrigin, task: &Task) -> Result<()> {
        let Some(run) = self.bench.get_run(&origin.bench_run_id).await? else {
            return Ok(());
        };
        let Some(idx) = run.attempts.iter().position(|a| {
            a.case_id == origin.case_id && a.agent == origin.agent && a.attempt == origin.attempt
        }) else {
            return Ok(());
        };
        if run.attempts[idx].verdict.is_some() {
            return Ok(()); // already settled -- the idempotency this exists for
        }
        // A local copy, not a borrow of `run` -- the gate below can run for
        // minutes, and `finish_bench_judgement` re-reads the run's own
        // current state itself rather than trusting this snapshot to still
        // be true by the time it writes anything back. See its own doc
        // comment for why.
        let mut attempt = run.attempts[idx].clone();

        let now = Utc::now();
        let task_run = self.store.runs(&task.id, 1).await?.into_iter().next();

        let Some(task_run) = task_run else {
            // Dispatch never even created a run row: `resolve_agent`, the
            // registry, or the scope-path check refused before
            // `store.create_run` was ever called.
            // Settled, not merely closed: a dispatch that failed leaves the
            // task blocked on that failure (`#122`), and that is the end of
            // this attempt.
            if !task.is_settled() {
                return Ok(());
            }
            attempt.verdict = Some(if task.status == TaskStatus::Cancelled {
                Verdict::Cancelled
            } else {
                Verdict::Error
            });
            attempt.reason = task
                .error
                .clone()
                .or_else(|| Some("dispatch never started".to_string()));
            attempt.started_at = Some(task.created_at);
            attempt.ended_at = Some(now);
            attempt.wall_clock_seconds = Some((now - task.created_at).num_seconds());
            return self.finish_bench_judgement(&origin.bench_run_id, task, attempt).await;
        };

        if !task_run.status.is_terminal() {
            return Ok(());
        }

        let ended_at = task_run.ended_at.unwrap_or(now);
        attempt.run_id = Some(task_run.id.clone());
        attempt.started_at = Some(task_run.started_at);
        attempt.ended_at = Some(ended_at);
        attempt.wall_clock_seconds = Some((ended_at - task_run.started_at).num_seconds());

        let case = run.cases.iter().find(|c| c.id == origin.case_id).cloned();

        match task_run.status {
            RunStatus::Cancelled => {
                attempt.verdict = Some(Verdict::Cancelled);
            }
            RunStatus::Done => {
                attempt.reported = Some("done".to_string());
                self.judge_with_gate_or_unverified(&mut attempt, case.as_ref(), &task_run).await;
            }
            RunStatus::Failed => {
                if self.was_reported_by_agent(&task_run.id).await {
                    attempt.reported = Some("failed".to_string());
                    self.judge_with_gate_or_unverified(&mut attempt, case.as_ref(), &task_run).await;
                } else {
                    let error = task_run.error.clone().unwrap_or_default();
                    if error.contains("reset failed") {
                        attempt.verdict = Some(Verdict::Skipped);
                        attempt.reason = Some(error);
                    } else {
                        attempt.verdict = Some(Verdict::Error);
                        attempt.reason = Some(if error.is_empty() {
                            "the run ended before any report came back".to_string()
                        } else {
                            error
                        });
                    }
                }
            }
            RunStatus::Dispatching | RunStatus::Running | RunStatus::Blocked | RunStatus::Verifying => {
                unreachable!("checked terminal above")
            }
        }

        self.finish_bench_judgement(&origin.bench_run_id, task, attempt).await
    }

    /// Whether the agent itself ever reported a terminal outcome for this
    /// run -- the one durable fact that tells a genuine `task report
    /// --status failed` apart from the daemon giving up on it (an ack or
    /// task timeout, a session gone, a dispatch error). `report()` writes an
    /// entry with `source: "agent"` and `kind` set to the reported status;
    /// nothing the daemon writes on its own behalf ever does.
    async fn was_reported_by_agent(&self, run_id: &str) -> bool {
        self.store
            .run_entries(run_id, 500)
            .await
            .unwrap_or_default()
            .iter()
            .any(|e| e.source == "agent" && (e.kind == "done" || e.kind == "failed"))
    }

    /// The case has no gate: `unverified`. It has one: run it in the run's
    /// own worktree and record exit code, output, and pass/fail. Runs even
    /// when the agent reported `failed` -- the gate is the judge, not the
    /// agent's own word.
    ///
    /// Takes a bare `BenchAttempt` rather than a borrow into a `BenchRun`:
    /// the gate below can run for the case's own timeout, up to ten minutes
    /// by default, and nothing here should hold, or silently go stale
    /// against, the run's own state for that long. `finish_bench_judgement`
    /// re-reads the run fresh once this returns.
    async fn judge_with_gate_or_unverified(&self, attempt: &mut BenchAttempt, case: Option<&Case>, task_run: &Run) {
        let Some(case) = case else {
            attempt.verdict = Some(Verdict::Error);
            attempt.reason = Some("the case no longer exists in this run's snapshot".into());
            return;
        };
        let Some(gate) = &case.gate else {
            attempt.verdict = Some(Verdict::Unverified);
            return;
        };
        let Some(dir) = task_run.worktree_path.as_ref() else {
            attempt.verdict = Some(Verdict::Error);
            attempt.reason = Some("no worktree recorded for this run; the gate could not be run".into());
            return;
        };
        let timeout = case.timeout_seconds.unwrap_or(DEFAULT_COMMAND_TIMEOUT_SECS);
        let (exit_code, output) = run_shell_capture(Path::new(dir), gate, timeout).await;
        attempt.gate_exit_code = exit_code;
        attempt.gate_output = Some(tail_4kib(&output));
        attempt.verdict = Some(if exit_code == Some(0) { Verdict::Pass } else { Verdict::Fail });
    }

    /// Persist the verdict, write it onto the task's own timeline, and
    /// recompute the run's status. Deliberately does not itself advance the
    /// run -- this is reachable from `advance_bench_run`'s own spawned
    /// continuations (via `record_bench_task_state`), so calling back into
    /// it here would be the same unresolvable cycle `mark_bench_attempt_uncreated`'s
    /// doc comment explains. `sync_bench_for_task` advances after calling
    /// the judging path that leads here; the scheduler's periodic sweep is
    /// the safety net for the paths that only reach `record_bench_task_state`.
    ///
    /// Re-reads the run fresh under `bench_edit` rather than trusting
    /// whatever `judge_bench_attempt` saw before the gate ran (which can
    /// take minutes): a `cancel_bench_run` landing in that window may have
    /// already moved the run to `Cancelled`, or its own settle-whatever's-
    /// left pass may have already given this very attempt a `Cancelled`
    /// verdict of its own. Either way, that decision stands -- this one
    /// checks the attempt is still unsettled in the fresh read before
    /// writing anything, and never flips a run that is not still `Running`.
    async fn finish_bench_judgement(self: &Arc<Self>, run_id: &str, task: &Task, attempt: BenchAttempt) -> Result<()> {
        let _guard = self.bench_edit.lock().await;
        let Some(mut run) = self.bench.get_run(run_id).await? else {
            return Ok(());
        };
        let Some(idx) = run.attempts.iter().position(|a| a.id == attempt.id) else {
            return Ok(());
        };
        if run.attempts[idx].verdict.is_some() {
            // Settled by somebody else -- a concurrent cancel's own pass --
            // while the gate above was running. That verdict stands; this
            // one is dropped rather than overwriting it.
            return Ok(());
        }
        run.attempts[idx] = attempt.clone();

        self.bench.put_attempt(run_id, &attempt).await?;
        self.entry(
            &task.id,
            TaskEntry::new(
                "daemon",
                "bench_verdict",
                format!("bench verdict: {}", attempt.verdict.map(Verdict::as_str).unwrap_or("?")),
            )
            .with_data(serde_json::to_value(&attempt).unwrap_or_default()),
        )
        .await;

        if run.status == BenchRunStatus::Running && run.attempts.iter().all(|a| a.verdict.is_some()) {
            run.status = BenchRunStatus::Done;
            run.ended_at = Some(Utc::now());
            self.bench.put_run(&run).await?;
        }
        self.bus.publish(Event::BenchRunUpdated { run });
        Ok(())
    }

    // -- cancel and clean --------------------------------------------------

    /// Cancel every attempt not yet settled: an in-flight one through the
    /// ordinary task-cancel path, a pending one directly, since there is no
    /// task yet to cancel.
    pub(crate) async fn cancel_bench_run(self: &Arc<Self>, run_id: &str) -> Result<BenchRun> {
        let task_ids = {
            let _guard = self.bench_edit.lock().await;
            let mut run = self.bench.get_run(run_id).await?.ok_or_else(|| missing("bench run", run_id))?;
            if run.status != BenchRunStatus::Running {
                return Ok(run);
            }
            let mut task_ids = Vec::new();
            for attempt in &mut run.attempts {
                if attempt.verdict.is_some() {
                    continue;
                }
                match attempt.task_id.clone() {
                    Some(task_id) => task_ids.push(task_id),
                    None => {
                        attempt.verdict = Some(Verdict::Cancelled);
                        attempt.ended_at = Some(Utc::now());
                    }
                }
            }
            for attempt in &run.attempts {
                self.bench.put_attempt(run_id, attempt).await?;
            }
            self.bus.publish(Event::BenchRunUpdated { run: run.clone() });
            task_ids
        };

        for task_id in &task_ids {
            if self.store.active_run(task_id).await.ok().flatten().is_some() {
                let _ = self
                    .cancel_task_run(task_id, None, factory_core::run::FailKind::CancelledWithParent)
                    .await;
            }
            // Best-effort: `judge_bench_attempt`, via the worker, may settle
            // this properly (with `reported`, and whatever the case's own
            // gate says) before the fallback pass below ever gets to it.
            // Not load-bearing for correctness either way -- that pass fills
            // in `run_id` itself now, and `finish_bench_judgement` (see its
            // own doc comment) never overwrites a verdict this fallback
            // already gave it, whichever lands first.
            self.record_bench_task_state(task_id).await;
        }

        // Settle whatever the cancellation above did not already -- a task
        // already terminal for some other reason (it happened to finish in
        // the gap between the two locked sections), or one the async judge
        // worker has not reached yet, is left with whatever verdict a
        // concurrent judgement already gave it. Anything still unsettled is
        // marked `cancelled` directly, with `run_id` and timing looked up
        // from the store when it was dispatched -- `clean_bench_run` needs
        // exactly that `run_id` to find this attempt's worktree, and
        // nothing here can assume the worker got to it first.
        let _guard = self.bench_edit.lock().await;
        let mut run = self.bench.get_run(run_id).await?.ok_or_else(|| missing("bench run", run_id))?;
        for idx in 0..run.attempts.len() {
            if run.attempts[idx].verdict.is_some() {
                continue;
            }
            let now = Utc::now();
            if let Some(task_id) = run.attempts[idx].task_id.clone() {
                if let Ok(Some(task_run)) = self.store.runs(&task_id, 1).await.map(|mut v| v.pop()) {
                    let ended_at = task_run.ended_at.unwrap_or(now);
                    run.attempts[idx].run_id = Some(task_run.id.clone());
                    run.attempts[idx].started_at = Some(task_run.started_at);
                    run.attempts[idx].ended_at = Some(ended_at);
                    run.attempts[idx].wall_clock_seconds = Some((ended_at - task_run.started_at).num_seconds());
                }
            }
            run.attempts[idx].verdict = Some(Verdict::Cancelled);
            if run.attempts[idx].ended_at.is_none() {
                run.attempts[idx].ended_at = Some(now);
            }
        }
        for attempt in &run.attempts {
            self.bench.put_attempt(run_id, attempt).await?;
        }
        run.status = BenchRunStatus::Cancelled;
        run.ended_at = Some(Utc::now());
        self.bench.put_run(&run).await?;
        self.bus.publish(Event::BenchRunUpdated { run: run.clone() });
        Ok(run)
    }

    /// Owner-only, explicit removal of exactly this finished run's worktrees
    /// and branches. Never automatic, and refused while the run is still
    /// going -- worktrees are evidence until a person asks for them back.
    pub(crate) async fn clean_bench_run(&self, run_id: &str) -> Result<BenchRun> {
        let run = self.bench.get_run(run_id).await?.ok_or_else(|| missing("bench run", run_id))?;
        if run.status == BenchRunStatus::Running {
            return Err(FactoryError::BadRequest(
                "this bench run is still going; cancel it first".into(),
            ));
        }
        let factory = self.factory_snapshot();
        for attempt in &run.attempts {
            let Some(task_run_id) = &attempt.run_id else { continue };
            let Ok(Some(task_run)) = self.store.get_run(task_run_id).await else { continue };
            let (Some(path), Some(branch)) = (&task_run.worktree_path, &task_run.worktree_branch) else {
                continue;
            };
            let Some(case) = run.cases.iter().find(|c| c.id == attempt.case_id) else { continue };
            let Ok(scope_path) = factory.scope_path(&case.scope) else { continue };
            if let Err(e) = worktree::remove(&scope_path, Path::new(path), branch).await {
                tracing::warn!(bench_run = run_id, attempt = attempt.id, "could not remove worktree: {e}");
            } else {
                let _ = crate::assignments::release(&self.workspaces, &[std::path::PathBuf::from(path)]).await;
            }
        }
        Ok(run)
    }

    /// L5 periodic progress backstop, independent of the process scheduler.
    pub(crate) async fn sweep_bench_runs(self: &Arc<Self>) {
        match self.bench.active_runs().await {
            Ok(runs) => {
                for run in runs {
                    if let Err(e) = self.advance_bench_run(&run.id).await {
                        tracing::warn!(bench_run = run.id, "could not advance bench run: {e}");
                    }
                }
            }
            Err(e) => tracing::warn!("could not list active bench runs: {e}"),
        }
    }

    // -- recovery ------------------------------------------------------

    /// Reconcile every bench run still `running` after a restart. Settled
    /// attempts with no verdict are judged now; attempts that never started
    /// are started by the same `advance_bench_run` every settle already
    /// calls. An attempt still in flight is left alone -- the ordinary run
    /// watchdog (`scheduler.rs`) notices a dead session or a timeout on its
    /// own, and that live path already calls `sync_bench_for_task` too.
    pub(crate) async fn recover_bench_runs(self: &Arc<Self>) {
        let runs = match self.bench.active_runs().await {
            Ok(runs) => runs,
            Err(e) => {
                tracing::warn!("could not load bench runs: {e}");
                return;
            }
        };
        for run in runs {
            for attempt in &run.attempts {
                if attempt.verdict.is_some() {
                    continue;
                }
                if let Some(task_id) = &attempt.task_id {
                    self.record_bench_task_state(task_id).await;
                }
            }
            if let Err(e) = self.advance_bench_run(&run.id).await {
                tracing::warn!(bench_run = run.id, "could not recover bench run: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! `judge_bench_attempt`'s two least obvious edges, pinned directly
    //! rather than only through the shell-agent e2e run: the string chain
    //! `place_run` -> `fail_run` -> `run.error` -> "was this a reset
    //! failure" runs through two independent `format!` call sites before it
    //! is ever read back here, and `was_reported_by_agent` is the one
    //! predicate that decides whether a `Failed` run's own report or its
    //! case's gate gets the last word. Either one silently regressing would
    //! misfile a verdict rather than fail loudly.

    use super::*;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::run::{NewRun, RunPatch};
    use factory_core::task::{NewTask, TaskReport};
    use factory_plugins::{Registry, SqliteStore};

    fn test_engine(scope_path: std::path::PathBuf) -> Arc<Engine> {
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            // `power_assertion` off: these tests dispatch real bench
            // attempts through the real `Engine`, and the default would
            // fork a real `caffeinate` on whatever machine runs the tests --
            // see the same note on `engine::tests::test_engine`.
            daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            scope: None,
            scopes: vec![Scope {
                id: "scope-id".into(),
                name: "demo".into(),
                path: scope_path,
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
            }],
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory {
            root: std::env::temp_dir().join(format!("factory-bench-engine-test-{}", uuid::Uuid::new_v4())),
            config,
        };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = Arc::new(Engine::new(factory, registry, store, std::path::PathBuf::from("factory"), Vec::new()));
        // Judging happens off the caller's path now (see `spawn_bench_judge`),
        // so any test that enqueues a judgement needs a worker actually
        // running to pick it up -- exactly what a real daemon does at
        // startup.
        engine.spawn_bench_judge();
        engine
    }

    #[tokio::test]
    async fn undecodable_origins_cannot_silently_skip_the_benchmark_base_or_reset() {
        let engine = test_engine(std::env::temp_dir().join("factory-bench-origin-no-dispatch"));
        let reference = factory_core::task::OriginRef::opaque("unsupported-reference");
        for error in [engine.bench_case_base(&reference).await.unwrap_err(), engine.bench_case_reset(&reference).await.unwrap_err()] {
            assert_eq!(error.code(), "bad_request");
            assert!(error.to_string().contains("benchmark origin"));
        }
        let known: factory_core::task::OriginRef = BenchOrigin {
            bench_run_id: "missing-run".into(), case_id: "case".into(), agent: "shell".into(), attempt: 1,
        }.into();
        assert!(engine.bench_case_base(&known).await.unwrap().is_none());
        assert!(engine.bench_case_reset(&known).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn l5_timer_settles_a_run_without_starting_the_process_scheduler() {
        let engine = test_engine(std::env::temp_dir().join("factory-bench-timer-no-dispatch"));
        let mut attempt = BenchAttempt::pending("attempt".into(), "case".into(), "shell".into(), 1);
        attempt.verdict = Some(Verdict::Error);
        attempt.ended_at = Some(Utc::now());
        let run = BenchRun {
            id: "timer-run".into(),
            dataset: "demo".into(),
            dataset_revision: 1,
            cases: vec![],
            case_bases: Default::default(),
            agents: vec!["shell".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Running,
            attempts: vec![],
            started_at: Utc::now(),
            ended_at: None,
        };
        engine.bench.put_run(&run).await.unwrap();
        engine.bench.put_attempt(&run.id, &attempt).await.unwrap();
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        // No scheduler::run, startup recovery, or task report can drive this.
        let timer = tokio::spawn(crate::bench::run(engine.clone(), rx));
        let settled = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let observed = engine.bench.get_run(&run.id).await.unwrap().unwrap();
                if observed.status != BenchRunStatus::Running {
                    break observed;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the independent L5 timer must advance the run");
        assert_eq!(settled.status, BenchRunStatus::Done);
        assert!(settled.ended_at.is_some());
        assert_eq!(settled.attempts[0].verdict, Some(Verdict::Error));
        shutdown.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), timer)
            .await
            .unwrap()
            .unwrap();
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-bench-engine-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A real git repository with one commit, for the tests that need
    /// `resolve_commit` to actually resolve something. Returns the full
    /// 40-hex SHA of that commit.
    async fn init_repo_with_a_commit(dir: &std::path::Path) -> String {
        async fn run(dir: &std::path::Path, args: &[&str]) {
            assert!(tokio::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .await
                .unwrap()
                .success());
        }
        run(dir, &["init", "-q"]).await;
        run(dir, &["config", "user.email", "factory@example.com"]).await;
        run(dir, &["config", "user.name", "factory"]).await;
        std::fs::write(dir.join("f.txt"), "hi").unwrap();
        run(dir, &["add", "f.txt"]).await;
        run(dir, &["commit", "-q", "-m", "initial"]).await;
        let out = tokio::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Poll `bench.get_run` until the run's first attempt has a verdict, or
    /// give up -- judging now happens on a worker off the caller's own path,
    /// so a test can no longer assume it is done the instant
    /// `record_bench_task_state`/`sync_bench_for_task` returns.
    async fn wait_for_settled(engine: &Arc<Engine>, run_id: &str) -> BenchRun {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(Some(run)) = engine.bench.get_run(run_id).await {
                if run.attempts.iter().all(|a| a.verdict.is_some()) {
                    return run;
                }
            }
            if std::time::Instant::now() >= deadline {
                panic!("bench run {run_id} did not settle within 5s");
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// One case, one pending attempt, and a `BenchRun` holding just that --
    /// enough for `record_bench_task_state`/`judge_bench_attempt` to find
    /// the attempt by identity and settle it, without a real dispatch.
    async fn seed(engine: &Arc<Engine>, run_id: &str, case: Case, agent: &str) -> (BenchOrigin, Task) {
        let origin = BenchOrigin {
            bench_run_id: run_id.to_string(),
            case_id: case.id.clone(),
            agent: agent.to_string(),
            attempt: 1,
        };
        let attempt = BenchAttempt::pending(uuid::Uuid::new_v4().to_string(), case.id.clone(), agent.to_string(), 1);
        let run = BenchRun {
            id: run_id.to_string(),
            dataset: "demo-set".into(),
            dataset_revision: 1,
            cases: vec![case],
            case_bases: Default::default(),
            agents: vec![agent.to_string()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Running,
            attempts: vec![attempt],
            started_at: Utc::now(),
            ended_at: None,
        };
        engine.bench.put_run(&run).await.unwrap();
        for attempt in &run.attempts {
            engine.bench.put_attempt(&run.id, attempt).await.unwrap();
        }

        let task = engine
            .create_bench_task(
                NewTask {
                    title: format!("bench {}", origin.case_id),
                    scope: Some("demo".into()),
                    worktree: Some(false),
                    ..Default::default()
                },
                origin.clone(),
                uuid::Uuid::new_v4().to_string(),
            )
            .await
            .unwrap();
        (origin, task)
    }

    /// Mirrors exactly what a real dispatch failure produces: `place_run`
    /// turns a failing reset into `FactoryError::BadRequest(format!("reset
    /// failed: {detail}"))`, and `start_run`'s catch arm hands that to
    /// `fail_run` as `format!("dispatch failed: {e}")` -- so the stored
    /// `run.error` carries both prefixes by the time judging ever reads it.
    /// If either `format!` changes shape, this is the test that notices.
    #[tokio::test]
    async fn a_reset_failure_is_read_back_as_skipped_never_error() {
        let scope_dir = temp_dir("reset-failure");
        let engine = test_engine(scope_dir);

        let case = Case {
            id: "case-1".into(),
            title: "case one".into(),
            scope: "demo".into(),
            instructions: "do the thing".into(),
            gate: None,
            reset: Some("false".into()),
            base: None,
            timeout_seconds: None,
            origin: None,
        };
        let (origin, task) = seed(&engine, "run-reset-failure", case, "shell").await;

        let task_run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Bench,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "local".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &task_run.id,
                &RunPatch {
                    status: Some(RunStatus::Failed),
                    error: Some("dispatch failed: reset failed: exit 1: nope".into()),
                    ended_at: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // Deliberately no agent-authored entry: nothing reported anything --
        // the daemon gave up before the agent ever ran.

        engine.record_bench_task_state(&task.id).await;

        let settled = wait_for_settled(&engine, &origin.bench_run_id).await;
        let attempt = &settled.attempts[0];
        assert_eq!(attempt.verdict, Some(Verdict::Skipped), "{attempt:?}");
        assert!(
            attempt.reason.as_deref().unwrap_or_default().contains("reset failed"),
            "{attempt:?}"
        );
    }

    /// The same shape of failure, but without the "reset failed" substring
    /// anywhere in the error -- an ordinary daemon give-up (an ack timeout,
    /// a session that vanished) must still read as `Error`, not `Skipped`.
    /// Pinning both directions is what makes the substring check above
    /// trustworthy rather than a test that would pass no matter what the
    /// code did.
    #[tokio::test]
    async fn a_daemon_give_up_with_no_reset_failure_text_is_error_not_skipped() {
        let scope_dir = temp_dir("give-up");
        let engine = test_engine(scope_dir);

        let case = Case {
            id: "case-1".into(),
            title: "case one".into(),
            scope: "demo".into(),
            instructions: "do the thing".into(),
            gate: None,
            reset: None,
            base: None,
            timeout_seconds: None,
            origin: None,
        };
        let (origin, task) = seed(&engine, "run-give-up", case, "shell").await;

        let task_run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Bench,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "local".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &task_run.id,
                &RunPatch {
                    status: Some(RunStatus::Failed),
                    error: Some("dispatch failed: the session never came up".into()),
                    ended_at: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        engine.record_bench_task_state(&task.id).await;

        let settled = wait_for_settled(&engine, &origin.bench_run_id).await;
        let attempt = &settled.attempts[0];
        assert_eq!(attempt.verdict, Some(Verdict::Error), "{attempt:?}");
    }

    /// `#122`: a dispatch refused before any run row existed leaves the task
    /// blocked on that failure, not closed -- and that is still the end of
    /// the attempt, judged `Error`, never left waiting forever.
    #[tokio::test]
    async fn a_dispatch_refused_before_any_run_is_judged_an_error_though_the_task_is_only_blocked() {
        let scope_dir = temp_dir("refused");
        let engine = test_engine(scope_dir);
        let case = Case {
            id: "case-1".into(),
            title: "case one".into(),
            scope: "demo".into(),
            instructions: "do the thing".into(),
            gate: None,
            reset: None,
            base: None,
            timeout_seconds: None,
            origin: None,
        };
        let (origin, task) = seed(&engine, "refused", case, "shell").await;
        engine
            .store
            .update(
                &task.id,
                &factory_core::task::TaskPatch {
                    status: Some(factory_core::task::TaskStatus::Blocked),
                    error: Some("dispatch failed: no such agent".into()),
                    failure: Some(factory_core::task::TaskFailure {
                        kind: Some(factory_core::run::FailKind::DispatchFailed),
                        run_id: None,
                        attempt: None,
                        at: Utc::now(),
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        engine.record_bench_task_state(&task.id).await;

        let settled = wait_for_settled(&engine, &origin.bench_run_id).await;
        let attempt = &settled.attempts[0];
        assert_eq!(attempt.verdict, Some(Verdict::Error), "{attempt:?}");
        assert!(attempt.reason.as_deref().unwrap_or_default().contains("no such agent"), "{attempt:?}");
    }

    /// The gate is the judge, never the agent's own word: a run the agent
    /// itself reported `failed` on must still be handed to the case's gate,
    /// not folded into `Error` as if the daemon had merely given up. This is
    /// the test that would catch `was_reported_by_agent` reading the wrong
    /// entry field -- if it always answered `false`, this attempt would
    /// land on `Error` instead of running the gate at all.
    #[tokio::test]
    async fn an_agent_reported_failure_is_still_judged_by_its_gate() {
        let scope_dir = temp_dir("agent-reported");
        let engine = test_engine(scope_dir.clone());

        let case = Case {
            id: "case-1".into(),
            title: "case one".into(),
            scope: "demo".into(),
            instructions: "do the thing".into(),
            gate: Some("exit 0".into()),
            reset: None,
            base: None,
            timeout_seconds: Some(5),
            origin: None,
        };
        let (origin, task) = seed(&engine, "run-agent-reported", case, "shell").await;

        let task_run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Bench,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "local".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &task_run.id,
                &RunPatch {
                    status: Some(RunStatus::Failed),
                    error: Some("agent process exited".into()),
                    ended_at: Some(Utc::now()),
                    worktree_path: Some(scope_dir.to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        engine
            .store
            .append_entry(
                &task.id,
                &TaskEntry::new("agent", "failed", "I could not finish").in_run(task_run.id.clone()),
            )
            .await
            .unwrap();

        engine.record_bench_task_state(&task.id).await;

        let settled = wait_for_settled(&engine, &origin.bench_run_id).await;
        let attempt = &settled.attempts[0];
        assert_eq!(attempt.reported.as_deref(), Some("failed"), "{attempt:?}");
        assert_eq!(
            attempt.verdict,
            Some(Verdict::Pass),
            "the gate exited 0, so the verdict must be Pass even though the agent itself \
             reported failure -- proving was_reported_by_agent took the gate branch \
             rather than reading this as a daemon give-up: {attempt:?}"
        );
    }

    /// Found by the shell-agent e2e run, not by inspection: cancelling a
    /// bench run whose attempt was genuinely in flight (dispatched, with a
    /// real `Run` row and a real worktree) left that attempt's `run_id`
    /// unset, because `cancel_task_run` -- called directly here rather than
    /// through the `TaskCancel` request path -- never itself judges the
    /// attempt behind it. `clean_bench_run` needs exactly that `run_id` to
    /// find the worktree at all, so a run cancelled this way could never
    /// have its worktrees removed by `bench clean`, forever. This is the
    /// regression test for the fix: `cancel_bench_run` now calls
    /// `record_bench_task_state` itself right after cancelling.
    #[tokio::test]
    async fn cancelling_an_in_flight_attempt_still_leaves_its_run_id_reachable() {
        let scope_dir = temp_dir("cancel-run-id");
        let engine = test_engine(scope_dir.clone());

        let case = Case {
            id: "slow-case".into(),
            title: "a case that takes a while".into(),
            scope: "demo".into(),
            instructions: "sleep 100".into(),
            gate: None,
            reset: None,
            base: None,
            timeout_seconds: None,
            origin: None,
        };
        let origin = BenchOrigin {
            bench_run_id: "run-cancel".into(),
            case_id: case.id.clone(),
            agent: "shell".into(),
            attempt: 1,
        };

        let task = engine
            .create_bench_task(
                NewTask {
                    title: "bench slow-case".into(),
                    scope: Some("demo".into()),
                    worktree: Some(false),
                    ..Default::default()
                },
                origin.clone(),
                uuid::Uuid::new_v4().to_string(),
            )
            .await
            .unwrap();

        // Already dispatched, in flight -- exactly what a running attempt
        // looks like the moment somebody asks to cancel the run.
        let mut attempt = BenchAttempt::pending(uuid::Uuid::new_v4().to_string(), case.id.clone(), "shell".into(), 1);
        attempt.task_id = Some(task.id.clone());
        attempt.started_at = Some(Utc::now());

        let run = BenchRun {
            id: origin.bench_run_id.clone(),
            dataset: "demo-set".into(),
            dataset_revision: 1,
            cases: vec![case],
            case_bases: Default::default(),
            agents: vec!["shell".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Running,
            attempts: vec![attempt],
            started_at: Utc::now(),
            ended_at: None,
        };
        engine.bench.put_run(&run).await.unwrap();
        for a in &run.attempts {
            engine.bench.put_attempt(&run.id, a).await.unwrap();
        }

        // A real, still-running `Run` row -- what `cancel_task_run` needs in
        // order to find anything to cancel, and what carries the worktree
        // this attempt would otherwise orphan.
        let task_run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Bench,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "local".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &task_run.id,
                &RunPatch {
                    status: Some(RunStatus::Running),
                    worktree_path: Some(scope_dir.to_string_lossy().to_string()),
                    worktree_branch: Some("factory/slow-case".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let settled = engine.cancel_bench_run(&origin.bench_run_id).await.unwrap();
        assert_eq!(settled.status, BenchRunStatus::Cancelled);
        let attempt = &settled.attempts[0];
        assert_eq!(attempt.verdict, Some(Verdict::Cancelled), "{attempt:?}");
        assert_eq!(
            attempt.run_id,
            Some(task_run.id.clone()),
            "a cancelled in-flight attempt must still carry the run id `clean_bench_run` \
             needs to find its worktree: {attempt:?}"
        );
    }

    /// Reported by QA, not found by inspection: `judge_bench_attempt` reads
    /// the run once, then a slow gate can run for minutes before
    /// `finish_bench_judgement` ever writes anything back. A
    /// `cancel_bench_run` landing in that window used to be clobbered --
    /// the stale snapshot's `put_run` wrote the run back to `Running`/`Done`,
    /// and the late verdict overwrote the attempt's own `Cancelled`. Fixed
    /// by having `finish_bench_judgement` re-read the run fresh, under
    /// `bench_edit`, and refuse to write over an attempt someone else
    /// already settled.
    #[tokio::test]
    async fn a_cancel_landing_during_a_slow_gate_is_never_clobbered_by_the_late_verdict() {
        let scope_dir = temp_dir("cancel-during-gate");
        let engine = test_engine(scope_dir.clone());

        let case = Case {
            id: "slow-gate-case".into(),
            title: "a case whose gate takes a moment".into(),
            scope: "demo".into(),
            instructions: "true".into(),
            gate: Some("sleep 1; true".into()),
            reset: None,
            base: None,
            timeout_seconds: Some(30),
            origin: None,
        };
        let (origin, task) = seed(&engine, "run-cancel-during-gate", case, "shell").await;

        let task_run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Bench,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "local".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &task_run.id,
                &RunPatch {
                    status: Some(RunStatus::Done),
                    worktree_path: Some(scope_dir.to_string_lossy().to_string()),
                    ended_at: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        engine
            .store
            .append_entry(
                &task.id,
                &TaskEntry::new("agent", "done", "finished").in_run(task_run.id.clone()),
            )
            .await
            .unwrap();

        // `seed` starts the attempt pending (no `task_id` yet); already
        // dispatched and in flight is what this test needs, so it is
        // patched in directly -- this is exactly the shape
        // `cancel_bench_run`'s in-flight branch expects.
        let mut run = engine.bench.get_run(&origin.bench_run_id).await.unwrap().unwrap();
        run.attempts[0].task_id = Some(task.id.clone());
        engine.bench.put_attempt(&run.id, &run.attempts[0]).await.unwrap();

        // Start judging in the background: its gate sleeps for a second,
        // simulating the window a cancel can land in.
        let judging_engine = engine.clone();
        let judging_task_id = task.id.clone();
        let handle = tokio::spawn(async move {
            judging_engine.record_bench_task_state(&judging_task_id).await;
        });

        // Give the gate time to actually be running before cancelling.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let cancelled = engine.cancel_bench_run(&origin.bench_run_id).await.unwrap();
        assert_eq!(cancelled.status, BenchRunStatus::Cancelled);

        handle.await.unwrap();

        let settled = wait_for_settled(&engine, &origin.bench_run_id).await;
        let attempt = &settled.attempts[0];
        assert_eq!(
            attempt.verdict,
            Some(Verdict::Cancelled),
            "a cancel that lands while the gate is still running must win, not the late \
             Pass the gate computes afterward: {attempt:?}"
        );
        assert_eq!(
            settled.status,
            BenchRunStatus::Cancelled,
            "the run's own status must not be flipped back to Done by the late judgement"
        );
    }

    /// Reported by QA: judging used to run inline on the reporting agent's
    /// own path, so a case with `gate: "sleep 15; true"` made `factory task
    /// report` itself hang for 15 seconds. Judging now only ever happens on
    /// `spawn_bench_judge`'s worker (see `enqueue_bench_judgement`), so
    /// `report` -- and `sync_bench_for_task` right after it, exactly what
    /// `dispatch_request`'s `TaskReport` arm calls -- must return well
    /// before the gate below (2s) does, and the verdict must still show up
    /// once the worker gets to it.
    #[tokio::test]
    async fn a_report_on_a_slow_gated_attempt_returns_promptly_and_the_verdict_appears_afterward() {
        let scope_dir = temp_dir("report-returns-promptly");
        let engine = test_engine(scope_dir.clone());

        let case = Case {
            id: "slow-gate-case".into(),
            title: "a case whose gate is slow".into(),
            scope: "demo".into(),
            instructions: "true".into(),
            gate: Some("sleep 2; true".into()),
            reset: None,
            base: None,
            timeout_seconds: Some(30),
            origin: None,
        };
        let (origin, task) = seed(&engine, "run-report-promptly", case, "shell").await;

        let task_run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Bench,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "local".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &task_run.id,
                &RunPatch {
                    worktree_path: Some(scope_dir.to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let started = std::time::Instant::now();
        engine
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("command exited 0".into()),
                    send_to: None,
                    error: None,
                    token: Some("tok".into()),
                },
            )
            .await
            .unwrap();
        engine.sync_bench_for_task(&task.id).await;
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "a report must never wait on the gate it just queued for judgement; took {elapsed:?}"
        );

        // The gate is slow, not skipped -- the verdict still shows up, once
        // the worker gets to it, and it is the gate's own answer (`Pass`),
        // not a shortcut taken because the report path did not wait for it.
        let settled = wait_for_settled(&engine, &origin.bench_run_id).await;
        let attempt = &settled.attempts[0];
        assert_eq!(attempt.verdict, Some(Verdict::Pass), "{attempt:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "sanity: the earlier measurement must not have been retroactively \
             widened by anything since"
        );
    }

    /// The same property, for the path the scheduler's own watchdog reaches:
    /// `fail_run` (an ack timeout, a task timeout, a session that vanished)
    /// calls `record_bench_task_state`, which used to judge inline too --
    /// meaning a slow gate on one bench run could stall the scheduler's
    /// entire tick (due tasks, other timeouts, standing-agent checks) behind
    /// it. `fail_run` must return promptly regardless of how slow the
    /// case's gate is.
    #[tokio::test]
    async fn fail_run_on_a_slow_gated_attempt_returns_promptly() {
        let scope_dir = temp_dir("fail-run-returns-promptly");
        let engine = test_engine(scope_dir.clone());

        let case = Case {
            id: "slow-gate-case".into(),
            title: "a case whose gate is slow".into(),
            scope: "demo".into(),
            instructions: "true".into(),
            gate: Some("sleep 2; true".into()),
            reset: None,
            base: None,
            timeout_seconds: Some(30),
            origin: None,
        };
        let (origin, task) = seed(&engine, "run-fail-run-promptly", case, "shell").await;

        let task_run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Bench,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "local".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &task_run.id,
                &RunPatch {
                    // Even a report the agent itself sent, so `fail_run`'s
                    // own judging (via the worker) takes the gate-judging
                    // branch rather than the no-report-came-back one.
                    worktree_path: Some(scope_dir.to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        engine
            .store
            .append_entry(
                &task.id,
                &TaskEntry::new("agent", "done", "finished").in_run(task_run.id.clone()),
            )
            .await
            .unwrap();

        let started = std::time::Instant::now();
        engine.fail_run(&task_run.id, factory_core::run::FailKind::SessionGone, "the agent's session is gone").await;
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "fail_run is reachable from the scheduler's own tick; it must never wait on a \
             case's gate. took {elapsed:?}"
        );

        let settled = wait_for_settled(&engine, &origin.bench_run_id).await;
        let attempt = &settled.attempts[0];
        // `fail_run` moves the task_run to `Failed`, and the agent already
        // reported a terminal outcome (a `done` entry) before the session
        // vanished -- `judge_bench_attempt`'s `RunStatus::Failed` branch
        // names that `"failed"`, matching the run's own terminal status,
        // regardless of which terminal status the agent's own entry named.
        assert_eq!(attempt.reported.as_deref(), Some("failed"), "{attempt:?}");
        assert_eq!(attempt.verdict, Some(Verdict::Pass), "{attempt:?}");
    }

    #[tokio::test]
    async fn start_bench_run_refuses_a_dataset_name_that_would_escape_the_datasets_directory() {
        let scope_dir = temp_dir("bad-dataset-name");
        let engine = test_engine(scope_dir);
        let e = engine
            .start_bench_run("../outside", vec!["shell".into()], 1, 1, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("must match"), "{e}");
    }

    /// Reported by QA, reproduced live: `"base":"deadbeef"` used to reach
    /// `git` unvalidated and only fail at dispatch, as an opaque `error` on
    /// the attempt. `start_bench_run` now resolves every case's base before
    /// ever building an attempt to dispatch; one that does not resolve is
    /// `skipped`, by name, and the agent is never even asked to run it.
    #[tokio::test]
    async fn a_case_with_an_unresolvable_base_is_skipped_before_ever_dispatching() {
        let scope_dir = temp_dir("unresolvable-base");
        init_repo_with_a_commit(&scope_dir).await;
        let engine = test_engine(scope_dir);

        engine.dataset_create("demo-set", None).await.unwrap();
        engine
            .dataset_add_cases(
                "demo-set",
                vec![Case {
                    id: "bad-base".into(),
                    title: "a case with a bad base".into(),
                    scope: "demo".into(),
                    instructions: "true".into(),
                    base: Some("deadbeef".into()),
                    reset: None,
                    gate: None,
                    timeout_seconds: None,
                    origin: None,
                }],
            )
            .await
            .unwrap();

        let run = engine
            .start_bench_run("demo-set", vec!["shell".into()], 1, 1, None)
            .await
            .unwrap();
        assert_eq!(
            run.status,
            BenchRunStatus::Done,
            "the run's only attempt pre-settles, so the run is immediately done: {run:?}"
        );
        let attempt = &run.attempts[0];
        assert_eq!(attempt.verdict, Some(Verdict::Skipped), "{attempt:?}");
        assert!(
            attempt.task_id.is_none(),
            "a case whose base never resolves must never dispatch at all: {attempt:?}"
        );
        assert!(
            attempt.reason.as_deref().unwrap_or_default().contains("deadbeef"),
            "{attempt:?}"
        );
        assert!(!run.case_bases.contains_key("bad-base"), "{run:?}");
    }

    /// The given `base` -- however it was spelled -- is what dispatch must
    /// never see again once this resolves it: only the full SHA reaches
    /// `case_bases`, and from there `worktree::create`'s own `git` argument.
    #[tokio::test]
    async fn a_case_with_a_real_base_records_its_full_commit_sha() {
        let scope_dir = temp_dir("real-base");
        let head = init_repo_with_a_commit(&scope_dir).await;
        let short = head[..7].to_string();
        let engine = test_engine(scope_dir);

        engine.dataset_create("demo-set", None).await.unwrap();
        engine
            .dataset_add_cases(
                "demo-set",
                vec![Case {
                    id: "good-base".into(),
                    title: "a case with a real base".into(),
                    scope: "demo".into(),
                    instructions: "true".into(),
                    base: Some(short),
                    reset: None,
                    gate: None,
                    timeout_seconds: None,
                    origin: None,
                }],
            )
            .await
            .unwrap();

        // An agent that resolves nowhere: the point here is `case_bases`,
        // resolved before any agent is ever looked up, not a real dispatch
        // (which would open a real session).
        let run = engine
            .start_bench_run("demo-set", vec!["nonexistent-agent".into()], 1, 1, None)
            .await
            .unwrap();
        assert_eq!(run.case_bases.get("good-base"), Some(&head), "{run:?}");
    }
}
