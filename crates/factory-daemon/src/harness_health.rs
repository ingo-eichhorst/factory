//! Running a harness's health probe (`#131`), and remembering the answer.
//!
//! An adapter declares its probe (`Agent::health_probe`); this runs it. The
//! whole of "a probe that hangs or panics must never take the daemon down" is
//! here:
//!
//! - the probe is a child process with no stdin, in a process group of its
//!   own, killed -- group and all -- the moment its timeout passes. A binary
//!   stalled in `_dyld_start` answers nothing short of `SIGKILL`, which is
//!   what it gets;
//! - it runs in a task of its own, so a panic anywhere in running it is a
//!   `JoinError` here, not a dead daemon. A probe that could not be run that
//!   way says nothing about the harness, so the dispatch goes ahead as it
//!   always did;
//! - the answer is cached per binary, both ways, and one probe runs at a
//!   time: five dispatches to one harness in the same tick probe it once,
//!   and the other four wait for that answer rather than asking again.
//!
//! One lock for every binary, not one each: probes are rare (a few minutes
//! apart per binary at most) and bounded by their timeout, and a harness that
//! is down takes its whole timeout once per `retry_seconds`, not once per
//! task. Per-binary locks would buy nothing but code.

use crate::engine::Engine;
use chrono::{DateTime, Utc};
use factory_core::adapter::Agent;
use factory_core::config::HarnessHealthConfig;
use factory_core::error::{FactoryError, Result};
use factory_core::harness::{
    blocked_reason, repair_command, HarnessRow, HarnessState, HealthProbe, HeldTask, HELD_ENTRY, RELEASED_ENTRY,
};
use factory_core::run::Trigger;
use factory_core::task::{Task, TaskEntry, TaskPatch, TaskStatus};
use std::sync::Arc;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What a dispatch is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// It answered (now or recently enough), or there is nothing to check.
    Healthy,
    /// It does not start. `problem` names the command and what went wrong.
    Unhealthy { binary: String, problem: String },
}

#[derive(Debug, Clone)]
struct Entry {
    harness: String,
    outcome: Result<String, String>,
    /// When it was probed, for the cache.
    checked: Instant,
    checked_at: DateTime<Utc>,
    unhealthy_since: Option<DateTime<Utc>>,
    /// Set when something (an automatic repair) means the next ask must
    /// probe again whatever the cache says.
    stale: bool,
    /// The automatic repair has been started in this unhealthy stretch.
    repair_started: bool,
    auto_repair: Option<String>,
}

#[derive(Default)]
pub struct HarnessHealth {
    cache: Mutex<HashMap<String, Entry>>,
    probing: tokio::sync::Mutex<()>,
    /// Tasks blocked before a run, by binary -- what the last recheck found,
    /// plus any held since.
    held: Mutex<BTreeMap<String, Vec<HeldTask>>>,
    rechecking: AtomicBool,
    last_recheck: Mutex<Option<Instant>>,
}

/// Where `program` is, the way `execvp` would find it: a name with a `/` in
/// it is a path already, anything else is looked up on the daemon's `PATH`.
/// `None` when it is on none of it.
pub fn resolve(program: &str) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// The binary a probe is about -- its cache key and the name every message
/// uses: the resolved path, or the bare program name when nothing resolved.
pub fn binary_of(probe: &HealthProbe) -> String {
    resolve(probe.program())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| probe.program().to_string())
}

impl HarnessHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `probe`'s harness starts. `trust_failure` false is a person
    /// asking by hand -- `task run` right after running the repair script --
    /// and a cached failure is not good enough for that: it probes again.
    pub async fn check(
        &self,
        harness: &str,
        probe: &HealthProbe,
        config: &HarnessHealthConfig,
        trust_failure: bool,
    ) -> Verdict {
        if !config.enabled || probe.command.is_empty() {
            return Verdict::Healthy;
        }
        let binary = binary_of(probe);
        let asked = Instant::now();
        if let Some(verdict) = self.cached(&binary, config, trust_failure, None) {
            return verdict;
        }
        let _one_at_a_time = self.probing.lock().await;
        // Somebody else may have probed it while this waited for the lock:
        // an answer that came after this asked is as fresh as one of its own.
        if let Some(verdict) = self.cached(&binary, config, trust_failure, Some(asked)) {
            return verdict;
        }

        let timeout = Duration::from_secs(config.timeout_seconds.max(1));
        let mut command = probe.command.clone();
        if let Some(resolved) = resolve(probe.program()) {
            command[0] = resolved.display().to_string();
        }
        let shown = command.join(" ");
        let outcome = match tokio::spawn(run_probe(command, timeout)).await {
            Ok(outcome) => outcome,
            Err(error) => {
                // Not the harness's fault, so nothing is cached and nothing
                // is blocked: the dispatch goes ahead as it would have
                // before there was a probe.
                tracing::warn!(harness, binary, "the harness probe itself failed: {error}");
                return Verdict::Healthy;
            }
        };
        let outcome = match outcome {
            Ok(version) => Ok(version),
            Err(ProbeError::NotFound) => Err(format!(
                "`{}` was not found on the daemon's PATH",
                probe.program()
            )),
            Err(ProbeError::Spawn(e)) => Err(format!("`{shown}` could not be started: {e}")),
            Err(ProbeError::TimedOut) => Err(format!("`{shown}` did not answer in {}s", timeout.as_secs())),
            Err(ProbeError::Exited(status, said)) => Err(match said {
                Some(said) => format!("`{shown}` exited with {status}: {said}"),
                None => format!("`{shown}` exited with {status}"),
            }),
        };
        self.record(harness, &binary, outcome)
    }

    fn cached(
        &self,
        binary: &str,
        config: &HarnessHealthConfig,
        trust_failure: bool,
        probed_after: Option<Instant>,
    ) -> Option<Verdict> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let entry = cache.get(binary)?;
        let fresh = if entry.stale {
            false
        } else if probed_after.is_some_and(|after| entry.checked >= after) {
            true
        } else {
            let age = entry.checked.elapsed();
            match entry.outcome {
                Ok(_) => age < Duration::from_secs(config.cache_seconds),
                Err(_) => trust_failure && age < Duration::from_secs(config.retry_seconds),
            }
        };
        fresh.then(|| verdict_of(binary, &entry.outcome))
    }

    fn record(&self, harness: &str, binary: &str, outcome: Result<String, String>) -> Verdict {
        let now = Utc::now();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let previous = cache.get(binary);
        let (unhealthy_since, repair_started, auto_repair) = match (&outcome, previous) {
            (Ok(_), _) => (None, false, previous.and_then(|p| p.auto_repair.clone())),
            (Err(_), Some(p)) if p.outcome.is_err() => {
                (p.unhealthy_since.or(Some(now)), p.repair_started, p.auto_repair.clone())
            }
            (Err(_), p) => (Some(now), false, p.and_then(|p| p.auto_repair.clone())),
        };
        match &outcome {
            Ok(version) => tracing::info!(harness, binary, version, "harness answered its probe"),
            Err(problem) => tracing::warn!(harness, binary, "harness does not start: {problem}"),
        }
        let verdict = verdict_of(binary, &outcome);
        cache.insert(
            binary.to_string(),
            Entry {
                harness: harness.to_string(),
                outcome,
                checked: Instant::now(),
                checked_at: now,
                unhealthy_since,
                stale: false,
                repair_started,
                auto_repair,
            },
        );
        verdict
    }

    /// Whether the opt-in automatic repair should start for `binary` now:
    /// once per unhealthy stretch, never while it is healthy.
    pub fn claim_repair(&self, binary: &str) -> bool {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.get_mut(binary) {
            Some(entry) if entry.outcome.is_err() && !entry.repair_started => {
                entry.repair_started = true;
                true
            }
            _ => false,
        }
    }

    /// What an automatic repair did. The next ask probes again.
    pub fn repaired(&self, binary: &str, outcome: String) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get_mut(binary) {
            entry.auto_repair = Some(outcome);
            entry.stale = true;
        }
    }

    /// Forget the cached answer for `binary`, so the next dispatch probes it.
    /// An acknowledgement timeout is a reason to doubt a healthy answer.
    pub fn doubt(&self, binary: &str) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get_mut(binary) {
            entry.stale = true;
        }
    }

    pub fn hold(&self, binary: &str, task: HeldTask) {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let tasks = held.entry(binary.to_string()).or_default();
        if !tasks.iter().any(|t| t.task_id == task.task_id) {
            tasks.push(task);
        }
    }

    pub fn set_held(&self, now_held: BTreeMap<String, Vec<HeldTask>>) {
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = now_held;
    }

    /// Whether a recheck is due, claiming it if so. At most one runs at a
    /// time, and one at most every `every`.
    pub fn claim_recheck(&self, every: Duration) -> bool {
        let mut last = self.last_recheck.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < every) {
            return false;
        }
        if self.rechecking.swap(true, Ordering::SeqCst) {
            return false;
        }
        *last = Some(Instant::now());
        true
    }

    pub fn recheck_done(&self) {
        self.rechecking.store(false, Ordering::SeqCst);
    }

    /// One row per harness: every `(harness, probe)` the config offers, then
    /// anything probed that the config no longer names. Read from the cache
    /// only -- a page read never probes anything.
    pub fn rows(&self, known: &[(String, HealthProbe)], repair_script: Option<&str>) -> Vec<HarnessRow> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows: Vec<HarnessRow> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut push = |harness: &str, binary: &str, entry: Option<&Entry>| {
            if !seen.insert(binary.to_string()) {
                return;
            }
            let state = match entry.map(|e| &e.outcome) {
                None => HarnessState::Unprobed,
                Some(Ok(_)) => HarnessState::Healthy,
                Some(Err(_)) => HarnessState::Unhealthy,
            };
            let unhealthy = state == HarnessState::Unhealthy;
            rows.push(HarnessRow {
                harness: harness.to_string(),
                binary: binary.to_string(),
                state,
                checked_at: entry.map(|e| e.checked_at),
                unhealthy_since: entry.and_then(|e| e.unhealthy_since),
                version: entry.and_then(|e| e.outcome.as_ref().ok().cloned()),
                reason: entry.and_then(|e| e.outcome.as_ref().err().cloned()),
                repair: unhealthy.then(|| factory_core::harness::repair_command(repair_script, harness)),
                held: held.get(binary).cloned().unwrap_or_default(),
                auto_repair: entry.and_then(|e| e.auto_repair.clone()),
            });
        };
        for (harness, probe) in known {
            let binary = binary_of(probe);
            push(harness, &binary, cache.get(&binary));
        }
        let mut rest: Vec<(&String, &Entry)> = cache.iter().collect();
        rest.sort_by(|a, b| a.0.cmp(b.0));
        for (binary, entry) in rest {
            push(&entry.harness, binary, Some(entry));
        }
        rows
    }
}

fn verdict_of(binary: &str, outcome: &Result<String, String>) -> Verdict {
    match outcome {
        Ok(_) => Verdict::Healthy,
        Err(problem) => Verdict::Unhealthy { binary: binary.to_string(), problem: problem.clone() },
    }
}

#[derive(Debug)]
enum ProbeError {
    NotFound,
    Spawn(std::io::Error),
    TimedOut,
    Exited(String, Option<String>),
}

/// Run the probe once: its first line of output when it exits zero in time.
async fn run_probe(command: Vec<String>, timeout: Duration) -> Result<String, ProbeError> {
    let program = &command[0];
    if !program.contains('/') {
        // `resolve` found nothing, or it would have been replaced by a path.
        return Err(ProbeError::NotFound);
    }
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Its own process group, so a timeout takes whatever it started with it
    // -- a wrapper script's children would otherwise hold its output open.
    #[cfg(unix)]
    cmd.process_group(0);
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ProbeError::NotFound),
        Err(e) => return Err(ProbeError::Spawn(e)),
    };
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let text = if text.trim().is_empty() { String::from_utf8_lossy(&output.stderr) } else { text };
            Ok(first_line(&text).unwrap_or_default())
        }
        Ok(Ok(output)) => {
            let status = match output.status.code() {
                Some(code) => format!("status {code}"),
                None => "a signal".to_string(),
            };
            let said = first_line(&String::from_utf8_lossy(&output.stderr))
                .or_else(|| first_line(&String::from_utf8_lossy(&output.stdout)));
            Err(ProbeError::Exited(status, said))
        }
        Ok(Err(e)) => Err(ProbeError::Spawn(e)),
        Err(_) => {
            // The child itself was dropped with the future, and
            // `kill_on_drop` has sent it SIGKILL; this is for the rest of
            // its group.
            #[cfg(unix)]
            if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
                // SAFETY: killpg with a pgid we created and a constant signal.
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            #[cfg(not(unix))]
            let _ = pid;
            Err(ProbeError::TimedOut)
        }
    }
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(200).collect())
}

// -- the daemon's side: before a dispatch, and after ---------------------------

/// How far back a hold is looked for when deciding what to release. A task
/// held longer than this stays blocked until somebody runs it by hand.
const HELD_LOOKBACK_DAYS: i64 = 7;
/// How long the opt-in automatic repair may take before it is given up on.
const REPAIR_TIMEOUT: Duration = Duration::from_secs(600);

/// The name a harness goes by in messages and in `repair-harness <name>`:
/// its program's file name.
pub fn harness_name(probe: &HealthProbe) -> String {
    Path::new(probe.program())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| probe.program().to_string())
}

impl Engine {
    /// The probe the agent a task would run on declares, if it declares one.
    pub(crate) fn probe_for(&self, scope: &str, agent: &str) -> Option<HealthProbe> {
        let (_, adapter, _) = self.resolve_agent(scope, agent).ok()?;
        self.registry.agent(&adapter).ok()?.health_probe()
    }

    /// Before a run exists (`#131`): whether `task`'s harness starts. When it
    /// does not, the task is blocked with a reason naming the binary and the
    /// repair, and this answers `HarnessUnhealthy` so `dispatch` stops before
    /// making a run row -- nothing is failed and no session is opened. A
    /// person's own `task run` probes afresh rather than trusting a cached
    /// failure: it is what they do right after running the repair.
    pub(crate) async fn harness_gate(self: &Arc<Self>, task: &Task, adapter: &dyn Agent, trigger: Trigger) -> Result<()> {
        let Some(probe) = adapter.health_probe() else {
            return Ok(());
        };
        let config = self.factory_snapshot().config.daemon.harness_health.clone();
        let harness = harness_name(&probe);
        let trust_failure = trigger != Trigger::Manual;
        let Verdict::Unhealthy { binary, problem } = self.harness.check(&harness, &probe, &config, trust_failure).await
        else {
            return Ok(());
        };
        let repair = repair_command(config.repair_script.as_deref(), &harness);
        let reason = blocked_reason(&harness, &problem, &repair);
        self.entry(
            &task.id,
            TaskEntry::new("daemon", HELD_ENTRY, reason.clone()).with_data(serde_json::json!({
                "harness": harness,
                "binary": binary,
                "repair": repair,
                "trigger": trigger,
            })),
        )
        .await;
        if let Err(e) = self
            .store
            .update(
                &task.id,
                &TaskPatch { status: Some(TaskStatus::Blocked), error: Some(reason.clone()), ..Default::default() },
            )
            .await
        {
            tracing::warn!(task = task.id, "could not block the task on its harness: {e}");
        }
        self.publish_task(&task.id).await;
        self.harness.hold(
            &binary,
            HeldTask { task_id: task.id.clone(), scope: task.scope.clone(), title: task.title.clone() },
        );
        self.maybe_auto_repair(&harness, &binary, &config);
        Err(FactoryError::HarnessUnhealthy(reason))
    }

    /// From the scheduler's tick: at most every `retry_seconds`, look again
    /// at every task held on a harness, and dispatch the ones whose harness
    /// answers now. In a task of its own -- a harness that is still down
    /// takes its whole timeout to say so, and the tick has other work.
    ///
    /// The journal is the record of a hold: a task is held while it is
    /// `blocked` with no run and its newest `harness_unhealthy` entry is
    /// recent, which is also what makes a restart lose nothing.
    pub(crate) fn recheck_harnesses(self: &Arc<Self>) {
        let config = self.factory_snapshot().config.daemon.harness_health.clone();
        if !config.enabled || !self.harness.claim_recheck(Duration::from_secs(config.retry_seconds.max(1))) {
            return;
        }
        let engine = self.clone();
        tokio::spawn(async move {
            engine.release_recovered(&config).await;
            engine.harness.recheck_done();
        });
    }

    pub(crate) async fn release_recovered(self: &Arc<Self>, config: &HarnessHealthConfig) {
        let since = Utc::now() - chrono::Duration::days(HELD_LOOKBACK_DAYS);
        let entries = match self.store.entries_of_kinds(&[HELD_ENTRY], since).await {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!("could not look for tasks held on a harness: {e}");
                return;
            }
        };
        // Oldest first, so the newest hold of each task is the one kept.
        let newest: BTreeMap<String, TaskEntry> = entries.into_iter().collect();
        let mut held: Vec<(Task, Trigger, Option<HealthProbe>)> = Vec::new();
        for (task_id, entry) in newest {
            let Ok(Some(task)) = self.store.get(&task_id).await else {
                continue;
            };
            if task.status != TaskStatus::Blocked || !matches!(self.store.active_run(&task_id).await, Ok(None)) {
                continue;
            }
            let trigger = entry
                .data
                .as_ref()
                .and_then(|d| d.get("trigger"))
                .and_then(|t| serde_json::from_value::<Trigger>(t.clone()).ok())
                .unwrap_or(Trigger::Manual);
            // The task may have been moved to an agent with nothing to
            // check since; then there is nothing to wait for.
            let probe = self.probe_for(&task.scope, &task.agent);
            held.push((task, trigger, probe));
        }
        // This is the recheck: every binary something waits on is probed
        // once now, whatever the cache says, and every task on it shares
        // that one answer.
        let binaries: std::collections::BTreeSet<String> =
            held.iter().filter_map(|(_, _, p)| p.as_ref().map(binary_of)).collect();
        for binary in &binaries {
            self.harness.doubt(binary);
        }
        let mut still_held: BTreeMap<String, Vec<HeldTask>> = BTreeMap::new();
        for (task, trigger, probe) in held {
            let verdict = match &probe {
                Some(probe) => {
                    let harness = harness_name(probe);
                    self.harness.check(&harness, probe, config, true).await
                }
                None => Verdict::Healthy,
            };
            match verdict {
                Verdict::Healthy => self.release_held(&task, trigger).await,
                Verdict::Unhealthy { binary, .. } => {
                    if let Some(probe) = &probe {
                        self.maybe_auto_repair(&harness_name(probe), &binary, config);
                    }
                    still_held.entry(binary).or_default().push(HeldTask {
                        task_id: task.id.clone(),
                        scope: task.scope.clone(),
                        title: task.title.clone(),
                    });
                }
            }
        }
        self.harness.set_held(still_held);
    }

    /// Back to `pending`, and dispatched by exactly one thing. A task whose
    /// `next_run_at` has already come -- a scheduled one held past its next
    /// slot, or a queued retry -- is the scheduler's to fire: it is `due()`
    /// the moment it is pending again, and the scheduler knows how to fire
    /// it (retry or slot) and journals the slots it passed over. Dispatching
    /// it here as well would start two runs of one task. Anything else is
    /// dispatched here, with the trigger it was held with. Under the
    /// schedule lock, so the scheduler's own look at the task cannot fall
    /// between the decision and the change.
    async fn release_held(self: &Arc<Self>, task: &Task, trigger: Trigger) {
        let scheduler_fires = {
            let _slot = self.schedule_lock.lock().await;
            let Ok(Some(current)) = self.store.get(&task.id).await else {
                return;
            };
            let scheduler_fires = current.next_run_at.is_some_and(|at| at <= Utc::now());
            let said = if scheduler_fires {
                "its harness answers again; the scheduler fires it on its next tick"
            } else {
                "its harness answers again; dispatching it"
            };
            self.entry(&task.id, TaskEntry::new("daemon", RELEASED_ENTRY, said)).await;
            if let Err(e) = self
                .store
                .update(
                    &task.id,
                    &TaskPatch { status: Some(TaskStatus::Pending), clear_error: true, ..Default::default() },
                )
                .await
            {
                tracing::warn!(task = task.id, "could not release a task held on its harness: {e}");
                return;
            }
            scheduler_fires
        };
        self.publish_task(&task.id).await;
        if scheduler_fires {
            return;
        }
        let engine = self.clone();
        let id = task.id.clone();
        tokio::spawn(async move { engine.start_run(&id, trigger).await });
    }

    /// An acknowledgement timeout on a run: a reason to doubt whatever the
    /// cache says about its harness, so the next dispatch probes it again.
    pub(crate) fn doubt_harness_of(&self, task: Option<&Task>) {
        if let Some(probe) = task.and_then(|t| self.probe_for(&t.scope, &t.agent)) {
            self.harness.doubt(&binary_of(&probe));
        }
    }

    /// The owner's opt-in (`auto_repair` with a `repair_script`): run the
    /// repair once per unhealthy stretch, in the background, bounded. The
    /// script's own signature check applies exactly as it does by hand.
    fn maybe_auto_repair(self: &Arc<Self>, harness: &str, binary: &str, config: &HarnessHealthConfig) {
        if !config.auto_repair {
            return;
        }
        let Some(script) = config.repair_script.clone() else {
            return;
        };
        if !self.harness.claim_repair(binary) {
            return;
        }
        let engine = self.clone();
        let (harness, binary) = (harness.to_string(), binary.to_string());
        tokio::spawn(async move {
            tracing::warn!(harness, binary, script, "running the automatic harness repair");
            let outcome = run_repair(&script, &harness).await;
            tracing::warn!(harness, binary, outcome, "automatic harness repair finished");
            engine.harness.repaired(&binary, outcome);
        });
    }
}

async fn run_repair(script: &str, harness: &str) -> String {
    let mut cmd = tokio::process::Command::new(script);
    cmd.arg(harness)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return format!("{script} could not be started: {e}"),
    };
    match tokio::time::timeout(REPAIR_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let text = String::from_utf8_lossy(if output.status.success() { &output.stdout } else { &output.stderr });
            let last = text.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default().to_string();
            match output.status.code() {
                Some(0) => format!("{script} {harness} succeeded: {last}"),
                Some(code) => format!("{script} {harness} exited with status {code}: {last}"),
                None => format!("{script} {harness} was killed by a signal"),
            }
        }
        Ok(Err(e)) => format!("{script} {harness} could not be waited on: {e}"),
        Err(_) => format!("{script} {harness} did not finish in {}s", REPAIR_TIMEOUT.as_secs()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fake harness: a shell script that appends a line to `count` every
    /// time it runs, then does `body`.
    ///
    /// Run once before it is handed back, outside any timeout: macOS may
    /// hold the first launch of a freshly written executable for an
    /// assessment that, with the whole suite starting processes at once,
    /// can outlast a short probe timeout -- the stall `#131` is about,
    /// in miniature. The warm-up run neither counts nor does `body`.
    pub(crate) fn fake_harness(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        let count = dir.join(format!("{name}.count"));
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\n[ -n \"$FACTORY_FAKE_WARMUP\" ] && exit 0\necho probed >> '{}'\n{body}\n",
                count.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let warm = std::process::Command::new(&path).env("FACTORY_FAKE_WARMUP", "1").status().unwrap();
        assert!(warm.success(), "the fake harness runs at all");
        path
    }

    pub(crate) fn probes(dir: &Path, name: &str) -> usize {
        std::fs::read_to_string(dir.join(format!("{name}.count")))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    /// Short enough for a test that waits out a hang, long enough that a
    /// busy machine running the whole suite still answers in it.
    pub(crate) fn fast() -> HarnessHealthConfig {
        HarnessHealthConfig { timeout_seconds: 3, ..HarnessHealthConfig::default() }
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("factory-harness-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn a_harness_that_answers_is_healthy_and_says_its_version() {
        let dir = scratch("ok");
        let bin = fake_harness(&dir, "fake", "echo 'fake 1.2.3'");
        let health = HarnessHealth::new();
        let probe = HealthProbe::version(bin.display().to_string());
        assert_eq!(health.check("fake", &probe, &fast(), true).await, Verdict::Healthy);
        let rows = health.rows(&[("fake".into(), probe)], None);
        assert_eq!(rows[0].state, HarnessState::Healthy);
        assert_eq!(rows[0].version.as_deref(), Some("fake 1.2.3"));
        assert!(rows[0].repair.is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_harness_that_hangs_is_unhealthy_within_its_timeout_and_is_killed() {
        let dir = scratch("hang");
        let bin = fake_harness(&dir, "stuck", "sleep 30");
        let health = HarnessHealth::new();
        let probe = HealthProbe::version(bin.display().to_string());
        let started = Instant::now();
        let verdict = health.check("stuck", &probe, &fast(), true).await;
        assert!(started.elapsed() < Duration::from_secs(10), "bounded by the timeout, not the sleep");
        let Verdict::Unhealthy { binary, problem } = verdict else { panic!("a hang is unhealthy") };
        assert_eq!(binary, bin.display().to_string());
        assert!(problem.contains(&format!("{} --version", bin.display())), "{problem}");
        assert!(problem.contains("did not answer in 3s"), "{problem}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_harness_that_exits_non_zero_is_unhealthy_and_says_what_it_said() {
        let dir = scratch("exit");
        let bin = fake_harness(&dir, "broken", "echo 'dyld: missing symbol' >&2; exit 3");
        let health = HarnessHealth::new();
        let probe = HealthProbe::version(bin.display().to_string());
        let Verdict::Unhealthy { problem, .. } = health.check("broken", &probe, &fast(), true).await else {
            panic!("a non-zero exit is unhealthy")
        };
        assert!(problem.contains("exited with status 3: dyld: missing symbol"), "{problem}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_program_nowhere_on_path_is_unhealthy() {
        let health = HarnessHealth::new();
        let probe = HealthProbe::version("factory-no-such-harness-anywhere");
        let Verdict::Unhealthy { binary, problem } = health.check("nope", &probe, &fast(), true).await else {
            panic!("missing is unhealthy")
        };
        assert_eq!(binary, "factory-no-such-harness-anywhere");
        assert!(problem.contains("not found on the daemon's PATH"), "{problem}");
    }

    #[tokio::test]
    async fn a_burst_of_checks_probes_once_and_a_cached_failure_is_trusted_until_asked_by_hand() {
        let dir = scratch("burst");
        let marker = dir.join("stuck");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&marker, "").unwrap();
        let bin = fake_harness(&dir, "slow", &format!("[ -e '{}' ] && sleep 30; echo ok", marker.display()));
        let health = Arc::new(HarnessHealth::new());
        let probe = HealthProbe::version(bin.display().to_string());
        let checks: Vec<_> = (0..5)
            .map(|_| {
                let (health, probe) = (health.clone(), probe.clone());
                tokio::spawn(async move { health.check("slow", &probe, &fast(), true).await })
            })
            .collect();
        for check in checks {
            assert!(matches!(check.await.unwrap(), Verdict::Unhealthy { .. }));
        }
        assert_eq!(probes(&dir, "slow"), 1, "five at once share one probe");

        // A scheduler's ask trusts the cached failure...
        assert!(matches!(health.check("slow", &probe, &fast(), true).await, Verdict::Unhealthy { .. }));
        assert_eq!(probes(&dir, "slow"), 1);
        // ...a person's `task run` does not.
        std::fs::remove_file(&marker).unwrap();
        assert_eq!(health.check("slow", &probe, &fast(), false).await, Verdict::Healthy);
        assert_eq!(probes(&dir, "slow"), 2);
        let rows = health.rows(&[], None);
        assert!(rows[0].unhealthy_since.is_none(), "healthy again ends the stretch");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn switched_off_it_probes_nothing() {
        let dir = scratch("off");
        let bin = fake_harness(&dir, "never", "exit 1");
        let health = HarnessHealth::new();
        let config = HarnessHealthConfig { enabled: false, ..fast() };
        let probe = HealthProbe::version(bin.display().to_string());
        assert_eq!(health.check("never", &probe, &config, true).await, Verdict::Healthy);
        assert_eq!(probes(&dir, "never"), 0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn an_unhealthy_row_names_the_repair_and_a_repair_is_claimed_once() {
        let dir = scratch("row");
        let bin = fake_harness(&dir, "codex", "exit 1");
        let health = HarnessHealth::new();
        let probe = HealthProbe::version(bin.display().to_string());
        health.check("codex", &probe, &fast(), true).await;
        let binary = bin.display().to_string();
        health.hold(&binary, HeldTask { task_id: "t1".into(), scope: "demo".into(), title: "x".into() });
        health.hold(&binary, HeldTask { task_id: "t1".into(), scope: "demo".into(), title: "x".into() });
        let rows = health.rows(&[("codex".into(), probe.clone())], Some("/srv/repair-harness"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, HarnessState::Unhealthy);
        assert_eq!(rows[0].repair.as_deref(), Some("/srv/repair-harness codex"));
        assert_eq!(rows[0].held.len(), 1, "one task held once");
        assert!(health.claim_repair(&binary));
        assert!(!health.claim_repair(&binary), "once per unhealthy stretch");
        health.repaired(&binary, "repaired".into());
        health.check("codex", &probe, &fast(), true).await;
        assert_eq!(probes(&dir, "codex"), 2, "a repair makes the next ask probe again");
        std::fs::remove_dir_all(dir).ok();
    }
    // -- the daemon's side: a dispatch to a harness that does not start -----

    mod dispatch {
        use super::*;
        use factory_core::adapter::{AgentRuntime, StartRequest};
        use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
        use factory_core::protocol::Payload;
        use factory_core::task::{NewTask, SessionRef};
        use factory_plugins::{HarnessAgent, Registry, SqliteStore};

        struct QuietRuntime;
        #[async_trait::async_trait]
        impl AgentRuntime for QuietRuntime {
            fn name(&self) -> &str {
                "quiet"
            }
            async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
                Ok(SessionRef { runtime: "quiet".into(), handle: req.id.clone(), meta: Default::default() })
            }
            async fn submit(&self, _: &SessionRef, _: &str) -> Result<()> {
                Ok(())
            }
            async fn status(&self, _: &SessionRef) -> Result<factory_core::adapter::RuntimeStatus> {
                Ok(factory_core::adapter::RuntimeStatus::Working)
            }
            async fn send_text(&self, _: &SessionRef, _: &str) -> Result<()> {
                Ok(())
            }
            async fn send_keys(&self, _: &SessionRef, _: &[String]) -> Result<()> {
                Ok(())
            }
            async fn read(&self, _: &SessionRef, _: u32) -> Result<String> {
                Ok(String::new())
            }
            async fn stop(&self, _: &SessionRef) -> Result<()> {
                Ok(())
            }
        }

        /// An engine whose `fake` agent is a harness at `bin` -- a path, so
        /// nothing depends on what is installed -- on a runtime that starts
        /// nothing.
        fn engine(dir: &Path, bin: &Path, health: HarnessHealthConfig) -> Arc<Engine> {
            let scope_dir = dir.join("scope");
            std::fs::create_dir_all(&scope_dir).unwrap();
            let config = Config {
                version: 1,
                instance: Instance { id: "test".into(), name: "test".into() },
                daemon: DaemonConfig {
                    power_assertion: false,
                    default_agent: "shell".into(),
                    harness_health: health,
                    ..DaemonConfig::default()
                },
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                scope: None,
                scopes: vec![Scope {
                    id: "demo-id".into(),
                    name: "demo".into(),
                    path: scope_dir,
                    agent: None,
                    agents: Vec::new(),
                    runtime: Some("quiet".into()),
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
                plugins_dir: None,
                renewals: Vec::new(),
                renewals_notify: None,
            };
            let mut registry = Registry::with_builtins();
            registry.add_runtime(Arc::new(QuietRuntime), "test");
            registry.add_agent(
                Arc::new(HarnessAgent::new("fake", bin.display().to_string(), "a fake harness")),
                "test",
            );
            Arc::new(Engine::new(
                Factory { root: dir.join("root"), config },
                registry,
                Arc::new(SqliteStore::in_memory().unwrap()),
                PathBuf::from("/bin/factory"),
                Vec::new(),
            ))
        }

        async fn task(engine: &Arc<Engine>, agent: &str) -> Task {
            engine
                .create(NewTask {
                    title: format!("work for {agent}"),
                    instructions: "true".into(),
                    scope: Some("demo".into()),
                    agent: Some(agent.into()),
                    // The scope is no git repository; the harness is the
                    // point here, not where the run works.
                    worktree: Some(false),
                    ..Default::default()
                })
                .await
                .unwrap()
        }

        async fn kinds(engine: &Arc<Engine>, id: &str) -> Vec<String> {
            engine.store.entries(id, 100).await.unwrap().into_iter().map(|e| e.kind).collect()
        }

        /// Until `check` holds, or a few seconds pass.
        async fn eventually<F, Fut>(mut check: F)
        where
            F: FnMut() -> Fut,
            Fut: std::future::Future<Output = bool>,
        {
            for _ in 0..100 {
                if check().await {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("did not happen in time");
        }

        #[tokio::test]
        async fn a_harness_that_hangs_blocks_the_task_within_its_timeout_with_no_run() {
            let dir = scratch("gate-hang");
            let bin = fake_harness(&dir, "codex", "sleep 30");
            let engine = engine(&dir, &bin, fast());
            let t = task(&engine, "fake").await;

            let started = Instant::now();
            engine.start_run(&t.id, Trigger::Schedule).await;
            assert!(started.elapsed() < Duration::from_secs(10), "within the probe's timeout");

            let now = engine.store.get(&t.id).await.unwrap().unwrap();
            assert_eq!(now.status, TaskStatus::Blocked, "blocked, never failed");
            let why = now.error.unwrap_or_default();
            assert!(why.contains(&format!("{} --version", bin.display())), "names the binary: {why}");
            assert!(why.contains("did not answer in 3s"), "{why}");
            assert!(why.contains("scripts/repair-harness codex"), "names the repair: {why}");
            assert!(engine.store.runs(&t.id, 10).await.unwrap().is_empty(), "no run was started");
            assert!(kinds(&engine, &t.id).await.contains(&HELD_ENTRY.to_string()));

            let rows = engine.harness.rows(&[], None);
            assert_eq!(rows[0].state, HarnessState::Unhealthy);
            assert_eq!(rows[0].held.len(), 1);
            std::fs::remove_dir_all(dir).ok();
        }

        #[tokio::test]
        async fn a_harness_that_exits_non_zero_blocks_the_task_too() {
            let dir = scratch("gate-exit");
            let bin = fake_harness(&dir, "codex", "exit 7");
            let engine = engine(&dir, &bin, fast());
            let t = task(&engine, "fake").await;
            engine.start_run(&t.id, Trigger::Manual).await;
            let now = engine.store.get(&t.id).await.unwrap().unwrap();
            assert_eq!(now.status, TaskStatus::Blocked);
            let why = now.error.unwrap_or_default();
            assert!(why.contains("exited with status 7"), "{why}");
            assert!(engine.store.runs(&t.id, 10).await.unwrap().is_empty());
            std::fs::remove_dir_all(dir).ok();
        }

        #[tokio::test]
        async fn five_dispatches_at_once_probe_once_and_are_all_held() {
            let dir = scratch("gate-burst");
            let bin = fake_harness(&dir, "codex", "sleep 30");
            let engine = engine(&dir, &bin, fast());
            let mut ids = Vec::new();
            for _ in 0..5 {
                ids.push(task(&engine, "fake").await.id);
            }
            let starts: Vec<_> = ids
                .iter()
                .map(|id| {
                    let (engine, id) = (engine.clone(), id.clone());
                    tokio::spawn(async move { engine.start_run(&id, Trigger::Schedule).await })
                })
                .collect();
            for s in starts {
                s.await.unwrap();
            }
            assert_eq!(probes(&dir, "codex"), 1, "one probe for the burst");
            for id in &ids {
                assert_eq!(engine.store.get(id).await.unwrap().unwrap().status, TaskStatus::Blocked);
            }
            let rows = engine.harness.rows(&[], None);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].held.len(), 5);
            std::fs::remove_dir_all(dir).ok();
        }

        #[tokio::test]
        async fn a_healthy_harness_dispatches_and_the_shell_agent_is_never_probed() {
            let dir = scratch("gate-ok");
            let bin = fake_harness(&dir, "codex", "echo 'codex-cli 0.157.0'");
            let engine = engine(&dir, &bin, fast());
            let t = task(&engine, "fake").await;
            engine.start_run(&t.id, Trigger::Manual).await;
            let runs = engine.store.runs(&t.id, 10).await.unwrap();
            assert_eq!(runs.len(), 1, "dispatched");
            assert_eq!(probes(&dir, "codex"), 1);

            let s = task(&engine, "shell").await;
            engine.start_run(&s.id, Trigger::Manual).await;
            assert_eq!(engine.store.runs(&s.id, 10).await.unwrap().len(), 1);
            assert_eq!(probes(&dir, "codex"), 1, "the shell agent declares no probe");

            let Payload::Infrastructure { harnesses, .. } = engine.infrastructure().await else {
                panic!("infrastructure")
            };
            assert_eq!(harnesses.len(), 1, "one harness known, the shell agent is none");
            assert_eq!(harnesses[0].state, HarnessState::Healthy);
            assert_eq!(harnesses[0].version.as_deref(), Some("codex-cli 0.157.0"));
            std::fs::remove_dir_all(dir).ok();
        }

        #[tokio::test]
        async fn a_held_task_is_released_once_its_harness_answers_again() {
            let dir = scratch("gate-release");
            let marker = dir.join("stuck");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&marker, "").unwrap();
            let bin = fake_harness(&dir, "codex", &format!("[ -e '{}' ] && sleep 30; echo ok", marker.display()));
            let engine = engine(&dir, &bin, HarnessHealthConfig { retry_seconds: 0, ..fast() });
            let t = task(&engine, "fake").await;
            engine.start_run(&t.id, Trigger::Schedule).await;
            assert_eq!(engine.store.get(&t.id).await.unwrap().unwrap().status, TaskStatus::Blocked);

            // Still down: stays held.
            let config = engine.factory_snapshot().config.daemon.harness_health.clone();
            engine.release_recovered(&config).await;
            assert_eq!(engine.store.get(&t.id).await.unwrap().unwrap().status, TaskStatus::Blocked);
            assert_eq!(engine.harness.rows(&[], None)[0].held.len(), 1);

            // Repaired: released, and dispatched with its own trigger.
            std::fs::remove_file(&marker).unwrap();
            engine.release_recovered(&config).await;
            let e = engine.clone();
            let id = t.id.clone();
            eventually(|| {
                let (e, id) = (e.clone(), id.clone());
                async move { e.store.runs(&id, 10).await.unwrap().len() == 1 }
            })
            .await;
            let run = &engine.store.runs(&t.id, 10).await.unwrap()[0];
            assert_eq!(run.trigger, Trigger::Schedule);
            assert!(run.error.is_none(), "dispatched cleanly: {:?}", run.error);
            let task = engine.store.get(&t.id).await.unwrap().unwrap();
            assert!(task.error.is_none(), "the reason does not outlive the hold: {:?}", task.error);
            assert!(kinds(&engine, &t.id).await.contains(&RELEASED_ENTRY.to_string()));
            assert!(engine.harness.rows(&[], None)[0].held.is_empty());
            std::fs::remove_dir_all(dir).ok();
        }

        #[tokio::test]
        async fn a_scheduled_task_held_past_its_slot_is_left_to_the_scheduler_not_dispatched_twice() {
            let dir = scratch("gate-slot");
            let marker = dir.join("stuck");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&marker, "").unwrap();
            let bin = fake_harness(&dir, "codex", &format!("[ -e '{}' ] && exit 1; echo ok", marker.display()));
            let engine = engine(&dir, &bin, HarnessHealthConfig { retry_seconds: 0, ..fast() });
            let t = engine
                .create(NewTask {
                    title: "every second".into(),
                    instructions: "true".into(),
                    scope: Some("demo".into()),
                    agent: Some("fake".into()),
                    worktree: Some(false),
                    schedule: Some(factory_core::task::Schedule::Every { seconds: 1 }),
                    ..Default::default()
                })
                .await
                .unwrap();
            engine.start_run(&t.id, Trigger::Schedule).await;
            assert_eq!(engine.store.get(&t.id).await.unwrap().unwrap().status, TaskStatus::Blocked);
            // Held past its next slot.
            tokio::time::sleep(Duration::from_millis(1200)).await;
            std::fs::remove_file(&marker).unwrap();

            let config = engine.factory_snapshot().config.daemon.harness_health.clone();
            engine.release_recovered(&config).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let task = engine.store.get(&t.id).await.unwrap().unwrap();
            assert_eq!(task.status, TaskStatus::Pending);
            assert!(engine.store.runs(&t.id, 10).await.unwrap().is_empty(), "the release did not dispatch it");
            let due = engine.due_now().await.unwrap();
            assert!(due.iter().any(|d| d.id == t.id), "the scheduler fires it, once, on its next tick");
            std::fs::remove_dir_all(dir).ok();
        }

        #[tokio::test]
        async fn a_manual_run_probes_again_rather_than_trusting_a_cached_failure() {
            let dir = scratch("gate-manual");
            let marker = dir.join("stuck");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&marker, "").unwrap();
            let bin = fake_harness(&dir, "codex", &format!("[ -e '{}' ] && exit 1; echo ok", marker.display()));
            let engine = engine(&dir, &bin, fast());
            let t = task(&engine, "fake").await;
            engine.start_run(&t.id, Trigger::Schedule).await;
            std::fs::remove_file(&marker).unwrap();
            engine.start_run(&t.id, Trigger::Manual).await;
            assert_eq!(engine.store.runs(&t.id, 10).await.unwrap().len(), 1, "the person's run went ahead");
            std::fs::remove_dir_all(dir).ok();
        }

        #[tokio::test]
        async fn the_opt_in_automatic_repair_runs_once_and_the_task_is_then_released() {
            let dir = scratch("gate-auto");
            let marker = dir.join("stuck");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&marker, "").unwrap();
            let bin = fake_harness(&dir, "codex", &format!("[ -e '{}' ] && exit 1; echo ok", marker.display()));
            // The repair: say which harness, un-stick it.
            let repair = fake_harness(
                &dir,
                "repair-harness",
                &format!("echo \"$1\" >> '{}'; rm -f '{}'; echo swapped", dir.join("repaired").display(), marker.display()),
            );
            let config = HarnessHealthConfig {
                retry_seconds: 0,
                auto_repair: true,
                repair_script: Some(repair.display().to_string()),
                ..fast()
            };
            let engine = engine(&dir, &bin, config.clone());
            let t = task(&engine, "fake").await;
            engine.start_run(&t.id, Trigger::Schedule).await;
            let why = engine.store.get(&t.id).await.unwrap().unwrap().error.unwrap_or_default();
            assert!(why.contains(&format!("{} codex", repair.display())), "names the configured script: {why}");

            let d = dir.clone();
            eventually(|| {
                let d = d.clone();
                async move { d.join("repaired").exists() && !d.join("stuck").exists() }
            })
            .await;
            let e = engine.clone();
            eventually(|| {
                let e = e.clone();
                async move { e.harness.rows(&[], None)[0].auto_repair.is_some() }
            })
            .await;
            assert_eq!(std::fs::read_to_string(dir.join("repaired")).unwrap().trim(), "codex");
            engine.release_recovered(&config).await;
            let (e, id) = (engine.clone(), t.id.clone());
            eventually(|| {
                let (e, id) = (e.clone(), id.clone());
                async move { e.store.runs(&id, 10).await.unwrap().len() == 1 }
            })
            .await;
            assert_eq!(probes(&dir, "repair-harness"), 1, "run once");
            std::fs::remove_dir_all(dir).ok();
        }

        #[tokio::test]
        async fn without_the_switch_nothing_repairs_on_its_own() {
            let dir = scratch("gate-noauto");
            let bin = fake_harness(&dir, "codex", "exit 1");
            let repair = fake_harness(&dir, "repair-harness", "echo nope");
            let config = HarnessHealthConfig { repair_script: Some(repair.display().to_string()), ..fast() };
            let engine = engine(&dir, &bin, config);
            let t = task(&engine, "fake").await;
            engine.start_run(&t.id, Trigger::Schedule).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(probes(&dir, "repair-harness"), 0, "auto_repair is off by default");
            std::fs::remove_dir_all(dir).ok();
        }
    }
}
