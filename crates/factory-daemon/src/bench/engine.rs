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
/// fallback when a case sets no `timeout_seconds`.
const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 600;

/// Run `sh -c command` in `dir`, combined stdout+stderr, bounded by
/// `timeout_secs`. `(None, ...)` on a timeout or a failure to even start the
/// process -- both read as "could not confirm this passed", never as `Pass`.
async fn run_shell_capture(dir: &Path, command: &str, timeout_secs: u64) -> (Option<i32>, String) {
    let attempt = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output();
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), attempt).await {
        Ok(Ok(output)) => {
            let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
            combined.push_str(&String::from_utf8_lossy(&output.stderr));
            (output.status.code(), combined)
        }
        Ok(Err(e)) => (None, format!("could not run the command: {e}")),
        Err(_) => (None, format!("did not finish within {timeout_secs}s")),
    }
}

async fn scope_head(scope_path: &Path) -> std::result::Result<String, String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(scope_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

        // Resolved once, now: every agent's attempt at a case must start from
        // the same commit, so a case without its own `base` gets the scope's
        // HEAD pinned here rather than re-read at each attempt's own dispatch.
        // Best-effort -- a scope that cannot be reached yet is not a reason to
        // refuse the whole run; `resolve_agent` refuses its own cases later,
        // by name, once dispatch actually gets there.
        let mut case_bases = std::collections::BTreeMap::new();
        for case in &cases {
            if let Some(base) = &case.base {
                case_bases.insert(case.id.clone(), base.clone());
                continue;
            }
            if let Ok(scope_path) = factory.scope_path(&case.scope) {
                if let Ok(head) = scope_head(&scope_path).await {
                    case_bases.insert(case.id.clone(), head);
                }
            }
        }

        let mut attempts = Vec::new();
        for case in &cases {
            for agent in &agents {
                for n in 1..=attempts_per_case {
                    attempts.push(BenchAttempt::pending(
                        uuid::Uuid::new_v4().to_string(),
                        case.id.clone(),
                        agent.clone(),
                        n,
                    ));
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
    pub(crate) async fn bench_case_base(&self, origin: &BenchOrigin) -> Option<String> {
        let run = self.bench.get_run(&origin.bench_run_id).await.ok().flatten()?;
        run.case_bases.get(&origin.case_id).cloned()
    }

    /// The case's reset command, for `place_run` to run before the agent
    /// starts.
    pub(crate) async fn bench_case_reset(&self, origin: &BenchOrigin) -> Option<String> {
        let run = self.bench.get_run(&origin.bench_run_id).await.ok().flatten()?;
        run.cases.iter().find(|c| c.id == origin.case_id).and_then(|c| c.reset.clone())
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

    /// Judge a bench attempt once its task's run has settled, and nothing
    /// more -- called at every site `record_workflow_task_state` is (a
    /// dispatch that ends before any report at all: `start_run`'s two
    /// branches, and `fail_run`), all of which `advance_bench_run`'s own
    /// spawned continuations can reach. Deliberately does not advance the
    /// run afterward: see `sync_bench_for_task`, which does, and why that
    /// has to live in a function these dispatch-path sites never call.
    ///
    /// A no-op when the task carries no `bench_origin`, when its run is not
    /// terminal yet, or when the attempt already has a verdict -- which is
    /// what makes calling this (or `sync_bench_for_task`) more than once for
    /// the same settle safe.
    pub(crate) async fn record_bench_task_state(self: &Arc<Self>, task_id: &str) {
        let Ok(Some(task)) = self.store.get(task_id).await else { return };
        let Some(origin) = task.bench_origin.clone() else { return };

        // One judgement per attempt, ever. A report and a cancel racing each
        // other, or a live call racing recovery's own sweep, must never run
        // the same case's gate command twice.
        {
            let mut judging = self.bench_judging.lock().unwrap();
            if !judging.insert(task_id.to_string()) {
                return;
            }
        }
        if let Err(e) = self.judge_bench_attempt(&origin, &task).await {
            tracing::warn!(task = task_id, "could not judge bench attempt: {e}");
        }
        self.bench_judging.lock().unwrap().remove(task_id);
    }

    /// The same, and then try to advance the run -- judging just settled an
    /// attempt, which may have freed a concurrency slot. Called only from
    /// request-level entry points (`TaskReport`, `TaskCancel`) and restart
    /// recovery: none of those are ever reachable from inside
    /// `advance_bench_run`'s own call tree, which is what makes calling
    /// `advance_bench_run` from here safe rather than a cycle.
    pub(crate) async fn sync_bench_for_task(self: &Arc<Self>, task_id: &str) {
        let Ok(Some(task)) = self.store.get(task_id).await else { return };
        let Some(origin) = task.bench_origin.clone() else { return };
        self.record_bench_task_state(task_id).await;
        let _ = self.advance_bench_run(&origin.bench_run_id).await;
    }

    async fn judge_bench_attempt(self: &Arc<Self>, origin: &BenchOrigin, task: &Task) -> Result<()> {
        let Some(mut run) = self.bench.get_run(&origin.bench_run_id).await? else {
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

        let now = Utc::now();
        let task_run = self.store.runs(&task.id, 1).await?.into_iter().next();

        let Some(task_run) = task_run else {
            // Dispatch never even created a run row: `resolve_agent`, the
            // registry, or the scope-path check refused before
            // `store.create_run` was ever called.
            if !task.status.is_terminal() {
                return Ok(());
            }
            let attempt = &mut run.attempts[idx];
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
            return self.finish_bench_judgement(run, idx, task).await;
        };

        if !task_run.status.is_terminal() {
            return Ok(());
        }

        let ended_at = task_run.ended_at.unwrap_or(now);
        {
            let attempt = &mut run.attempts[idx];
            attempt.run_id = Some(task_run.id.clone());
            attempt.started_at = Some(task_run.started_at);
            attempt.ended_at = Some(ended_at);
            attempt.wall_clock_seconds = Some((ended_at - task_run.started_at).num_seconds());
        }

        let case = run.cases.iter().find(|c| c.id == origin.case_id).cloned();

        match task_run.status {
            RunStatus::Cancelled => {
                run.attempts[idx].verdict = Some(Verdict::Cancelled);
            }
            RunStatus::Done => {
                run.attempts[idx].reported = Some("done".to_string());
                self.judge_with_gate_or_unverified(&mut run, idx, case.as_ref(), &task_run).await;
            }
            RunStatus::Failed => {
                if self.was_reported_by_agent(&task_run.id).await {
                    run.attempts[idx].reported = Some("failed".to_string());
                    self.judge_with_gate_or_unverified(&mut run, idx, case.as_ref(), &task_run).await;
                } else {
                    let error = task_run.error.clone().unwrap_or_default();
                    if error.contains("reset failed") {
                        run.attempts[idx].verdict = Some(Verdict::Skipped);
                        run.attempts[idx].reason = Some(error);
                    } else {
                        run.attempts[idx].verdict = Some(Verdict::Error);
                        run.attempts[idx].reason = Some(if error.is_empty() {
                            "the run ended before any report came back".to_string()
                        } else {
                            error
                        });
                    }
                }
            }
            RunStatus::Dispatching | RunStatus::Running | RunStatus::Blocked => {
                unreachable!("checked terminal above")
            }
        }

        self.finish_bench_judgement(run, idx, task).await
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
    async fn judge_with_gate_or_unverified(
        &self,
        run: &mut BenchRun,
        idx: usize,
        case: Option<&Case>,
        task_run: &Run,
    ) {
        let Some(case) = case else {
            run.attempts[idx].verdict = Some(Verdict::Error);
            run.attempts[idx].reason = Some("the case no longer exists in this run's snapshot".into());
            return;
        };
        let Some(gate) = &case.gate else {
            run.attempts[idx].verdict = Some(Verdict::Unverified);
            return;
        };
        let Some(dir) = task_run.worktree_path.as_ref() else {
            run.attempts[idx].verdict = Some(Verdict::Error);
            run.attempts[idx].reason = Some("no worktree recorded for this run; the gate could not be run".into());
            return;
        };
        let timeout = case.timeout_seconds.unwrap_or(DEFAULT_COMMAND_TIMEOUT_SECS);
        let (exit_code, output) = run_shell_capture(Path::new(dir), gate, timeout).await;
        run.attempts[idx].gate_exit_code = exit_code;
        run.attempts[idx].gate_output = Some(tail_4kib(&output));
        run.attempts[idx].verdict = Some(if exit_code == Some(0) { Verdict::Pass } else { Verdict::Fail });
    }

    /// Persist the verdict, write it onto the task's own timeline, and
    /// recompute the run's status. Deliberately does not itself advance the
    /// run -- this is reachable from `advance_bench_run`'s own spawned
    /// continuations (via `record_bench_task_state`), so calling back into
    /// it here would be the same unresolvable cycle `mark_bench_attempt_uncreated`'s
    /// doc comment explains. `sync_bench_for_task` advances after calling
    /// the judging path that leads here; the scheduler's periodic sweep is
    /// the safety net for the paths that only reach `record_bench_task_state`.
    async fn finish_bench_judgement(self: &Arc<Self>, mut run: BenchRun, idx: usize, task: &Task) -> Result<()> {
        let attempt = run.attempts[idx].clone();
        self.bench.put_attempt(&run.id, &attempt).await?;
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

        if run.attempts.iter().all(|a| a.verdict.is_some()) && run.status == BenchRunStatus::Running {
            run.status = BenchRunStatus::Done;
            run.ended_at = Some(Utc::now());
        }
        self.bench.put_run(&run).await?;
        self.bus.publish(Event::BenchRunUpdated { run: run.clone() });
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
                let _ = self.cancel_task_run(task_id).await;
            }
            // `cancel_task_run` closes the run but, unlike the request-level
            // `TaskCancel` path, never itself judges the attempt behind it --
            // so without this, the attempt would reach the settle-whatever's-
            // left pass below with no `run_id`, no `started_at`, and no
            // `wall_clock_seconds`, and its worktree would never be
            // reachable by `clean_bench_run` again. Safe to call
            // unconditionally: a no-op cycle (E0391) is not a risk here --
            // `cancel_bench_run` is never on `advance_bench_run`'s own spawn
            // tree -- and a no-op read of what already happened is fine when
            // there was nothing to cancel.
            self.record_bench_task_state(task_id).await;
        }

        // Settle whatever the cancellation above did not already -- a task
        // already terminal for some other reason (it happened to finish in
        // the gap between the two locked sections) is left with whatever
        // verdict a concurrent judgement already gave it.
        let _guard = self.bench_edit.lock().await;
        let mut run = self.bench.get_run(run_id).await?.ok_or_else(|| missing("bench run", run_id))?;
        for attempt in &mut run.attempts {
            if attempt.verdict.is_some() {
                continue;
            }
            attempt.verdict = Some(Verdict::Cancelled);
            attempt.ended_at = Some(Utc::now());
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
            }
        }
        Ok(run)
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
    use factory_core::task::NewTask;
    use factory_plugins::{Registry, SqliteStore};

    fn test_engine(scope_path: std::path::PathBuf) -> Arc<Engine> {
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            roles: Default::default(),
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
                roles: Default::default(),
            }],
            plugins_dir: None,
        };
        let factory = Factory {
            root: std::env::temp_dir().join(format!("factory-bench-engine-test-{}", uuid::Uuid::new_v4())),
            config,
        };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, std::path::PathBuf::from("factory"), Vec::new()))
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-bench-engine-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
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

        let settled = engine.bench.get_run(&origin.bench_run_id).await.unwrap().unwrap();
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

        let settled = engine.bench.get_run(&origin.bench_run_id).await.unwrap().unwrap();
        let attempt = &settled.attempts[0];
        assert_eq!(attempt.verdict, Some(Verdict::Error), "{attempt:?}");
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

        let settled = engine.bench.get_run(&origin.bench_run_id).await.unwrap().unwrap();
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
}
