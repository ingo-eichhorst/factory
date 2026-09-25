//! How the line is running: the read projection behind the L4 Operations
//! tab, `factory stats` and the Inbox's attention list (issue `#106`). Pure,
//! like `goals::evaluate` and `scenario::evaluate_signposts`: it is handed
//! tasks, runs, standing agents and a few journal-derived facts, with `now`
//! passed in rather than read off the clock, and it turns them into one
//! [`OperationsReport`]. Gathering those inputs from the store is
//! `factory-daemon`'s job; nothing here does I/O, and nothing here starts,
//! stops or changes anything -- a picture, not a controller (design §8).
//!
//! ## Words, fixed here
//!
//! *finished*, *scrapped*, *reworked* and *first pass* are
//! `factory-daemon/src/production.rs`'s definitions, which that module
//! calls fixed there and nowhere else. They are restated below as
//! predicates ([`is_finished`] and its neighbours) only because this crate
//! cannot reach the daemon's; production.rs stays the source of truth, and
//! a test here pins the same edge cases its own tests do. Rework in
//! particular is not `attempt > 1` -- a scheduled task's every firing bumps
//! `attempt` -- but a `retry`, or a `manual`/`workflow` run whose task's
//! previous attempt failed or was cancelled ([`is_rework`]).
//!
//! * **cycle time** -- `started_at` to `ended_at` of a run that ended
//!   `done`. Done only: a failed or cancelled run did not complete the
//!   work, so its length says nothing about how long the work takes.
//! * **queue wait** -- `queued_at` to `started_at`: how long a run waited
//!   once it could have been dispatched. For a scheduled firing `queued_at`
//!   is already past any downtime and past the previous run's end (see
//!   `Run::queued_at`), so a daemon that was down or a task still running
//!   at its slot does not show up here. Only for a run that records
//!   `queued_at`; see "No data is not zero" below.
//! * **schedule lateness** -- `scheduled_for` to `started_at`: every second
//!   between a slot and its dispatch, whatever the cause. Reported apart
//!   from queue wait ([`ScheduleRow::last_late_s`]), never folded into it.
//! * **age** -- how long a run in progress has been going: `now -
//!   started_at`, the same clock cycle time uses, so an age and a
//!   percentile of cycle times can be compared honestly. A task that is
//!   due but not dispatched yet ages from its slot (`next_run_at`).
//! * **recovery** -- from the end of the first failed run of a failure
//!   streak to the end of the same task's next run that ended `done`.
//!   Cancelled runs neither start nor end a streak.
//! * **intervention** -- what the record shows the owner doing to keep the
//!   line moving: a `manual` run of a task whose previous attempt ended
//!   failed or cancelled (run again), a run cancelled by the owner
//!   ([`FailKind::CancelledByPerson`]), and an answer the owner typed into
//!   a blocked run through `run.answer` ([`OperationsInput::answers`]). A
//!   `task.run` journals who asked, so a run an agent asked for is left
//!   out ([`OperationsInput::agent_runs`]); so is a cancel an agent asked
//!   for, which is `CancelledByAgent`. Two limits, pulling opposite ways:
//!   an agent that leaves its token out *is* the owner to Factory and is
//!   counted as one, and text typed straight into a run's terminal (not
//!   through `run.answer`) leaves no record and is not counted. Runs from
//!   before `task.run` journaled who asked count as the owner's.
//!
//! ## Percentiles and pace
//!
//! Every percentile here is nearest-rank over the sorted samples --
//! `index = round(p * (n - 1))`, the rule `scenario.rs` already uses for its
//! forecasts (one method, one place: [`crate::scenario::nearest_rank`]).
//! A run's pace is judged against the cycle times of its own task's
//! finished runs when there are at least [`MIN_HISTORY`] of them, else its
//! scope's, else not at all: fewer than [`MIN_HISTORY`] finished runs is
//! "not enough history", which means no pace colour and never an `aging`
//! exception -- a percentile of three runs is a guess wearing a number.
//!
//! ## No data is not zero
//!
//! `queued_at`, `scheduled_for` and `fail_kind` did not exist before this
//! module did. A run made earlier has none of them, and a figure read off
//! those fields over a window that reaches back before them says so --
//! [`Figure::reason`] names [`OperationsReport::recorded_since`] -- instead
//! of reading the missing values as zeros. An old failed run with no
//! `fail_kind` is counted as `unclassified`, never as any one kind.
//!
//! ## The attention queue
//!
//! Only what a human can act on goes in it ([`ExceptionKind`]): an alert is
//! something a person must do something about; everything else is a
//! number in Flow or Health. A run that failed and will be retried is not
//! in it -- it is [`Flow::retrying`], a count -- and only one whose retries
//! are spent is (Dagster's *degraded* against *warning*). A screen-read
//! suspicion is shown as a suspicion ([`Exception::suspicion`]) and never
//! as a status (`AGENTS.md`). A permanent agent is only ever judged on
//! whether its session is there, never on being quiet, and is never
//! `aging`. Each run appears at most once, as its most severe exception,
//! with any other kind it also matched listed in [`Exception::also`].

use crate::agent::{AgentSession, AgentState, Lifetime};
use crate::metrics::MetricId;
use crate::run::{FailKind, Run, RunStatus, Trigger};
use crate::task::{Schedule, Task, TaskStatus};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Fewer finished runs than this and there is no pace to judge against.
pub const MIN_HISTORY: usize = 5;

/// How long past its slot a due task may sit undispatched before it counts
/// as late, when the caller names nothing -- two scheduler ticks at the
/// default tick, so a slot the next tick is about to fire is not an alarm.
pub const DEFAULT_LATE_AFTER_SECONDS: i64 = 120;

// ============================================================= run facts

/// `ended_at` is set. production.rs's *finished*.
pub fn is_finished(run: &Run) -> bool {
    run.ended_at.is_some()
}

/// Finished `failed` or `cancelled`. production.rs's *scrapped*.
pub fn is_scrapped(run: &Run) -> bool {
    is_finished(run) && matches!(run.status, RunStatus::Failed | RunStatus::Cancelled)
}

/// `(task_id, attempt) -> status` over `runs` -- where [`is_rework`] looks
/// a run's own predecessor up, built once rather than per run.
pub type Predecessors<'a> = BTreeMap<(&'a str, u32), RunStatus>;

pub fn predecessors(runs: &[Run]) -> Predecessors<'_> {
    runs.iter().map(|r| ((r.task_id.as_str(), r.attempt), r.status)).collect()
}

/// production.rs's `is_rework`: a re-attempt of work that did not succeed.
/// A `retry` always is; a `manual` or `workflow` run is when the same
/// task's previous attempt ended failed or cancelled; a `schedule`, `bench`
/// or `agent` run never is -- each is the next occurrence of standing work.
/// A predecessor older than the runs handed over reads as "did not fail".
pub fn is_rework(run: &Run, prev: &Predecessors<'_>) -> bool {
    match run.trigger {
        Trigger::Retry => true,
        Trigger::Manual | Trigger::Workflow if run.attempt > 1 => matches!(
            prev.get(&(run.task_id.as_str(), run.attempt - 1)),
            Some(RunStatus::Failed | RunStatus::Cancelled)
        ),
        _ => false,
    }
}

/// Finished and rework. production.rs's *reworked*.
pub fn is_reworked(run: &Run, prev: &Predecessors<'_>) -> bool {
    is_finished(run) && is_rework(run, prev)
}

/// Finished `done` and not rework. production.rs's *first_pass* -- not
/// `!reworked`: a run scrapped first time round is neither.
pub fn is_first_pass(run: &Run, prev: &Predecessors<'_>) -> bool {
    is_finished(run) && run.status == RunStatus::Done && !is_rework(run, prev)
}

/// Finished `failed` -- narrower than scrapped: a cancel is not a failure.
pub fn is_failed(run: &Run) -> bool {
    is_finished(run) && run.status == RunStatus::Failed
}

fn seconds(d: Duration) -> f64 {
    d.num_milliseconds().max(0) as f64 / 1000.0
}

/// A done run's cycle time in seconds; `None` for anything else.
pub fn cycle_time(run: &Run) -> Option<f64> {
    match (run.status, run.ended_at) {
        (RunStatus::Done, Some(end)) => Some(seconds(end - run.started_at)),
        _ => None,
    }
}

/// How long the run waited to be dispatched; `None` for a run from before
/// `queued_at` was recorded -- unknown, not zero.
pub fn queue_wait(run: &Run) -> Option<f64> {
    run.queued_at.map(|q| seconds(run.started_at - q))
}

// =========================================================== percentiles

/// Nearest-rank percentile of `values` (any order), `None` when empty.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    Some(sorted[crate::scenario::nearest_rank(sorted.len(), p)])
}

/// The four lines of the Aging WIP chart, from finished runs' cycle times.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AgePercentiles {
    pub p50: f64,
    pub p70: f64,
    pub p85: f64,
    pub p95: f64,
    /// How many cycle times these came from.
    pub samples: u32,
}

/// `None` below [`MIN_HISTORY`] samples -- see the module doc.
pub fn age_percentiles(cycle_times: &[f64]) -> Option<AgePercentiles> {
    if cycle_times.len() < MIN_HISTORY {
        return None;
    }
    let p = |q| percentile(cycle_times, q).expect("non-empty");
    Some(AgePercentiles {
        p50: p(0.50),
        p70: p(0.70),
        p85: p(0.85),
        p95: p(0.95),
        samples: cycle_times.len() as u32,
    })
}

/// Where an age sits against the percentile lines -- the Aging WIP dot's
/// colour, green below p50 to red above p95. A value exactly on a line
/// reads as still below it, the same "not yet past it" `scenario.rs`'s
/// signposts give a value sitting on a threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pace {
    BelowP50,
    P50ToP70,
    P70ToP85,
    P85ToP95,
    AboveP95,
}

pub fn pace(age_s: f64, lines: &AgePercentiles) -> Pace {
    if age_s > lines.p95 {
        Pace::AboveP95
    } else if age_s > lines.p85 {
        Pace::P85ToP95
    } else if age_s > lines.p70 {
        Pace::P70ToP85
    } else if age_s > lines.p50 {
        Pace::P50ToP70
    } else {
        Pace::BelowP50
    }
}

/// Whose history a pace was judged against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaceBasis {
    /// The same task's own finished runs -- at least [`MIN_HISTORY`].
    Task,
    /// The task had too few, so its scope's.
    Scope,
    /// Neither had [`MIN_HISTORY`]: shown as "not enough history".
    NotEnoughHistory,
    /// Not paced at all: a task waiting to be dispatched has no cycle
    /// time to be compared against yet.
    NotPaced,
}

// ================================================================ window

/// A stretch of time, half-open at the start: `(from, to]`, so two windows
/// that share a boundary never both count the run that ended on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

impl Window {
    /// The `days` ending at `now`.
    pub fn trailing(now: DateTime<Utc>, days: i64) -> Self {
        Self { from: now - Duration::days(days), to: now }
    }

    pub fn contains(&self, at: DateTime<Utc>) -> bool {
        at > self.from && at <= self.to
    }

    fn days(&self) -> f64 {
        seconds(self.to - self.from) / 86_400.0
    }

    fn previous(&self) -> Self {
        let len = self.to - self.from;
        Self { from: self.from - len, to: self.from }
    }
}

fn ended_in(run: &Run, window: &Window) -> bool {
    run.ended_at.is_some_and(|end| window.contains(end))
}

/// Cycle times of runs that ended done in `window`.
pub fn cycle_times(runs: &[Run], window: &Window) -> Vec<f64> {
    runs.iter().filter(|r| ended_in(r, window)).filter_map(cycle_time).collect()
}

/// Queue waits of runs that started in `window` and record one.
pub fn queue_waits(runs: &[Run], window: &Window) -> Vec<f64> {
    runs.iter().filter(|r| window.contains(r.started_at)).filter_map(queue_wait).collect()
}

fn ratio(runs: &[Run], window: &Window, hit: impl Fn(&Run) -> bool) -> Option<f64> {
    let finished: Vec<&Run> = runs.iter().filter(|r| ended_in(r, window)).collect();
    if finished.is_empty() {
        return None;
    }
    Some(finished.iter().filter(|&&r| hit(r)).count() as f64 / finished.len() as f64)
}

/// failed/finished over `window`; `None` with nothing finished.
pub fn fail_rate(runs: &[Run], window: &Window) -> Option<f64> {
    ratio(runs, window, is_failed)
}

/// reworked/finished over `window`; `None` with nothing finished.
pub fn rework_rate(runs: &[Run], window: &Window) -> Option<f64> {
    let prev = predecessors(runs);
    ratio(runs, window, |r| is_reworked(r, &prev))
}

/// Every recovery that completed in `window`, in seconds -- see the module
/// doc for the definition. A streak whose start lies before the runs
/// passed in is measured from the earliest failure the caller handed over;
/// the daemon passes enough history for that to be rare.
pub fn recovery_times(runs: &[Run], window: &Window) -> Vec<f64> {
    recoveries(runs, window).into_iter().map(|(_, secs)| secs).collect()
}

/// [`recovery_times`] with the moment each recovery completed -- the done
/// run's end -- beside it, for [`registry_metric_as_of`].
fn recoveries(runs: &[Run], window: &Window) -> Vec<(DateTime<Utc>, f64)> {
    let mut by_task: BTreeMap<&str, Vec<&Run>> = BTreeMap::new();
    for run in runs.iter().filter(|r| is_finished(r)) {
        by_task.entry(run.task_id.as_str()).or_default().push(run);
    }
    let mut out = Vec::new();
    for (_, mut task_runs) in by_task {
        task_runs.sort_by_key(|r| (r.ended_at, r.attempt));
        let mut streak_start: Option<DateTime<Utc>> = None;
        for run in task_runs {
            let end = run.ended_at.expect("finished");
            match run.status {
                RunStatus::Failed => {
                    streak_start.get_or_insert(end);
                }
                RunStatus::Done => {
                    if let Some(start) = streak_start.take() {
                        if window.contains(end) {
                            out.push((end, seconds(end - start)));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

// ================================================================ figure

/// One number on the page, with how many samples it stands on and, when it
/// could not be computed (or only partly), why. `value: None` is "unknown",
/// never zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Figure {
    pub value: Option<f64>,
    pub samples: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Figure {
    fn of(value: Option<f64>, samples: usize, empty: &str) -> Self {
        Self {
            value,
            samples: samples as u32,
            reason: value.is_none().then(|| empty.to_string()),
        }
    }

    fn percentile_of(values: &[f64], p: f64, empty: &str) -> Self {
        Self::of(percentile(values, p), values.len(), empty)
    }

    /// A figure read off a field only newer runs record (`queued_at`):
    /// when the window reaches back before `since`, say so -- as the whole
    /// answer if nothing in the window records it, as a caveat otherwise.
    fn recorded(mut self, window: &Window, since: Option<DateTime<Utc>>) -> Self {
        match since {
            None if self.value.is_none() => {
                self.reason = Some("not recorded by any run yet".into());
            }
            Some(since) if since > window.from => {
                let date = since.format("%Y-%m-%d");
                self.reason = Some(if self.value.is_none() {
                    format!("no data before {date}")
                } else {
                    format!("only runs from {date} on record this; earlier runs are left out")
                });
            }
            _ => {}
        }
        self
    }
}

pub const METRIC_EMPTY_NO_DONE: &str = "no run ended done in the window";
pub const METRIC_EMPTY_NO_FINISHED: &str = "no finished runs in the window";
pub const METRIC_EMPTY_NO_WAIT: &str = "no run in the window records its queue wait";
pub const METRIC_EMPTY_NO_RECOVERY: &str = "no failure streak ended in a success in the window";

/// The registry metrics this module computes (`metrics::registry`'s
/// `cycle_time_p50` and neighbours), by id, over `window` -- the one entry
/// point `factory-daemon`'s metric wiring calls, so the Goals and Scenarios
/// tabs read exactly the numbers the Operations tab shows. `None` for an id
/// that is not one of them.
pub fn registry_metric(id: &str, runs: &[Run], window: &Window) -> Option<Figure> {
    let since = recorded_since(runs);
    Some(match id {
        "cycle_time_p50" => Figure::percentile_of(&cycle_times(runs, window), 0.50, METRIC_EMPTY_NO_DONE),
        "cycle_time_p85" => Figure::percentile_of(&cycle_times(runs, window), 0.85, METRIC_EMPTY_NO_DONE),
        "queue_wait_p95" => {
            Figure::percentile_of(&queue_waits(runs, window), 0.95, METRIC_EMPTY_NO_WAIT).recorded(window, since)
        }
        "fail_rate" => Figure::of(fail_rate(runs, window), finished_in(runs, window), METRIC_EMPTY_NO_FINISHED),
        "rework_rate" => Figure::of(rework_rate(runs, window), finished_in(runs, window), METRIC_EMPTY_NO_FINISHED),
        "time_to_recover_p50" => {
            Figure::percentile_of(&recovery_times(runs, window), 0.50, METRIC_EMPTY_NO_RECOVERY)
        }
        _ => return None,
    })
}

/// When the data behind [`registry_metric`]'s value for `id` is from: the
/// newest moment among the runs that value is computed from -- the end of
/// the newest run a cycle time, fail or rework rate counts, the start of the
/// newest run a queue wait counts (a wait is known once the run starts),
/// the end of the newest recovery. `None` when nothing in `window`
/// contributes, which is exactly when the value is `None` too, and for an
/// id that is not one of ours. The daemon sets `MetricValue::as_of` from
/// this, the rule the production ratios follow: a value is as old as the
/// data behind it, so a freshness window (a quality scenario's `max_age`)
/// can read a figure nothing has moved in weeks as stale.
pub fn registry_metric_as_of(id: &str, runs: &[Run], window: &Window) -> Option<DateTime<Utc>> {
    match id {
        "cycle_time_p50" | "cycle_time_p85" => runs
            .iter()
            .filter(|r| ended_in(r, window) && cycle_time(r).is_some())
            .filter_map(|r| r.ended_at)
            .max(),
        "queue_wait_p95" => runs
            .iter()
            .filter(|r| window.contains(r.started_at) && queue_wait(r).is_some())
            .map(|r| r.started_at)
            .max(),
        "fail_rate" | "rework_rate" => runs.iter().filter(|r| ended_in(r, window)).filter_map(|r| r.ended_at).max(),
        "time_to_recover_p50" => recoveries(runs, window).into_iter().map(|(end, _)| end).max(),
        _ => None,
    }
}

fn finished_in(runs: &[Run], window: &Window) -> usize {
    runs.iter().filter(|r| ended_in(r, window)).count()
}

/// The earliest start among runs that record `queued_at` -- which every run
/// made since these facts existed does, so it marks where the record of
/// queue waits, slots and fail kinds begins.
pub fn recorded_since(runs: &[Run]) -> Option<DateTime<Utc>> {
    runs.iter().filter(|r| r.queued_at.is_some()).map(|r| r.started_at).min()
}

// ================================================================= input

/// A `schedule_skipped` journal entry, as the daemon read it back: when it
/// was written, and the slots it names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkippedSlots {
    pub task_id: String,
    /// When the entry was written -- the late firing that passed them over.
    pub at: DateTime<Utc>,
    pub count: u32,
    pub first: DateTime<Utc>,
    pub last: DateTime<Utc>,
    /// `still_active` or `not_running`, as the entry recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A scenario signpost that is past its threshold -- `ScenariosReport`'s
/// triggered list, handed over as it stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggeredSignpost {
    pub scenario: String,
    pub metric: MetricId,
    pub reason: String,
}

/// The health windows the page offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthWindow {
    #[default]
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
}

impl HealthWindow {
    pub fn days(self) -> i64 {
        match self {
            Self::Week => 7,
            Self::Month => 30,
        }
    }
}

/// Everything [`report`] reads. The daemon decides how much history to
/// hand over -- enough finished runs for the percentiles and both health
/// windows -- and which journal facts are recent enough to matter.
#[derive(Debug, Clone, Default)]
pub struct OperationsInput<'a> {
    pub now: DateTime<Utc>,
    pub tasks: &'a [Task],
    /// Every open run, plus the finished history.
    pub runs: &'a [Run],
    pub agents: &'a [AgentSession],
    /// Narrow everything to one scope and every scope nested under it --
    /// the subtree the rail means everywhere else; `None` is every scope.
    pub scope: Option<ScopeFilter>,
    /// Hand over [`Health::days`] and [`Health::finished_runs`] -- the
    /// tab's charts. The Inbox and `factory stats` read the report without
    /// them, and should not pay for them.
    pub detail: bool,
    pub window: HealthWindow,
    /// Session capacity per scope, where the config states one. Nothing
    /// enforces `max_sessions` today, so this is shown, never acted on.
    pub capacity: BTreeMap<String, u32>,
    /// A blocked run's reason in its agent's own words, by run id -- the
    /// journal line its `blocked` report wrote.
    pub block_reasons: BTreeMap<String, String>,
    /// When each open run last said anything, by run id: its newest
    /// journal entry. Silence past the p95 of its pace basis is a suspicion.
    pub last_progress: BTreeMap<String, DateTime<Utc>>,
    /// `schedule_skipped` entries recent enough to still need a look.
    pub skipped: Vec<SkippedSlots>,
    pub signposts: Vec<TriggeredSignpost>,
    /// When the owner answered a blocked run (`run.answer`, journaled as
    /// `answer` by the owner) -- see "intervention" in the module doc.
    pub answers: Vec<DateTime<Utc>>,
    /// Manual runs an agent asked for, as `(task id, queued_at)` -- the
    /// `run_requested` entries an agent's `task.run` wrote. Not a person's
    /// "run again", so not an intervention.
    pub agent_runs: BTreeSet<(String, DateTime<Utc>)>,
    /// How long past its slot a due task may wait before it is late;
    /// `None` uses [`DEFAULT_LATE_AFTER_SECONDS`].
    pub late_after_seconds: Option<i64>,
}

/// The scope a report was asked for, and the subtree it covers: that scope
/// and every scope nested under it, by name. Which scopes are nested is the
/// config's to say (`Config::ancestors_of`), so the daemon works it out and
/// hands the names over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeFilter {
    pub name: String,
    pub subtree: BTreeSet<String>,
}

impl ScopeFilter {
    /// Just `name` -- a scope with nothing nested under it.
    pub fn exactly(name: &str) -> Self {
        Self { name: name.to_string(), subtree: [name.to_string()].into_iter().collect() }
    }

    pub fn covers(&self, scope: &str) -> bool {
        self.subtree.contains(scope)
    }
}

// ================================================================ report

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationsReport {
    pub generated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Sorted by severity, then age, oldest first.
    pub attention: Vec<Exception>,
    /// One per scope with anything in flight, queued or retrying.
    pub flow: Vec<Flow>,
    pub aging: Aging,
    pub health: HealthReport,
    pub schedules: Vec<ScheduleRow>,
    /// Where the record of queue waits, slots and fail kinds begins --
    /// see "No data is not zero" in the module doc. `None` when no run
    /// handed over records them yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExceptionKind {
    /// The run is `Blocked`, asserted by its agent or a lifecycle hook.
    Blocked,
    /// The runtime's screen-read thinks the run may be waiting, or it has
    /// said nothing for longer than its p95 -- a suspicion, never a status.
    SuspectedStuck,
    /// The newest run failed and no retry is left. It stays until a newer
    /// run replaces it, so how far back a failure can surface here is the
    /// history the daemon chooses to hand over.
    FailedExhausted,
    /// Running past the p85 (medium) or p95 (high) of its pace basis.
    Aging,
    /// A slot passed and the task was not dispatched.
    ScheduleLate,
    /// Slots passed over without firing -- `schedule_skipped`.
    ScheduleMissed,
    /// A permanent agent's session is gone.
    LivenessLost,
    /// A scenario signpost triggered -- an observation, not a fault.
    TriggeredSignpost,
}

impl ExceptionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::SuspectedStuck => "suspected_stuck",
            Self::FailedExhausted => "failed_exhausted",
            Self::Aging => "aging",
            Self::ScheduleLate => "schedule_late",
            Self::ScheduleMissed => "schedule_missed",
            Self::LivenessLost => "liveness_lost",
            Self::TriggeredSignpost => "triggered_signpost",
        }
    }
}

/// What a person may do about an exception from where it is shown. Each
/// maps onto a request (`task.run`, `task.cancel`, `run.answer`,
/// `task.skip_next`, or `task.update`'s `schedule_paused`); none of them is
/// taken here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// `task.run`, trigger `manual`. Spends a model call.
    RunAgain,
    /// `task.cancel`. Counts as scrap.
    Cancel,
    /// `run.input` into the run's own session.
    Answer,
    /// `task.run` for a scheduled task, ahead of its slot.
    RunNow,
    /// `task.skip_next`: pass over the schedule's next slot.
    SkipNext,
    PauseSchedule,
    ResumeSchedule,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exception {
    pub kind: ExceptionKind,
    pub severity: Severity,
    /// The scope it belongs to; `None` for a signpost, which belongs to a
    /// scenario, and for a run whose task is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The standing agent, for `liveness_lost`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// When the condition began; `age_s` is `now` minus this.
    pub since: DateTime<Utc>,
    pub age_s: f64,
    /// Why, in the agent's own words when there are any.
    pub reason: String,
    pub actions: Vec<Action>,
    /// A guess, labelled as one wherever it is shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suspicion: bool,
    /// Something noticed, not something wrong.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub observation: bool,
    /// Other kinds the same run also matched -- a run appears once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<ExceptionKind>,
}

/// Work in flight by state, for one scope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wip {
    /// Due and not dispatched yet.
    pub queued: u32,
    pub dispatching: u32,
    pub running: u32,
    pub blocked: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flow {
    pub scope: String,
    pub wip: Wip,
    /// `wip.queued`, named for what the page calls it.
    pub queue_depth: u32,
    /// Over runs started in the health window.
    pub wait_p50: Figure,
    pub wait_p95: Figure,
    /// Open runs -- each holds one session.
    pub sessions_in_use: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions_max: Option<u32>,
    /// Tasks with a retry queued: failing, and not yet out of retries.
    pub retrying: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Queued,
    Dispatching,
    Running,
    Blocked,
}

/// One dot on the Aging WIP chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgingItem {
    /// `None` for a task that is due and not dispatched: there is no run yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub task_id: String,
    pub title: String,
    pub scope: String,
    pub stage: Stage,
    pub age_s: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<Pace>,
    pub basis: PaceBasis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopePercentiles {
    pub scope: String,
    /// `None`: fewer than [`MIN_HISTORY`] runs ended done in this scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<AgePercentiles>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Aging {
    /// Oldest first.
    pub items: Vec<AgingItem>,
    pub percentiles: Vec<ScopePercentiles>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub window: Window,
    pub finished: u32,
    pub throughput_day: Figure,
    pub cycle_p50: Figure,
    pub cycle_p85: Figure,
    pub first_pass_yield: Figure,
    pub rework_rate: Figure,
    pub scrap_rate: Figure,
    pub fail_rate: Figure,
    /// Failed and cancelled runs by why. Only runs that record a kind.
    pub fail_by_kind: BTreeMap<FailKind, u32>,
    /// Failed or cancelled runs from before `fail_kind` was recorded.
    pub unclassified: u32,
    pub recover_p50: Figure,
    pub queue_wait_p50: Figure,
    pub queue_wait_p95: Figure,
    pub interventions: u32,
    pub interventions_per_100: Figure,
    /// The window cut into 24-hour steps from its start, oldest first -- the
    /// small multiples' lines and the cumulative flow diagram. Only with
    /// [`OperationsInput::detail`]. Steps, not
    /// calendar days: a window ends now, and each step of it lines up with
    /// the same step of the window before, which is what a ghost line is
    /// compared against.
    #[serde(default)]
    pub days: Vec<HealthDay>,
    /// Every run that finished in the window, newest first, at most
    /// [`FINISHED_RUNS_CAP`] -- the cycle-time scatter's dots. Only the
    /// current window carries them, and only with
    /// [`OperationsInput::detail`]; a ghost of a scatter is noise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub finished_runs: Vec<FinishedRun>,
}

/// How many finished runs one report hands over for the scatter.
pub const FINISHED_RUNS_CAP: usize = 2000;

/// One 24-hour step of a health window. The counts are the same words
/// [`Health`]'s figures are made of, so a step's line and the window's
/// number cannot disagree; `waiting` and `in_progress` are how many runs
/// stood in each state at the step's end -- the two upper bands of a
/// cumulative flow diagram, over the cumulative `finished` below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthDay {
    /// The step's end; it covers the 24 hours before it.
    pub to: DateTime<Utc>,
    pub finished: u32,
    pub done: u32,
    pub scrapped: u32,
    pub failed: u32,
    pub reworked: u32,
    pub first_pass: u32,
    /// Queued and not yet started at `to` -- only a run that records
    /// `queued_at` can say it was. The step ending now counts the queue
    /// instead ([`Flow::queue_depth`]): a run exists only once dispatched,
    /// so nothing on record is waiting at this instant.
    pub waiting: u32,
    /// Started and not yet ended at `to`.
    pub in_progress: u32,
}

/// A run that finished in the window: a dot on the cycle-time scatter when
/// it ended done, a count in the page's "since you last looked" either way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishedRun {
    pub run_id: String,
    pub task_id: String,
    pub ended_at: DateTime<Utc>,
    pub status: RunStatus,
    /// Only for a run that ended done -- see [`cycle_time`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle_s: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthReport {
    pub window: HealthWindow,
    pub current: Health,
    /// The same length of time just before it -- the ghost line.
    pub previous: Health,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleState {
    /// Waiting for its next slot.
    Due,
    /// The slot passed and it has not been dispatched.
    Late,
    /// Slots were recently passed over without firing.
    Missed,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleRow {
    pub task_id: String,
    pub title: String,
    pub scope: String,
    /// How the schedule reads: `0 9 * * 1`, `every 600s`.
    pub schedule: String,
    /// The cron's timezone; `None` is UTC (or an `every` schedule, which no
    /// timezone changes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
    pub state: ScheduleState,
    /// Slots passed over in the entries handed in.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skipped: u32,
    /// How late its newest scheduled firing started (`started_at -
    /// scheduled_for`); `None` before slots were recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_late_s: Option<f64>,
    /// A retry is queued and will fire first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retrying: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

// ================================================================= build

/// The whole report. See the module doc for what each part means.
pub fn report(input: &OperationsInput<'_>) -> OperationsReport {
    let now = input.now;
    let late_after = Duration::seconds(input.late_after_seconds.unwrap_or(DEFAULT_LATE_AFTER_SECONDS));

    let tasks: BTreeMap<&str, &Task> = input
        .tasks
        .iter()
        .filter(|t| input.scope.as_ref().is_none_or(|s| s.covers(&t.scope)))
        .map(|t| (t.id.as_str(), t))
        .collect();
    // A run carries no scope of its own: it is joined through its task, and
    // one whose task is gone belongs to no scope -- counted only when no
    // scope is asked for, production.rs's stance on the same join.
    let runs: Vec<&Run> = input
        .runs
        .iter()
        .filter(|r| match &input.scope {
            None => true,
            Some(_) => tasks.contains_key(r.task_id.as_str()),
        })
        .collect();
    let owned: Vec<Run> = runs.iter().map(|r| (*r).clone()).collect();
    let scope_of = |task_id: &str| tasks.get(task_id).map(|t| t.scope.clone());

    // -- pace history ----------------------------------------------------
    let mut task_cycles: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut scope_cycles: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for run in &runs {
        if let Some(ct) = cycle_time(run) {
            task_cycles.entry(run.task_id.as_str()).or_default().push(ct);
            if let Some(scope) = scope_of(&run.task_id) {
                scope_cycles.entry(scope).or_default().push(ct);
            }
        }
    }
    let scope_lines: BTreeMap<String, Option<AgePercentiles>> =
        scope_cycles.iter().map(|(s, c)| (s.clone(), age_percentiles(c))).collect();
    let basis_for = |task_id: &str, scope: Option<&str>| -> (PaceBasis, Option<AgePercentiles>) {
        if let Some(lines) = task_cycles.get(task_id).and_then(|c| age_percentiles(c)) {
            return (PaceBasis::Task, Some(lines));
        }
        match scope.and_then(|s| scope_lines.get(s).copied().flatten()) {
            Some(lines) => (PaceBasis::Scope, Some(lines)),
            None => (PaceBasis::NotEnoughHistory, None),
        }
    };

    let mut attention: Vec<Exception> = Vec::new();
    let mut aging_items: Vec<AgingItem> = Vec::new();
    let mut flow: BTreeMap<String, Flow> = BTreeMap::new();
    let flow_for = |flow: &mut BTreeMap<String, Flow>, scope: &str| -> Flow {
        flow.remove(scope).unwrap_or_else(|| Flow {
            scope: scope.to_string(),
            wip: Wip::default(),
            queue_depth: 0,
            wait_p50: Figure::of(None, 0, METRIC_EMPTY_NO_WAIT),
            wait_p95: Figure::of(None, 0, METRIC_EMPTY_NO_WAIT),
            sessions_in_use: 0,
            sessions_max: input.capacity.get(scope).copied(),
            retrying: 0,
        })
    };

    // -- open runs: flow, aging, and their exceptions ----------------------
    let open_tasks: BTreeSet<&str> =
        runs.iter().filter(|r| !r.status.is_terminal()).map(|r| r.task_id.as_str()).collect();
    for run in runs.iter().filter(|r| !r.status.is_terminal()) {
        let task = tasks.get(run.task_id.as_str());
        let scope = task.map(|t| t.scope.clone()).unwrap_or_default();
        let title = task.map(|t| t.title.clone()).unwrap_or_else(|| run.task_id.clone());
        let mut f = flow_for(&mut flow, &scope);
        f.sessions_in_use += 1;
        let stage = match run.status {
            RunStatus::Dispatching => {
                f.wip.dispatching += 1;
                Stage::Dispatching
            }
            RunStatus::Blocked => {
                f.wip.blocked += 1;
                Stage::Blocked
            }
            _ => {
                f.wip.running += 1;
                Stage::Running
            }
        };
        flow.insert(scope.clone(), f);

        let age = seconds(now - run.started_at);
        let (basis, lines) = basis_for(&run.task_id, task.map(|t| t.scope.as_str()));
        aging_items.push(AgingItem {
            run_id: Some(run.id.clone()),
            task_id: run.task_id.clone(),
            title: title.clone(),
            scope: scope.clone(),
            stage,
            age_s: age,
            pace: lines.as_ref().map(|l| pace(age, l)),
            basis,
        });

        // Every exception this run matches, most severe kept.
        let mut found: Vec<Exception> = Vec::new();
        let base = |kind, severity, since: DateTime<Utc>, reason: String, actions: Vec<Action>| Exception {
            kind,
            severity,
            scope: task.map(|t| t.scope.clone()),
            task_id: Some(run.task_id.clone()),
            title: Some(title.clone()),
            run_id: Some(run.id.clone()),
            agent: None,
            since,
            age_s: seconds(now - since),
            reason,
            actions,
            suspicion: false,
            observation: false,
            also: Vec::new(),
        };
        if run.status == RunStatus::Blocked {
            let since = run.blocked_since.unwrap_or(run.started_at);
            let reason = input
                .block_reasons
                .get(&run.id)
                .cloned()
                .unwrap_or_else(|| "blocked, waiting for a human".into());
            found.push(base(ExceptionKind::Blocked, Severity::High, since, reason, vec![Action::Answer, Action::Cancel]));
        } else {
            if let Some(since) = run.block_suspected_since {
                let mut e = base(
                    ExceptionKind::SuspectedStuck,
                    Severity::Medium,
                    since,
                    "the runtime thinks this may be waiting for input; the agent has not said so".into(),
                    vec![Action::Cancel],
                );
                e.suspicion = true;
                found.push(e);
            } else if let (Some(last), Some(lines)) = (input.last_progress.get(&run.id), lines.as_ref()) {
                let silent = seconds(now - *last);
                if silent > lines.p95 {
                    let mut e = base(
                        ExceptionKind::SuspectedStuck,
                        Severity::Medium,
                        *last,
                        format!(
                            "nothing said for {}, longer than the p95 of {} ({})",
                            human(silent),
                            basis_words(basis),
                            human(lines.p95)
                        ),
                        vec![Action::Cancel],
                    );
                    e.suspicion = true;
                    found.push(e);
                }
            }
            if let Some(lines) = lines.as_ref() {
                let severity = match pace(age, lines) {
                    Pace::AboveP95 => Some((Severity::High, "p95", lines.p95)),
                    Pace::P85ToP95 => Some((Severity::Medium, "p85", lines.p85)),
                    _ => None,
                };
                if let Some((severity, line, value)) = severity {
                    found.push(base(
                        ExceptionKind::Aging,
                        severity,
                        run.started_at,
                        format!(
                            "running for {}, past the {line} of {} ({})",
                            human(age),
                            basis_words(basis),
                            human(value)
                        ),
                        vec![Action::Cancel],
                    ));
                }
            }
        }
        if let Some(e) = keep_worst(found) {
            attention.push(e);
        }
    }

    // -- tasks: queued, retrying, exhausted, schedules --------------------
    let mut newest: BTreeMap<&str, &Run> = BTreeMap::new();
    for &run in &runs {
        let slot = newest.entry(run.task_id.as_str()).or_insert(run);
        if run.attempt > slot.attempt {
            *slot = run;
        }
    }
    let mut skipped_by_task: BTreeMap<&str, Vec<&SkippedSlots>> = BTreeMap::new();
    for s in &input.skipped {
        if tasks.contains_key(s.task_id.as_str()) {
            skipped_by_task.entry(s.task_id.as_str()).or_default().push(s);
        }
    }
    let mut last_late: BTreeMap<&str, (DateTime<Utc>, f64)> = BTreeMap::new();
    for run in &runs {
        if let Some(slot) = run.scheduled_for {
            let late = seconds(run.started_at - slot);
            let entry = last_late.entry(run.task_id.as_str()).or_insert((run.started_at, late));
            if run.started_at > entry.0 {
                *entry = (run.started_at, late);
            }
        }
    }

    let mut schedules = Vec::new();
    for task in tasks.values() {
        let active = open_tasks.contains(task.id.as_str());
        if task.pending_retry.is_some() {
            let mut f = flow_for(&mut flow, &task.scope);
            f.retrying += 1;
            flow.insert(task.scope.clone(), f);
        }

        // Due and not dispatched: the queue.
        let overdue = task
            .next_run_at
            .filter(|at| *at <= now && task.schedule.is_some() && !task.schedule_paused && !active);
        if let Some(at) = overdue {
            if task.status == TaskStatus::Pending {
                let mut f = flow_for(&mut flow, &task.scope);
                f.wip.queued += 1;
                f.queue_depth += 1;
                flow.insert(task.scope.clone(), f);
                aging_items.push(AgingItem {
                    run_id: None,
                    task_id: task.id.clone(),
                    title: task.title.clone(),
                    scope: task.scope.clone(),
                    stage: Stage::Queued,
                    age_s: seconds(now - at),
                    pace: None,
                    basis: PaceBasis::NotPaced,
                });
            }
        }

        // The dead set: failed, and nothing will try again on its own. A
        // bench attempt's failure is the benchmark's data, not the line's.
        if let Some(run) = newest.get(task.id.as_str()) {
            if run.status == RunStatus::Failed
                && task.pending_retry.is_none()
                && !active
                && task.bench_origin.is_none()
            {
                let since = run.ended_at.unwrap_or(run.started_at);
                let mut actions = vec![Action::RunAgain];
                if task.schedule.is_some() && !task.schedule_paused {
                    actions.push(Action::PauseSchedule);
                }
                let why = run.error.clone().unwrap_or_else(|| "failed".into());
                let reason = if task.schedule.is_some() {
                    format!("attempt {} failed with no retry left: {why}", run.attempt)
                } else {
                    format!("attempt {} failed: {why}", run.attempt)
                };
                attention.push(Exception {
                    kind: ExceptionKind::FailedExhausted,
                    severity: Severity::High,
                    scope: Some(task.scope.clone()),
                    task_id: Some(task.id.clone()),
                    title: Some(task.title.clone()),
                    run_id: Some(run.id.clone()),
                    agent: None,
                    since,
                    age_s: seconds(now - since),
                    reason,
                    actions,
                    suspicion: false,
                    observation: false,
                    also: Vec::new(),
                });
            }
        }

        let Some(schedule) = &task.schedule else { continue };
        let skipped = skipped_by_task.get(task.id.as_str());
        let skipped_count: u32 = skipped.map(|v| v.iter().map(|s| s.count).sum()).unwrap_or(0);
        let late_at = task.next_run_at.filter(|at| *at + late_after < now);
        let state = if task.schedule_paused {
            ScheduleState::Paused
        } else if late_at.is_some() {
            ScheduleState::Late
        } else if skipped_count > 0 {
            ScheduleState::Missed
        } else {
            ScheduleState::Due
        };
        let (schedule_text, timezone) = describe(schedule);
        schedules.push(ScheduleRow {
            task_id: task.id.clone(),
            title: task.title.clone(),
            scope: task.scope.clone(),
            schedule: schedule_text,
            timezone,
            next_run_at: task.next_run_at,
            state,
            skipped: skipped_count,
            last_late_s: last_late.get(task.id.as_str()).map(|(_, late)| *late),
            retrying: task.pending_retry.is_some(),
        });

        if task.schedule_paused {
            continue;
        }
        let schedule_actions = |active: bool| {
            let mut a = Vec::new();
            if !active {
                a.push(Action::RunNow);
            }
            a.push(Action::SkipNext);
            a.push(Action::PauseSchedule);
            a
        };
        if let Some(at) = late_at {
            // With a retry queued, `next_run_at` is the retry's time, not
            // one of the schedule's slots -- say which it is.
            let what = if task.pending_retry.is_some() {
                format!("the retry due at {}", at.to_rfc3339())
            } else {
                format!("the slot at {}", at.to_rfc3339())
            };
            let reason = if active {
                format!("{what} passed while the previous run was still going")
            } else {
                format!("{what} passed and nothing was dispatched")
            };
            attention.push(Exception {
                kind: ExceptionKind::ScheduleLate,
                severity: Severity::Medium,
                scope: Some(task.scope.clone()),
                task_id: Some(task.id.clone()),
                title: Some(task.title.clone()),
                run_id: None,
                agent: None,
                since: at,
                age_s: seconds(now - at),
                reason,
                actions: schedule_actions(active),
                suspicion: false,
                observation: false,
                also: Vec::new(),
            });
        }
        if let Some(entries) = skipped.filter(|_| skipped_count > 0) {
            let first = entries.iter().map(|s| s.first).min().expect("non-empty");
            let last = entries.iter().map(|s| s.last).max().expect("non-empty");
            let at = entries.iter().map(|s| s.at).max().expect("non-empty");
            // Counted per reason rather than letting one entry's reason
            // speak for all of them: a night of downtime and one long run
            // are different stories, and a person should see both.
            let mut by_reason: BTreeMap<&str, u32> = BTreeMap::new();
            for s in entries.iter() {
                let why = match s.reason.as_deref() {
                    Some("still_active") => "the previous run was still going",
                    Some("not_running") => "nothing was running to fire them",
                    _ => "no reason recorded",
                };
                *by_reason.entry(why).or_default() += s.count;
            }
            let why = if by_reason.len() == 1 {
                by_reason.keys().next().expect("non-empty").to_string()
            } else {
                by_reason.iter().map(|(why, n)| format!("{n} as {why}")).collect::<Vec<_>>().join("; ")
            };
            attention.push(Exception {
                kind: ExceptionKind::ScheduleMissed,
                severity: Severity::Medium,
                scope: Some(task.scope.clone()),
                task_id: Some(task.id.clone()),
                title: Some(task.title.clone()),
                run_id: None,
                agent: None,
                since: at,
                age_s: seconds(now - at),
                reason: format!(
                    "{skipped_count} slot{} between {} and {} never fired: {why}",
                    if skipped_count == 1 { "" } else { "s" },
                    first.to_rfc3339(),
                    last.to_rfc3339()
                ),
                actions: schedule_actions(active),
                suspicion: false,
                observation: false,
                also: Vec::new(),
            });
        }
    }

    // -- standing agents: session only, never silence ---------------------
    for agent in input.agents {
        if agent.lifetime != Lifetime::Permanent || agent.state != AgentState::Gone || !agent.declared {
            continue;
        }
        if input.scope.as_ref().is_some_and(|s| !s.covers(&agent.scope)) {
            continue;
        }
        attention.push(Exception {
            kind: ExceptionKind::LivenessLost,
            severity: Severity::High,
            scope: Some(agent.scope.clone()),
            task_id: None,
            title: None,
            run_id: None,
            agent: Some(agent.name.clone()),
            since: agent.last_seen_at,
            age_s: seconds(now - agent.last_seen_at),
            reason: agent
                .error
                .clone()
                .unwrap_or_else(|| "the permanent agent's session is gone".into()),
            actions: Vec::new(),
            suspicion: false,
            observation: false,
            also: Vec::new(),
        });
    }

    // -- signposts: observations ------------------------------------------
    for sp in &input.signposts {
        attention.push(Exception {
            kind: ExceptionKind::TriggeredSignpost,
            severity: Severity::Low,
            scope: None,
            task_id: None,
            title: Some(sp.scenario.clone()),
            run_id: None,
            agent: None,
            since: now,
            age_s: 0.0,
            reason: format!("{}: {}", sp.metric, sp.reason),
            actions: Vec::new(),
            suspicion: false,
            observation: true,
            also: Vec::new(),
        });
    }

    sort_attention(&mut attention);
    aging_items.sort_by(|a, b| b.age_s.total_cmp(&a.age_s).then_with(|| a.task_id.cmp(&b.task_id)));

    // -- health and the flow's waits ---------------------------------------
    let since = recorded_since(&owned);
    let current = Window::trailing(now, input.window.days());
    let mut waits_by_scope: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for run in owned.iter().filter(|r| current.contains(r.started_at)) {
        if let (Some(wait), Some(scope)) = (queue_wait(run), scope_of(&run.task_id)) {
            waits_by_scope.entry(scope).or_default().push(wait);
        }
    }
    let mut flow: Vec<Flow> = flow.into_values().collect();
    for f in &mut flow {
        let waits = waits_by_scope.get(&f.scope).cloned().unwrap_or_default();
        f.wait_p50 = Figure::percentile_of(&waits, 0.50, METRIC_EMPTY_NO_WAIT).recorded(&current, since);
        f.wait_p95 = Figure::percentile_of(&waits, 0.95, METRIC_EMPTY_NO_WAIT).recorded(&current, since);
    }

    let charts = input.detail;
    let prev_runs = predecessors(&owned);
    let queued_now: u32 = flow.iter().map(|f| f.wip.queued).sum();

    let percentiles = scope_lines
        .into_iter()
        .map(|(scope, lines)| ScopePercentiles { scope, lines })
        .collect();

    OperationsReport {
        generated_at: now,
        scope: input.scope.as_ref().map(|s| s.name.clone()),
        attention,
        flow,
        aging: Aging { items: aging_items, percentiles },
        health: HealthReport {
            window: input.window,
            current: Health {
                days: if charts {
                    let mut days = health_days(&owned, &current, &prev_runs);
                    // The step ending now: a run is only made at dispatch,
                    // so nothing on record is "queued and not started" at
                    // this instant -- what is waiting now is the queue,
                    // tasks due and not dispatched, counted above.
                    if let Some(last) = days.last_mut() {
                        last.waiting = queued_now;
                    }
                    days
                } else {
                    Vec::new()
                },
                finished_runs: if charts {
                    let ended: Vec<&Run> = owned.iter().filter(|r| ended_in(r, &current)).collect();
                    finished_runs(&ended)
                } else {
                    Vec::new()
                },
                ..health(&owned, &current, since, &input.answers, &input.agent_runs)
            },
            // The ghost gets its steps for the lines; a ghost of a scatter
            // is noise, so it gets no runs, and none are computed for it.
            previous: Health {
                days: if charts { health_days(&owned, &current.previous(), &prev_runs) } else { Vec::new() },
                ..health(&owned, &current.previous(), since, &input.answers, &input.agent_runs)
            },
        },
        schedules,
        recorded_since: since,
    }
}

/// The health strip for one window.
pub fn health(
    runs: &[Run],
    window: &Window,
    since: Option<DateTime<Utc>>,
    answers: &[DateTime<Utc>],
    agent_runs: &BTreeSet<(String, DateTime<Utc>)>,
) -> Health {
    let finished: Vec<&Run> = runs.iter().filter(|r| ended_in(r, window)).collect();
    let n = finished.len();
    let prev = predecessors(runs);
    let share = |hit: &dyn Fn(&Run) -> bool| {
        Figure::of(
            (n > 0).then(|| finished.iter().filter(|&&r| hit(r)).count() as f64 / n as f64),
            n,
            METRIC_EMPTY_NO_FINISHED,
        )
    };

    let mut fail_by_kind: BTreeMap<FailKind, u32> = BTreeMap::new();
    let mut unclassified = 0;
    for run in finished.iter().filter(|r| is_scrapped(r)) {
        match run.fail_kind {
            Some(kind) => *fail_by_kind.entry(kind).or_default() += 1,
            None => unclassified += 1,
        }
    }

    let interventions = interventions(runs, window, answers, agent_runs);
    let cycles = cycle_times(runs, window);
    let waits = queue_waits(runs, window);
    Health {
        window: *window,
        finished: n as u32,
        throughput_day: Figure {
            value: Some(n as f64 / window.days().max(f64::EPSILON)),
            samples: n as u32,
            reason: None,
        },
        cycle_p50: Figure::percentile_of(&cycles, 0.50, METRIC_EMPTY_NO_DONE),
        cycle_p85: Figure::percentile_of(&cycles, 0.85, METRIC_EMPTY_NO_DONE),
        first_pass_yield: share(&|r| is_first_pass(r, &prev)),
        rework_rate: share(&|r| is_reworked(r, &prev)),
        scrap_rate: share(&is_scrapped),
        fail_rate: share(&is_failed),
        fail_by_kind,
        unclassified,
        recover_p50: Figure::percentile_of(&recovery_times(runs, window), 0.50, METRIC_EMPTY_NO_RECOVERY),
        queue_wait_p50: Figure::percentile_of(&waits, 0.50, METRIC_EMPTY_NO_WAIT).recorded(window, since),
        queue_wait_p95: Figure::percentile_of(&waits, 0.95, METRIC_EMPTY_NO_WAIT).recorded(window, since),
        interventions,
        interventions_per_100: Figure::of(
            (n > 0).then(|| interventions as f64 * 100.0 / n as f64),
            n,
            METRIC_EMPTY_NO_FINISHED,
        ),
        days: Vec::new(),
        finished_runs: Vec::new(),
    }
}

/// [`Health::days`]: `window` in 24-hour steps from its start, the last one
/// cut short at the window's end when the window is not whole days long.
pub fn health_days(runs: &[Run], window: &Window, prev: &Predecessors<'_>) -> Vec<HealthDay> {
    let step = Duration::days(1);
    let mut out = Vec::new();
    let mut from = window.from;
    while from < window.to {
        let to = (from + step).min(window.to);
        let slice = Window { from, to };
        let ended: Vec<&Run> = runs.iter().filter(|r| ended_in(r, &slice)).collect();
        let count = |hit: &dyn Fn(&Run) -> bool| ended.iter().filter(|&&r| hit(r)).count() as u32;
        // Standing at `to`: a run is in a state from the moment it entered
        // it up to, not including, the moment it left.
        let open_at = |r: &Run| r.ended_at.is_none_or(|end| end > to);
        out.push(HealthDay {
            to,
            finished: ended.len() as u32,
            done: count(&|r| r.status == RunStatus::Done),
            scrapped: count(&is_scrapped),
            failed: count(&is_failed),
            reworked: count(&|r| is_reworked(r, prev)),
            first_pass: count(&|r| is_first_pass(r, prev)),
            waiting: runs
                .iter()
                .filter(|r| r.queued_at.is_some_and(|q| q <= to) && r.started_at > to && open_at(r))
                .count() as u32,
            in_progress: runs.iter().filter(|r| r.started_at <= to && open_at(r)).count() as u32,
        });
        from = to;
    }
    out
}

/// [`Health::finished_runs`], newest first and capped.
fn finished_runs(finished: &[&Run]) -> Vec<FinishedRun> {
    let mut out: Vec<FinishedRun> = finished
        .iter()
        .map(|r| FinishedRun {
            run_id: r.id.clone(),
            task_id: r.task_id.clone(),
            ended_at: r.ended_at.expect("finished"),
            status: r.status,
            cycle_s: cycle_time(r),
        })
        .collect();
    out.sort_by(|a, b| b.ended_at.cmp(&a.ended_at).then_with(|| a.run_id.cmp(&b.run_id)));
    out.truncate(FINISHED_RUNS_CAP);
    out
}

/// Interventions in `window`, as the module doc defines them.
pub fn interventions(
    runs: &[Run],
    window: &Window,
    answers: &[DateTime<Utc>],
    agent_runs: &BTreeSet<(String, DateTime<Utc>)>,
) -> u32 {
    // A person's "run again" is exactly production.rs's manual rework --
    // less the ones an agent asked for.
    let prev = predecessors(runs);
    let by_agent = |r: &Run| r.queued_at.is_some_and(|q| agent_runs.contains(&(r.task_id.clone(), q)));
    let run_again = runs
        .iter()
        .filter(|r| r.trigger == Trigger::Manual && window.contains(r.started_at) && is_rework(r, &prev))
        .filter(|r| !by_agent(r))
        .count();
    let cancels = runs
        .iter()
        .filter(|r| r.fail_kind == Some(FailKind::CancelledByPerson) && ended_in(r, window))
        .count();
    let answered = answers.iter().filter(|at| window.contains(**at)).count();
    (run_again + cancels + answered) as u32
}

/// The most severe of a run's exceptions, the rest named in `also`. Ties go
/// to the more specific kind -- `ExceptionKind`'s own order.
fn keep_worst(mut found: Vec<Exception>) -> Option<Exception> {
    found.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.kind.cmp(&b.kind)));
    let mut iter = found.into_iter();
    let mut worst = iter.next()?;
    worst.also = iter.map(|e| e.kind).collect();
    Some(worst)
}

/// Severity first, then the oldest first; the rest only to keep the order
/// stable between two reads of the same state.
pub fn sort_attention(attention: &mut [Exception]) {
    attention.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(b.age_s.total_cmp(&a.age_s))
            .then(a.kind.cmp(&b.kind))
            .then(a.task_id.cmp(&b.task_id))
            .then(a.run_id.cmp(&b.run_id))
            .then(a.agent.cmp(&b.agent))
    });
}

fn describe(schedule: &Schedule) -> (String, Option<String>) {
    match schedule {
        Schedule::Cron(c) => (c.expr.clone(), c.timezone.clone()),
        Schedule::Every { seconds } => (format!("every {seconds}s"), None),
    }
}

fn basis_words(basis: PaceBasis) -> &'static str {
    match basis {
        PaceBasis::Task => "this task's finished runs",
        _ => "its scope's finished runs",
    }
}

/// `90s`, `12m`, `3.5h`, `2.0d` -- enough for a reason line.
fn human(secs: f64) -> String {
    if secs < 120.0 {
        format!("{secs:.0}s")
    } else if secs < 7200.0 {
        format!("{:.0}m", secs / 60.0)
    } else if secs < 172_800.0 {
        format!("{:.1}h", secs / 3600.0)
    } else {
        format!("{:.1}d", secs / 86_400.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Role;
    use crate::task::{CronSchedule, PendingRetry};

    fn now() -> DateTime<Utc> {
        "2026-09-25T12:00:00Z".parse().unwrap()
    }

    /// `mins` minutes before `now()`.
    fn ago(mins: i64) -> DateTime<Utc> {
        now() - Duration::minutes(mins)
    }

    fn task(id: &str, scope: &str) -> Task {
        Task {
            id: id.into(),
            title: format!("title of {id}"),
            instructions: String::new(),
            scope: scope.into(),
            agent: "shell".into(),
            runtime: "herdr".into(),
            status: TaskStatus::Pending,
            schedule: None,
            estimate_seconds: None,
            result: None,
            error: None,
            runs: 0,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            blocked_timeout_seconds: None,
            labels: Default::default(),
            created_at: ago(100_000),
            updated_at: ago(100_000),
            last_run_at: None,
            next_run_at: None,
            worktree: false,
            knowledge_hints: false,
            workflow_origin: None,
            bench_origin: None,
            retry: None,
            pending_retry: None,
            schedule_paused: false,
            category: None,
            intake: None,
            failure: None,
            closure: None,
        }
    }

    fn scheduled(mut t: Task, next: DateTime<Utc>) -> Task {
        t.schedule = Some(Schedule::Cron(CronSchedule {
            expr: "0 9 * * 1".into(),
            timezone: Some("Europe/Berlin".into()),
        }));
        t.next_run_at = Some(next);
        t
    }

    /// A run of `task` that started `started` minutes ago and, with `ended`,
    /// finished that many minutes ago. Recorded the way a run made today is:
    /// queued the moment it started.
    fn run(id: &str, task: &str, attempt: u32, status: RunStatus, started: i64, ended: Option<i64>) -> Run {
        Run {
            id: id.into(),
            task_id: task.into(),
            attempt,
            status,
            trigger: Trigger::Manual,
            agent: "shell".into(),
            adapter: "shell".into(),
            worktree_path: None,
            worktree_branch: None,
            runtime: "herdr".into(),
            session: None,
            token: None,
            result: None,
            error: None,
            started_at: ago(started),
            ended_at: ended.map(ago),
            queued_at: Some(ago(started)),
            scheduled_for: None,
            fail_kind: None,
            blocked_since: None,
            blocked_source: None,
            block_suspected_since: None,
            turn_ended_at: None,
            turn_end_reason: None,
            required_steps: Vec::new(),
            usage: None,
        }
    }

    /// A run from before the operations facts existed.
    fn old(mut r: Run) -> Run {
        r.queued_at = None;
        r.scheduled_for = None;
        r.fail_kind = None;
        r
    }

    /// `n` done runs of `task`, each `mins` minutes long, finished well
    /// before any window edge the tests look at.
    fn history(task: &str, n: u32, mins: &[i64]) -> Vec<Run> {
        (0..n)
            .map(|i| {
                let len = mins[i as usize % mins.len()];
                let end = 600 + i as i64 * 10;
                run(&format!("{task}-h{i}"), task, i + 1, RunStatus::Done, end + len, Some(end))
            })
            .collect()
    }

    fn input<'a>(tasks: &'a [Task], runs: &'a [Run]) -> OperationsInput<'a> {
        OperationsInput { now: now(), tasks, runs, ..Default::default() }
    }

    // -- run facts --------------------------------------------------------

    #[test]
    fn the_production_words_hold_their_edge_cases() {
        // production.rs's own: scrapped first time round is neither
        // reworked nor first pass; a manual re-run after it is reworked.
        let scrapped = run("a", "t", 1, RunStatus::Failed, 20, Some(15));
        let rerun = run("b", "t", 2, RunStatus::Done, 10, Some(5));
        let runs = vec![scrapped.clone(), rerun.clone()];
        let prev = predecessors(&runs);
        assert!(is_scrapped(&scrapped) && !is_reworked(&scrapped, &prev) && !is_first_pass(&scrapped, &prev));
        assert!(is_reworked(&rerun, &prev) && !is_first_pass(&rerun, &prev) && !is_scrapped(&rerun));
        let cancelled = run("c", "t", 1, RunStatus::Cancelled, 10, Some(5));
        assert!(is_scrapped(&cancelled) && !is_failed(&cancelled), "a cancel is scrap, not a failure");
        let open = run("d", "t", 1, RunStatus::Running, 10, None);
        assert!(!is_finished(&open) && !is_scrapped(&open));
    }

    #[test]
    fn a_scheduled_firing_is_never_rework_and_a_retry_always_is() {
        // attempt counts firings of standing work, not corrections: the
        // hundredth scheduled firing after a failure, done clean, is a
        // first pass (the fix production.rs made in #110).
        let failed = run("a", "t", 99, RunStatus::Failed, 20, Some(15));
        let mut firing = run("b", "t", 100, RunStatus::Done, 10, Some(5));
        firing.trigger = Trigger::Schedule;
        let mut retry = run("c", "u", 2, RunStatus::Done, 10, Some(5));
        retry.trigger = Trigger::Retry;
        let runs = vec![failed, firing.clone(), retry.clone()];
        let prev = predecessors(&runs);
        assert!(!is_reworked(&firing, &prev) && is_first_pass(&firing, &prev));
        assert!(is_reworked(&retry, &prev), "a retry is rework even with no predecessor in view");
        let again = run("d", "v", 7, RunStatus::Done, 10, Some(5));
        assert!(!is_reworked(&again, &predecessors(std::slice::from_ref(&again))), "a predecessor out of view did not fail");
    }

    #[test]
    fn cycle_time_is_done_only_and_queue_wait_is_unknown_for_old_runs() {
        assert_eq!(cycle_time(&run("a", "t", 1, RunStatus::Done, 30, Some(20))), Some(600.0));
        assert_eq!(cycle_time(&run("b", "t", 1, RunStatus::Failed, 30, Some(20))), None);
        let mut waited = run("c", "t", 1, RunStatus::Done, 30, Some(20));
        waited.queued_at = Some(ago(32));
        assert_eq!(queue_wait(&waited), Some(120.0));
        assert_eq!(queue_wait(&old(waited)), None, "unknown, never zero");
    }

    // -- percentiles and pace ----------------------------------------------

    #[test]
    fn percentiles_are_nearest_rank_and_monotone() {
        let v: Vec<f64> = (1..=10).map(f64::from).collect();
        // round(p * 9): p50 -> index 5 (4.5 rounds up), p95 -> index 9.
        assert_eq!(percentile(&v, 0.50), Some(6.0));
        assert_eq!(percentile(&v, 0.95), Some(10.0));
        assert_eq!(percentile(&v, 0.0), Some(1.0));
        assert_eq!(percentile(&[], 0.5), None);
        let reversed: Vec<f64> = v.iter().rev().copied().collect();
        assert_eq!(percentile(&reversed, 0.50), Some(6.0), "order of the input does not matter");
        let lines = age_percentiles(&v).unwrap();
        assert!(lines.p50 <= lines.p70 && lines.p70 <= lines.p85 && lines.p85 <= lines.p95);
        assert_eq!(lines.samples, 10);
    }

    #[test]
    fn fewer_than_five_samples_is_not_enough_history() {
        assert_eq!(age_percentiles(&[1.0, 2.0, 3.0, 4.0]), None);
        assert!(age_percentiles(&[1.0, 2.0, 3.0, 4.0, 5.0]).is_some());
    }

    #[test]
    fn pace_reads_each_band_and_a_value_on_a_line_is_still_below_it() {
        let lines = AgePercentiles { p50: 10.0, p70: 20.0, p85: 30.0, p95: 40.0, samples: 9 };
        assert_eq!(pace(5.0, &lines), Pace::BelowP50);
        assert_eq!(pace(10.0, &lines), Pace::BelowP50);
        assert_eq!(pace(15.0, &lines), Pace::P50ToP70);
        assert_eq!(pace(25.0, &lines), Pace::P70ToP85);
        assert_eq!(pace(35.0, &lines), Pace::P85ToP95);
        assert_eq!(pace(41.0, &lines), Pace::AboveP95);
    }

    // -- aging --------------------------------------------------------------

    #[test]
    fn a_run_is_paced_against_its_own_task_when_it_has_five_finished_runs() {
        let tasks = vec![task("t", "demo")];
        let mut runs = history("t", 5, &[10]);
        runs.push(run("live", "t", 6, RunStatus::Running, 60, None));
        let r = report(&input(&tasks, &runs));
        let item = r.aging.items.iter().find(|i| i.run_id.as_deref() == Some("live")).unwrap();
        assert_eq!(item.basis, PaceBasis::Task);
        assert_eq!(item.pace, Some(Pace::AboveP95));
        let aging = r.attention.iter().find(|e| e.kind == ExceptionKind::Aging).unwrap();
        assert_eq!(aging.severity, Severity::High);
        assert_eq!(aging.run_id.as_deref(), Some("live"));
        assert!(aging.reason.contains("p95"), "{}", aging.reason);
    }

    #[test]
    fn a_task_with_little_history_falls_back_to_its_scope() {
        let tasks = vec![task("new", "demo"), task("a", "demo"), task("b", "demo")];
        let mut runs = history("a", 3, &[10]);
        runs.extend(history("b", 3, &[10]));
        // 12 minutes against a scope whose runs all took 10: past p95.
        runs.push(run("live", "new", 1, RunStatus::Running, 12, None));
        let r = report(&input(&tasks, &runs));
        let item = r.aging.items.iter().find(|i| i.run_id.as_deref() == Some("live")).unwrap();
        assert_eq!(item.basis, PaceBasis::Scope);
        assert!(r.attention.iter().any(|e| e.kind == ExceptionKind::Aging));
    }

    #[test]
    fn with_too_little_history_anywhere_there_is_no_pace_and_no_aging() {
        let tasks = vec![task("t", "demo")];
        let mut runs = history("t", 4, &[1]);
        runs.push(run("live", "t", 5, RunStatus::Running, 10_000, None));
        let r = report(&input(&tasks, &runs));
        let item = r.aging.items.iter().find(|i| i.run_id.as_deref() == Some("live")).unwrap();
        assert_eq!(item.basis, PaceBasis::NotEnoughHistory);
        assert_eq!(item.pace, None);
        assert!(r.attention.is_empty(), "no pace, no aging: {:?}", r.attention);
        assert_eq!(r.aging.percentiles[0].lines, None);
    }

    #[test]
    fn past_p85_but_not_p95_is_a_medium_aging_warning() {
        let tasks = vec![task("t", "demo")];
        // Cycle times 1..=20 minutes: p85 is 17m (index round(.85*19)=16),
        // p95 is 19m (index 18).
        let lens: Vec<i64> = (1..=20).collect();
        let mut runs = history("t", 20, &lens);
        runs.push(run("live", "t", 21, RunStatus::Running, 18, None));
        let r = report(&input(&tasks, &runs));
        let aging = r.attention.iter().find(|e| e.kind == ExceptionKind::Aging).unwrap();
        assert_eq!(aging.severity, Severity::Medium);
    }

    // -- exceptions -----------------------------------------------------------

    #[test]
    fn a_blocked_run_carries_its_agents_reason_and_can_be_answered() {
        let tasks = vec![task("t", "demo")];
        let mut blocked = run("r", "t", 1, RunStatus::Blocked, 30, None);
        blocked.blocked_since = Some(ago(20));
        let runs = vec![blocked];
        let mut inp = input(&tasks, &runs);
        inp.block_reasons.insert("r".into(), "which database should I migrate?".into());
        let r = report(&inp);
        assert_eq!(r.attention.len(), 1);
        let e = &r.attention[0];
        assert_eq!(e.kind, ExceptionKind::Blocked);
        assert_eq!(e.severity, Severity::High);
        assert_eq!(e.reason, "which database should I migrate?");
        assert_eq!(e.since, ago(20), "aged from the block, not the run");
        assert_eq!(e.actions, vec![Action::Answer, Action::Cancel]);
        assert!(!e.suspicion);
    }

    #[test]
    fn a_screen_read_suspicion_is_labelled_a_suspicion() {
        let tasks = vec![task("t", "demo")];
        let mut r1 = run("r", "t", 1, RunStatus::Running, 30, None);
        r1.block_suspected_since = Some(ago(5));
        let runs = vec![r1];
        let r = report(&input(&tasks, &runs));
        let e = &r.attention[0];
        assert_eq!(e.kind, ExceptionKind::SuspectedStuck);
        assert!(e.suspicion);
        assert_eq!(e.severity, Severity::Medium);
    }

    #[test]
    fn silence_past_the_p95_is_a_suspicion_and_a_run_appears_once() {
        let tasks = vec![task("t", "demo")];
        let mut runs = history("t", 5, &[10]);
        runs.push(run("live", "t", 6, RunStatus::Running, 60, None));
        let mut inp = input(&tasks, &runs);
        inp.last_progress.insert("live".into(), ago(50));
        let r = report(&inp);
        let for_run: Vec<_> = r.attention.iter().filter(|e| e.run_id.as_deref() == Some("live")).collect();
        assert_eq!(for_run.len(), 1, "one row per run: {:?}", r.attention);
        assert_eq!(for_run[0].kind, ExceptionKind::Aging, "high aging outranks a medium suspicion");
        assert_eq!(for_run[0].also, vec![ExceptionKind::SuspectedStuck]);
    }

    #[test]
    fn a_retrying_task_is_a_count_and_an_exhausted_one_is_an_exception() {
        let mut retrying = scheduled(task("retrying", "demo"), ago(-5));
        retrying.pending_retry = Some(PendingRetry { attempts: 1, resume_at: ago(-600) });
        let exhausted = scheduled(task("exhausted", "demo"), ago(-600));
        let tasks = vec![retrying, exhausted];
        let mut failed = run("x1", "exhausted", 3, RunStatus::Failed, 30, Some(25));
        failed.error = Some("exit 1".into());
        let runs = vec![run("r1", "retrying", 1, RunStatus::Failed, 30, Some(25)), failed];
        let r = report(&input(&tasks, &runs));
        let kinds: Vec<_> = r.attention.iter().map(|e| (e.kind, e.task_id.clone().unwrap())).collect();
        assert_eq!(kinds, vec![(ExceptionKind::FailedExhausted, "exhausted".to_string())]);
        assert_eq!(r.attention[0].actions, vec![Action::RunAgain, Action::PauseSchedule]);
        assert!(r.attention[0].reason.contains("exit 1"));
        assert_eq!(r.flow[0].retrying, 1);
    }

    #[test]
    fn a_failure_followed_by_a_success_or_a_bench_attempt_is_not_in_the_dead_set() {
        let mut bench = task("bench", "demo");
        bench.bench_origin = Some(crate::bench::BenchOrigin {
            bench_run_id: "b".into(),
            case_id: "c".into(),
            agent: "pi".into(),
            attempt: 1,
        });
        let tasks = vec![task("recovered", "demo"), bench];
        let runs = vec![
            run("a", "recovered", 1, RunStatus::Failed, 30, Some(25)),
            run("b", "recovered", 2, RunStatus::Done, 20, Some(10)),
            run("c", "bench", 1, RunStatus::Failed, 30, Some(25)),
        ];
        assert!(report(&input(&tasks, &runs)).attention.is_empty());
    }

    #[test]
    fn a_slot_passed_and_undispatched_is_late_and_a_paused_one_is_not() {
        let mut paused = scheduled(task("paused", "demo"), ago(60));
        paused.schedule_paused = true;
        let tasks = vec![scheduled(task("late", "demo"), ago(30)), paused, scheduled(task("soon", "demo"), ago(-30))];
        let r = report(&input(&tasks, &[]));
        let late: Vec<_> = r.attention.iter().map(|e| (e.kind, e.task_id.clone().unwrap())).collect();
        assert_eq!(late, vec![(ExceptionKind::ScheduleLate, "late".to_string())]);
        assert_eq!(r.attention[0].actions, vec![Action::RunNow, Action::SkipNext, Action::PauseSchedule]);
        let state = |id: &str| r.schedules.iter().find(|s| s.task_id == id).unwrap().state;
        assert_eq!(state("late"), ScheduleState::Late);
        assert_eq!(state("paused"), ScheduleState::Paused);
        assert_eq!(state("soon"), ScheduleState::Due);
        let row = r.schedules.iter().find(|s| s.task_id == "late").unwrap();
        assert_eq!(row.timezone.as_deref(), Some("Europe/Berlin"), "the timezone is shown");
        // The queue holds the late one, aged from its slot, unpaced.
        assert_eq!(r.flow[0].wip.queued, 1);
        let queued = r.aging.items.iter().find(|i| i.stage == Stage::Queued).unwrap();
        assert_eq!(queued.age_s, 1800.0);
        assert_eq!(queued.basis, PaceBasis::NotPaced);
    }

    #[test]
    fn an_overdue_retry_is_named_a_retry_not_a_slot() {
        let mut t = scheduled(task("t", "demo"), ago(10));
        t.pending_retry = Some(PendingRetry { attempts: 1, resume_at: ago(-600) });
        let tasks = vec![t];
        let r = report(&input(&tasks, &[]));
        let e = r.attention.iter().find(|e| e.kind == ExceptionKind::ScheduleLate).unwrap();
        assert!(e.reason.starts_with("the retry due at"), "{}", e.reason);
    }

    #[test]
    fn missed_slots_with_one_reason_say_it_plainly() {
        let tasks = vec![scheduled(task("t", "demo"), ago(-60))];
        let mut inp = input(&tasks, &[]);
        inp.skipped = vec![SkippedSlots {
            task_id: "t".into(),
            at: ago(50),
            count: 2,
            first: ago(120),
            last: ago(60),
            reason: Some("not_running".into()),
        }];
        let r = report(&inp);
        assert!(r.attention[0].reason.ends_with("never fired: nothing was running to fire them"), "{}", r.attention[0].reason);
    }

    #[test]
    fn a_slot_within_the_grace_is_not_late_yet() {
        let tasks = vec![scheduled(task("t", "demo"), now() - Duration::seconds(30))];
        let r = report(&input(&tasks, &[]));
        assert!(r.attention.is_empty());
        assert_eq!(r.schedules[0].state, ScheduleState::Due);
    }

    #[test]
    fn skipped_slots_become_one_missed_exception_per_task() {
        let tasks = vec![scheduled(task("t", "demo"), ago(-60))];
        let mut inp = input(&tasks, &[]);
        inp.skipped = vec![
            SkippedSlots { task_id: "t".into(), at: ago(100), count: 2, first: ago(300), last: ago(240), reason: Some("not_running".into()) },
            SkippedSlots { task_id: "t".into(), at: ago(50), count: 1, first: ago(120), last: ago(120), reason: Some("still_active".into()) },
            SkippedSlots { task_id: "gone".into(), at: ago(50), count: 1, first: ago(120), last: ago(120), reason: None },
        ];
        let r = report(&inp);
        assert_eq!(r.attention.len(), 1);
        let e = &r.attention[0];
        assert_eq!(e.kind, ExceptionKind::ScheduleMissed);
        assert!(e.reason.starts_with("3 slots"), "{}", e.reason);
        // Both reasons, each with its own count -- one does not speak for all.
        assert!(e.reason.contains("1 as the previous run was still going"), "{}", e.reason);
        assert!(e.reason.contains("2 as nothing was running to fire them"), "{}", e.reason);
        assert_eq!(r.schedules[0].state, ScheduleState::Missed);
        assert_eq!(r.schedules[0].skipped, 3);
    }

    #[test]
    fn only_a_permanent_agents_lost_session_is_an_exception() {
        let agent = |name: &str, lifetime, state| {
            let mut a = AgentSession::new("demo", name, "pi", "herdr", lifetime, Role::default());
            a.state = state;
            a.last_seen_at = ago(15);
            a
        };
        let agents = vec![
            agent("keeper", Lifetime::Permanent, AgentState::Gone),
            agent("quiet", Lifetime::Permanent, AgentState::Ready),
            agent("temp", Lifetime::Temporary, AgentState::Gone),
            agent("stopped", Lifetime::Permanent, AgentState::Stopped),
        ];
        let inp = OperationsInput { now: now(), agents: &agents, ..Default::default() };
        let r = report(&inp);
        assert_eq!(r.attention.len(), 1);
        assert_eq!(r.attention[0].kind, ExceptionKind::LivenessLost);
        assert_eq!(r.attention[0].agent.as_deref(), Some("keeper"));
        assert_eq!(r.attention[0].severity, Severity::High);
    }

    #[test]
    fn a_triggered_signpost_is_a_low_observation() {
        let inp = OperationsInput {
            now: now(),
            signposts: vec![TriggeredSignpost {
                scenario: "demand-doubles".into(),
                metric: MetricId::new("throughput_week").unwrap(),
                reason: "above 40".into(),
            }],
            ..Default::default()
        };
        let r = report(&inp);
        assert_eq!(r.attention[0].kind, ExceptionKind::TriggeredSignpost);
        assert!(r.attention[0].observation);
        assert_eq!(r.attention[0].severity, Severity::Low);
    }

    #[test]
    fn the_queue_is_sorted_by_severity_then_age() {
        let mk = |kind, severity, age_s: f64, id: &str| Exception {
            kind,
            severity,
            scope: None,
            task_id: Some(id.into()),
            title: None,
            run_id: None,
            agent: None,
            since: now(),
            age_s,
            reason: String::new(),
            actions: Vec::new(),
            suspicion: false,
            observation: false,
            also: Vec::new(),
        };
        let mut q = vec![
            mk(ExceptionKind::TriggeredSignpost, Severity::Low, 9_999.0, "a"),
            mk(ExceptionKind::ScheduleLate, Severity::Medium, 10.0, "b"),
            mk(ExceptionKind::Blocked, Severity::High, 5.0, "c"),
            mk(ExceptionKind::FailedExhausted, Severity::High, 50.0, "d"),
            mk(ExceptionKind::ScheduleMissed, Severity::Medium, 100.0, "e"),
        ];
        sort_attention(&mut q);
        let order: Vec<_> = q.iter().map(|e| e.task_id.clone().unwrap()).collect();
        assert_eq!(order, vec!["d", "c", "e", "b", "a"]);
    }

    #[test]
    fn a_parent_scope_covers_work_that_lives_only_in_its_children() {
        let tasks = vec![task("child-work", "child"), task("elsewhere", "other")];
        let mut a = run("a", "child-work", 1, RunStatus::Blocked, 10, None);
        a.blocked_since = Some(ago(5));
        let mut b = run("b", "elsewhere", 1, RunStatus::Blocked, 10, None);
        b.blocked_since = Some(ago(5));
        let runs = vec![a, b];
        let parent = ScopeFilter {
            name: "parent".into(),
            subtree: ["parent".to_string(), "child".to_string()].into_iter().collect(),
        };
        let r = report(&OperationsInput { scope: Some(parent), ..input(&tasks, &runs) });
        assert_eq!(r.scope.as_deref(), Some("parent"));
        assert_eq!(r.attention.len(), 1);
        assert_eq!(r.attention[0].scope.as_deref(), Some("child"));
        assert_eq!(r.flow.iter().map(|f| f.scope.as_str()).collect::<Vec<_>>(), vec!["child"]);
        assert_eq!(r.aging.items.len(), 1);
    }

    #[test]
    fn a_scope_filter_keeps_only_that_scopes_work() {
        let tasks = vec![task("here", "demo"), task("there", "other")];
        let mut a = run("a", "here", 1, RunStatus::Blocked, 10, None);
        a.blocked_since = Some(ago(5));
        let mut b = run("b", "there", 1, RunStatus::Blocked, 10, None);
        b.blocked_since = Some(ago(5));
        let orphan = run("c", "deleted", 1, RunStatus::Running, 10, None);
        let runs = vec![a, b, orphan];
        let mut inp = input(&tasks, &runs);
        inp.scope = Some(ScopeFilter::exactly("demo"));
        let r = report(&inp);
        assert_eq!(r.attention.len(), 1);
        assert_eq!(r.attention[0].scope.as_deref(), Some("demo"));
        assert_eq!(r.flow.len(), 1);
        assert_eq!(r.flow[0].scope, "demo");
        // Unfiltered, the run whose task is gone still counts, unscoped.
        let r = report(&input(&tasks, &runs));
        assert_eq!(r.flow.iter().map(|f| f.sessions_in_use).sum::<u32>(), 3);
    }

    // -- flow ------------------------------------------------------------------

    #[test]
    fn flow_counts_wip_by_state_sessions_and_capacity() {
        let tasks = vec![task("a", "demo"), task("b", "demo"), task("c", "demo")];
        let runs = vec![
            run("1", "a", 1, RunStatus::Dispatching, 1, None),
            run("2", "b", 1, RunStatus::Running, 1, None),
            run("3", "c", 1, RunStatus::Blocked, 1, None),
        ];
        let mut inp = input(&tasks, &runs);
        inp.capacity.insert("demo".into(), 4);
        let r = report(&inp);
        let f = &r.flow[0];
        assert_eq!(f.wip, Wip { queued: 0, dispatching: 1, running: 1, blocked: 1 });
        assert_eq!(f.sessions_in_use, 3);
        assert_eq!(f.sessions_max, Some(4));
    }

    #[test]
    fn flow_waits_are_percentiles_of_recorded_waits_only() {
        let tasks = vec![task("t", "demo")];
        let mut runs = Vec::new();
        for (i, wait) in [60, 120, 180, 240, 300].into_iter().enumerate() {
            let mut r = run(&format!("r{i}"), "t", i as u32 + 1, RunStatus::Done, 100, Some(50));
            r.queued_at = Some(r.started_at - Duration::seconds(wait));
            runs.push(r);
        }
        runs.push(run("live", "t", 9, RunStatus::Running, 1, None));
        let r = report(&input(&tasks, &runs));
        let f = &r.flow[0];
        // The live run waited 0s; with it the six waits are 0,60..300.
        assert_eq!(f.wait_p50.value, Some(180.0));
        assert_eq!(f.wait_p95.value, Some(300.0));
        assert_eq!(f.wait_p95.samples, 6);
    }

    // -- health ------------------------------------------------------------------

    #[test]
    fn health_reads_the_window_and_the_one_before_it() {
        let tasks = vec![task("t", "demo")];
        let day = 24 * 60;
        let runs = vec![
            // This week: a failure, then a recovery an hour later, then a
            // cancel by a person and a first-pass success.
            {
                let mut r = run("f", "t", 1, RunStatus::Failed, 3 * day + 10, Some(3 * day));
                r.fail_kind = Some(FailKind::RunTimeout);
                r
            },
            run("ok", "t", 2, RunStatus::Done, 3 * day - 30, Some(3 * day - 60)),
            {
                let mut r = run("x", "u", 1, RunStatus::Cancelled, 100, Some(90));
                r.fail_kind = Some(FailKind::CancelledByPerson);
                r
            },
            run("fp", "v", 1, RunStatus::Done, 50, Some(40)),
            // Last week: one done.
            run("prev", "w", 1, RunStatus::Done, 10 * day, Some(10 * day - 5)),
        ];
        let r = report(&input(&tasks, &runs));
        let h = &r.health.current;
        assert_eq!(r.health.window, HealthWindow::Week);
        assert_eq!(h.finished, 4);
        assert_eq!(h.fail_rate.value, Some(0.25));
        assert_eq!(h.scrap_rate.value, Some(0.5));
        assert_eq!(h.rework_rate.value, Some(0.25));
        assert_eq!(h.first_pass_yield.value, Some(0.25));
        assert_eq!(h.recover_p50.value, Some(3600.0));
        assert_eq!(h.fail_by_kind.get(&FailKind::RunTimeout), Some(&1));
        assert_eq!(h.fail_by_kind.get(&FailKind::CancelledByPerson), Some(&1));
        assert_eq!(h.unclassified, 0);
        assert_eq!(h.interventions, 2, "the manual run after the failure, and the person's cancel");
        assert_eq!(h.interventions_per_100.value, Some(50.0));
        assert!((h.throughput_day.value.unwrap() - 4.0 / 7.0).abs() < 1e-9);
        assert_eq!(r.health.previous.finished, 1);
        assert_eq!(r.health.previous.cycle_p50.value, Some(300.0));
    }

    #[test]
    fn health_days_step_the_window_and_the_finished_runs_feed_the_scatter() {
        let tasks = vec![task("t", "demo")];
        let day = 24 * 60;
        let mut waiting = run("w", "t", 3, RunStatus::Running, 30, None);
        // Queued two days ago, started half an hour ago: it stood waiting at
        // the end of every step in between.
        waiting.queued_at = Some(ago(2 * day + 30));
        let runs = vec![
            run("a", "t", 1, RunStatus::Done, 6 * day + 90, Some(6 * day + 30)),
            {
                let mut r = run("b", "t", 2, RunStatus::Failed, 3 * day + 20, Some(3 * day + 10));
                r.fail_kind = Some(FailKind::AgentFailed);
                r
            },
            waiting,
            run("prev", "t", 0, RunStatus::Done, 9 * day, Some(9 * day - 5)),
        ];
        let r = report(&OperationsInput { detail: true, ..input(&tasks, &runs) });
        let h = &r.health.current;
        assert_eq!(h.days.len(), 7);
        assert_eq!(h.days.last().unwrap().to, now());
        assert_eq!(h.days.iter().map(|d| d.finished).sum::<u32>(), h.finished, "the steps add up to the window");
        assert_eq!(h.days[0].done, 1);
        assert_eq!(h.days[0].first_pass, 1);
        assert_eq!(h.days[3].failed, 1);
        assert_eq!(h.days[3].scrapped, 1);
        assert_eq!(h.days[5].waiting, 1, "queued two days ago and not started by that step's end");
        assert_eq!(h.days[6].waiting, 0, "the step ending now counts the queue, and nothing is queued");
        assert_eq!(h.days[6].in_progress, 1, "started before now and not ended");
        assert_eq!(h.finished_runs.len(), 2);
        assert_eq!(h.finished_runs[0].run_id, "b", "newest first");
        assert_eq!(h.finished_runs[0].cycle_s, None, "a failure has no cycle time");
        assert_eq!(h.finished_runs[1].cycle_s, Some(3600.0));
        // The ghost window has its steps but hands no dots over.
        assert_eq!(r.health.previous.days.len(), 7);
        assert_eq!(r.health.previous.days.iter().map(|d| d.done).sum::<u32>(), 1);
        assert!(r.health.previous.finished_runs.is_empty());
        // Without the charts asked for, none of it is computed or sent.
        let plain = report(&input(&tasks, &runs));
        assert!(plain.health.current.days.is_empty() && plain.health.current.finished_runs.is_empty());
        assert!(plain.health.previous.days.is_empty());
        assert_eq!(plain.health.current.finished, h.finished, "the figures do not depend on it");
    }

    #[test]
    fn the_cfd_step_ending_now_counts_the_queue_and_old_open_runs() {
        let mut due = scheduled(task("due", "demo"), ago(30));
        due.status = TaskStatus::Pending;
        let tasks = vec![due, task("long", "demo")];
        // Started long before either window, still going.
        let runs = vec![run("old", "long", 1, RunStatus::Running, 90 * 24 * 60, None)];
        let r = report(&OperationsInput { detail: true, ..input(&tasks, &runs) });
        let days = &r.health.current.days;
        assert_eq!(days.last().unwrap().waiting, 1, "the due, undispatched task");
        assert!(days.iter().all(|d| d.in_progress == 1), "a run older than the history is in progress all along");
    }

    #[test]
    fn a_manual_run_after_a_failure_is_an_intervention_and_a_first_run_is_not() {
        let window = Window::trailing(now(), 7);
        let failed = run("a", "t", 1, RunStatus::Failed, 60, Some(50));
        let again = run("b", "t", 2, RunStatus::Done, 40, Some(30));
        let fresh = run("c", "u", 1, RunStatus::Done, 40, Some(30));
        let mut retry = run("d", "v", 2, RunStatus::Done, 40, Some(30));
        retry.trigger = Trigger::Retry;
        let prior = run("e", "v", 1, RunStatus::Failed, 60, Some(50));
        let runs = vec![failed, again, fresh, retry, prior];
        let none = BTreeSet::new();
        assert_eq!(interventions(&runs, &window, &[], &none), 1, "the automatic retry is not a person");
        assert_eq!(interventions(&runs, &window, &[ago(5), ago(20_000)], &none), 2, "answers count inside the window only");
        // The same run again, asked for by an agent: not a person's doing.
        let asked: BTreeSet<_> = [("t".to_string(), ago(40))].into_iter().collect();
        assert_eq!(interventions(&runs, &window, &[], &asked), 0, "an agent's run again is not an intervention");
    }

    #[test]
    fn old_runs_without_the_new_facts_read_as_no_data_never_zero() {
        let tasks = vec![task("t", "demo")];
        let day = 24 * 60;
        // Everything is from before the facts were recorded.
        let mut failed = old(run("a", "t", 1, RunStatus::Failed, 2 * day, Some(2 * day - 5)));
        failed.error = Some("the agent never acknowledged the task within 300s".into());
        let runs = vec![failed, old(run("b", "t", 2, RunStatus::Done, day, Some(day - 5)))];
        let r = report(&input(&tasks, &runs));
        assert_eq!(r.recorded_since, None);
        let h = &r.health.current;
        assert_eq!(h.queue_wait_p50.value, None);
        assert_eq!(h.queue_wait_p50.reason.as_deref(), Some("not recorded by any run yet"));
        assert_eq!(h.unclassified, 1, "an old failure is unclassified");
        assert!(h.fail_by_kind.is_empty(), "never guessed from its prose");
        assert_eq!(h.fail_rate.value, Some(0.5), "the facts every run has are still counted");
    }

    #[test]
    fn a_window_reaching_back_before_the_record_says_from_when() {
        let tasks = vec![task("t", "demo")];
        let day = 24 * 60;
        // Recorded from two days ago; the week window starts before that.
        let runs = vec![
            old(run("a", "t", 1, RunStatus::Done, 5 * day, Some(5 * day - 5))),
            run("b", "t", 2, RunStatus::Done, 2 * day, Some(2 * day - 5)),
        ];
        let r = report(&input(&tasks, &runs));
        assert_eq!(r.recorded_since, Some(ago(2 * day)));
        let h = &r.health.current;
        assert_eq!(h.queue_wait_p50.value, Some(0.0), "the one run that records it waited 0s");
        assert!(h.queue_wait_p50.reason.as_deref().unwrap().contains("only runs from 2026-09-23"), "{:?}", h.queue_wait_p50.reason);
        let prev = &r.health.previous;
        assert_eq!(prev.queue_wait_p50.value, None);
        assert_eq!(prev.queue_wait_p50.reason.as_deref(), Some("no data before 2026-09-23"));
    }

    #[test]
    fn the_schedule_row_shows_how_late_the_last_firing_started() {
        let tasks = vec![scheduled(task("t", "demo"), ago(-60))];
        let mut r1 = run("a", "t", 1, RunStatus::Done, 100, Some(90));
        r1.scheduled_for = Some(ago(103));
        r1.trigger = Trigger::Schedule;
        let mut r2 = run("b", "t", 2, RunStatus::Done, 50, Some(40));
        r2.scheduled_for = Some(ago(50));
        r2.trigger = Trigger::Schedule;
        let runs = vec![r1, r2];
        let r = report(&input(&tasks, &runs));
        assert_eq!(r.schedules[0].last_late_s, Some(0.0), "the newest firing, not the worst");
    }

    // -- registry metrics --------------------------------------------------------

    #[test]
    fn the_registry_metrics_come_from_the_same_functions_as_the_page() {
        let window = Window::trailing(now(), 28);
        let mut runs = history("t", 5, &[10, 20, 30, 40, 50]);
        runs.push({
            let mut r = run("f", "u", 1, RunStatus::Failed, 500, Some(490));
            r.queued_at = Some(r.started_at - Duration::seconds(90));
            r
        });
        runs.push(run("ok", "u", 2, RunStatus::Done, 400, Some(430 - 60)));
        let fig = |id| registry_metric(id, &runs, &window).unwrap();
        // Done runs took 10, 20, 30, 40 and 50 minutes, and "ok" 30:
        // nearest rank over six is index round(2.5) = 3 and round(4.25) = 4.
        assert_eq!(fig("cycle_time_p50").value, Some(30.0 * 60.0));
        assert_eq!(fig("cycle_time_p85").value, Some(40.0 * 60.0));
        assert_eq!(fig("queue_wait_p95").value, Some(90.0));
        assert_eq!(fig("fail_rate").value, Some(1.0 / 7.0));
        // The history's re-runs followed successes; only "ok" re-ran a failure.
        assert_eq!(fig("rework_rate").value, Some(1.0 / 7.0));
        assert_eq!(fig("time_to_recover_p50").value, Some(120.0 * 60.0));
        assert!(registry_metric("scrap_rate", &runs, &window).is_none(), "not one of ours");
    }

    #[test]
    fn a_registry_metric_is_as_of_the_newest_run_behind_it() {
        let window = Window::trailing(now(), 28);
        let mut runs = history("t", 5, &[10, 20, 30, 40, 50]);
        runs.push(run("f", "u", 1, RunStatus::Failed, 500, Some(490)));
        runs.push(run("ok", "u", 2, RunStatus::Done, 400, Some(370)));
        // Started later than anything else, still going: a queue wait, not
        // a finished run.
        runs.push(run("live", "v", 1, RunStatus::Running, 300, None));
        let as_of = |id| registry_metric_as_of(id, &runs, &window);
        for id in ["cycle_time_p50", "cycle_time_p85", "fail_rate", "rework_rate", "time_to_recover_p50"] {
            assert_eq!(as_of(id), Some(ago(370)), "{id}: the newest run it counts ended then");
        }
        assert_eq!(as_of("queue_wait_p95"), Some(ago(300)), "a wait is known once the run starts");
        assert_eq!(as_of("scrap_rate"), None, "not one of ours");
        assert_eq!(registry_metric_as_of("fail_rate", &[], &window), None, "nothing behind it");
    }

    #[test]
    fn a_registry_metric_with_nothing_to_read_is_none_with_a_reason() {
        let window = Window::trailing(now(), 28);
        let fig = registry_metric("cycle_time_p50", &[], &window).unwrap();
        assert_eq!(fig.value, None);
        assert_eq!(fig.reason.as_deref(), Some(METRIC_EMPTY_NO_DONE));
        let fig = registry_metric("time_to_recover_p50", &[run("a", "t", 1, RunStatus::Failed, 9, Some(8))], &window).unwrap();
        assert_eq!(fig.value, None, "a streak with no success yet is not a recovery");
    }

    #[test]
    fn the_report_round_trips_through_json_with_snake_case_kinds() {
        let tasks = vec![task("t", "demo")];
        let mut b = run("r", "t", 1, RunStatus::Blocked, 30, None);
        b.blocked_since = Some(ago(20));
        let mut f = run("f", "u", 1, RunStatus::Failed, 30, Some(20));
        f.fail_kind = Some(FailKind::AckTimeout);
        let runs = vec![b, f];
        let r = report(&input(&tasks, &runs));
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["attention"][0]["kind"], "blocked");
        assert_eq!(json["health"]["window"], "7d");
        assert_eq!(json["health"]["current"]["fail_by_kind"]["ack_timeout"], 1);
        let back: OperationsReport = serde_json::from_value(json).unwrap();
        assert_eq!(back, r);
    }
}
