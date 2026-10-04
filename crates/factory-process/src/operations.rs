//! Process-owned run history arithmetic, shared with outside-stack page projections.
use crate::run::{Run, RunStatus, Trigger};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MIN_HISTORY: usize = 5;

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
    runs.iter()
        .map(|r| ((r.task_id.as_str(), r.attempt), r.status))
        .collect()
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

#[doc(hidden)]
pub fn seconds(d: Duration) -> f64 {
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
    factory_kernel::percentile(values, p)
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

pub use crate::window::Window;

#[doc(hidden)]
pub fn ended_in(run: &Run, window: &Window) -> bool {
    run.ended_at.is_some_and(|end| window.contains(end))
}

/// Cycle times of runs that ended done in `window`.
pub fn cycle_times(runs: &[Run], window: &Window) -> Vec<f64> {
    runs.iter()
        .filter(|r| ended_in(r, window))
        .filter_map(cycle_time)
        .collect()
}

/// Queue waits of runs that started in `window` and record one.
pub fn queue_waits(runs: &[Run], window: &Window) -> Vec<f64> {
    runs.iter()
        .filter(|r| window.contains(r.started_at))
        .filter_map(queue_wait)
        .collect()
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
    recoveries(runs, window)
        .into_iter()
        .map(|(_, secs)| secs)
        .collect()
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
    pub fn of(value: Option<f64>, samples: usize, empty: &str) -> Self {
        Self {
            value,
            samples: samples as u32,
            reason: value.is_none().then(|| empty.to_string()),
        }
    }

    pub fn percentile_of(values: &[f64], p: f64, empty: &str) -> Self {
        Self::of(percentile(values, p), values.len(), empty)
    }

    /// A figure read off a field only newer runs record (`queued_at`):
    /// when the window reaches back before `since`, say so -- as the whole
    /// answer if nothing in the window records it, as a caveat otherwise.
    pub fn recorded(mut self, window: &Window, since: Option<DateTime<Utc>>) -> Self {
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
/// tabs read exactly the numbers the Line tab shows. `None` for an id
/// that is not one of them.
pub fn registry_metric(id: &str, runs: &[Run], window: &Window) -> Option<Figure> {
    let since = recorded_since(runs);
    Some(match id {
        "cycle_time_p50" => {
            Figure::percentile_of(&cycle_times(runs, window), 0.50, METRIC_EMPTY_NO_DONE)
        }
        "cycle_time_p85" => {
            Figure::percentile_of(&cycle_times(runs, window), 0.85, METRIC_EMPTY_NO_DONE)
        }
        "queue_wait_p95" => {
            Figure::percentile_of(&queue_waits(runs, window), 0.95, METRIC_EMPTY_NO_WAIT)
                .recorded(window, since)
        }
        "fail_rate" => Figure::of(
            fail_rate(runs, window),
            finished_in(runs, window),
            METRIC_EMPTY_NO_FINISHED,
        ),
        "rework_rate" => Figure::of(
            rework_rate(runs, window),
            finished_in(runs, window),
            METRIC_EMPTY_NO_FINISHED,
        ),
        "time_to_recover_p50" => Figure::percentile_of(
            &recovery_times(runs, window),
            0.50,
            METRIC_EMPTY_NO_RECOVERY,
        ),
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
        "fail_rate" | "rework_rate" => runs
            .iter()
            .filter(|r| ended_in(r, window))
            .filter_map(|r| r.ended_at)
            .max(),
        "time_to_recover_p50" => recoveries(runs, window)
            .into_iter()
            .map(|(end, _)| end)
            .max(),
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
    runs.iter()
        .filter(|r| r.queued_at.is_some())
        .map(|r| r.started_at)
        .min()
}
