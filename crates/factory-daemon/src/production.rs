//! The dashboard's run history: throughput bucketed by hour, day or week, and
//! the year of daily totals the production grid always draws. Assembled here,
//! beside `occupancy.rs`, for the same reason -- one place decides what
//! "finished", "scrapped" and "reworked" mean, so every card on the page
//! draws the same numbers from the same rules.
//!
//! Definitions, fixed here and nowhere else:
//!
//! * **finished** -- `ended_at` is set. Bucketed by `ended_at`, never
//!   `started_at`, or a run that crosses midnight lands on the wrong day.
//! * **scrapped** -- a finished run whose status is `failed` or `cancelled`.
//! * **reworked** -- a finished run that re-attempts work that did not
//!   succeed. `attempt` alone is not that signal: it is a per-*task* run
//!   counter, not a per-attempt-at-fixing-a-failure counter, so a scheduled
//!   task's every firing bumps it, and a task fired every 30 minutes racks
//!   up hundreds of attempts that are each the next occurrence of standing
//!   work, not a correction of the one before -- counting `attempt > 1` as
//!   rework (the bug fixed here) read a healthy recurring task as almost
//!   entirely rework. The true signal is `Run::trigger`, fixed by
//!   `is_rework`:
//!     - `Trigger::Retry` is always rework -- it exists (see `Trigger`'s own
//!       doc comment) only as the daemon's automatic retry of a run that
//!       just failed.
//!     - `Trigger::Manual`/`Trigger::Workflow` is rework only when the same
//!       task's *previous* run (`attempt - 1`) ended `failed` or
//!       `cancelled` -- a person or a workflow re-running a task to fix a
//!       failure. Re-running a task whose previous run already finished
//!       `done` is not rework: that is new work on a standing task, not a
//!       correction, however many times it has been run before.
//!     - `Trigger::Schedule`, `Trigger::Bench` and `Trigger::Agent` are
//!       never rework, whatever the previous run's outcome. A scheduled
//!       firing, a bench harness's own repeated attempts, and a standing
//!       agent's own dispatch are each the next occurrence of the thing
//!       they always do, not an attempt at correcting a failure -- nothing
//!       about them says "this is fixing the one before" the way `Retry`
//!       and a deliberate manual/workflow re-run do.
//!
//!   The previous run's status is read from a `(task_id, attempt)` map built
//!   once, from the same runs this endpoint already loads (`is_rework`'s own
//!   doc comment). A previous run older than that fetch has no entry, and is
//!   read as not-failed -- no evidence of a failure to correct, so not
//!   rework -- rather than guessing from a second, per-task query.
//! * **first_pass** -- a finished run whose status is `done` and which is
//!   not reworked. Not `attempt == 1`: a scheduled task's hundredth firing,
//!   done clean, is exactly as much a first pass as its first ever firing,
//!   since `attempt` counts occurrences of standing work, not correction
//!   attempts. `1.0 - reworked/finished` reads a scrap-only run (0 reworked,
//!   all failed) as a perfect `1.0`; `first_pass/finished` reads it as
//!   `0.0`, which is the true fact.
//!
//! A run carries no scope of its own. Narrowed by joining through its task,
//! by exact name -- the way `occupancy.rs` does. The rail's tree (a scope's
//! selection also reaches everything nested under it) is a client-side
//! reading of scope paths for the live task figures; this aggregate matches
//! the scope named in the request, nothing wider. Where that could read
//! differently from a card built on `state.tasks`, the dashboard says so in
//! its own prose rather than this endpoint inventing a second containment
//! rule in a second language.
//!
//! A run whose task has since been deleted has nothing to join through, so a
//! *scoped* query correctly finds it in none of them -- but an *unscoped*
//! query ("All scopes") still counts it, the same way `occupancy.rs` still
//! draws a row for a run whose agent the config no longer declares rather
//! than dropping it. The consequence is the same shape as there: the
//! instance-wide total can be larger than every scope's total added
//! together. That is the honest answer for a run that happened, not a bug in
//! the arithmetic.

use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use factory_core::error::Result;
use factory_core::protocol::{Production, ProductionBin, ProductionBucket};
use factory_core::run::{Run, RunStatus, Trigger};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::engine::Engine;

/// How far back the throughput window looks when nobody says: the middle of
/// the three presets the dashboard offers.
const DEFAULT_MINUTES: u32 = 14 * 24 * 60;
/// A window wider than this is what the production-year grid is for, not the
/// throughput chart -- the same reasoning `occupancy::MAX_MINUTES` states.
const MAX_MINUTES: u32 = 90 * 24 * 60;
/// The production-year grid: 53 weeks, always, whatever window is selected
/// above it.
const YEAR_DAYS: i64 = 53 * 7;

impl Engine {
    pub async fn production(
        self: &Arc<Self>,
        minutes: Option<u32>,
        bin: Option<ProductionBin>,
        scope: Option<String>,
    ) -> Result<Production> {
        let now = Utc::now();
        let minutes = minutes.unwrap_or(DEFAULT_MINUTES).clamp(5, MAX_MINUTES);
        let bin = bin.unwrap_or(ProductionBin::Day);
        let from = now - Duration::minutes(minutes as i64);
        // Anchored to today's own midnight, not to `now` directly, so the
        // grid is exactly `YEAR_DAYS` calendar days -- today included -- and
        // not 371 or 373 depending on what time of day this happened to run.
        let year_from = floor_to(ProductionBin::Day, now) - Duration::days(YEAR_DAYS - 1);
        // One query covers both series: the throughput window is never wider
        // than the year, so fetching from the earlier of the two bounds
        // gives the daily grid and the requested buckets from a single pass.
        let earliest_fetch = from.min(year_from);

        let tasks = self.store.list(&Default::default()).await?;
        let factory = self.factory_snapshot();
        // Canonicalized, the same way `occupancy.rs` canonicalizes this same
        // join: a task written before a scope's identity became its path
        // still carries the bare name it was given, and a request narrowed to
        // that scope's current identity must still find it. Held in its own
        // `Vec` so `scope_of` can go on borrowing `&str` the way
        // `finished_in_scope` -- and its own tests -- expect.
        let canonical_scopes: Vec<String> = tasks
            .iter()
            .map(|t| factory.canonical_scope_name(&t.scope))
            .collect();
        let scope_of: BTreeMap<&str, &str> = tasks
            .iter()
            .zip(canonical_scopes.iter())
            .map(|(t, s)| (t.id.as_str(), s.as_str()))
            .collect();

        let runs = self.store.runs_between(earliest_fetch, now).await?;
        let finished = finished_in_scope(&runs, &scope_of, scope.as_deref());

        // Built over every run this fetch loaded, not just `finished` and
        // not narrowed to `scope` -- `is_rework` looks a finished run's own
        // predecessor up here rather than a second query per task. See the
        // module doc comment for what a predecessor older than
        // `earliest_fetch` (missing here) reads as.
        let predecessor_status = predecessor_statuses(&runs);

        // Rework is decided once per finished run, not once per bucket it
        // lands in -- `bucket()` runs twice below (the requested window and
        // the year grid) over what can be the same run.
        let classified = classify(&finished, &predecessor_status);

        let earliest_run = classified.iter().map(|(_, end, _)| *end).min();

        Ok(Production {
            bin,
            from,
            to: now,
            buckets: bucket(&classified, bin, from, now),
            daily: bucket(&classified, ProductionBin::Day, year_from, now),
            earliest_run,
        })
    }
}

/// Finished runs -- `ended_at` is set -- narrowed to the requested scope by
/// exact name, joined through the task the way `occupancy.rs` joins them.
/// `None` for `scope` is every scope, matching the rail's "All scopes".
fn finished_in_scope<'a>(
    runs: &'a [Run],
    scope_of: &BTreeMap<&str, &str>,
    scope: Option<&str>,
) -> Vec<(&'a Run, DateTime<Utc>)> {
    runs.iter()
        .filter(|r| match scope {
            None => true,
            Some(s) => scope_of.get(r.task_id.as_str()) == Some(&s),
        })
        .filter_map(|r| r.ended_at.map(|end| (r, end)))
        .collect()
}

/// `(task_id, attempt) -> status` for every run in `runs` -- what
/// `is_rework` reads a finished run's own predecessor from, built once per
/// request rather than looked up with a second, per-task query.
fn predecessor_statuses(runs: &[Run]) -> BTreeMap<(&str, u32), RunStatus> {
    runs.iter().map(|r| ((r.task_id.as_str(), r.attempt), r.status)).collect()
}

/// Pairs each of `finished` with whether it counts as rework
/// (`is_rework`, against `predecessor_status`), once -- so `bucket()`,
/// called twice below over what can be the same runs, never has to decide
/// it twice.
fn classify<'a>(
    finished: &[(&'a Run, DateTime<Utc>)],
    predecessor_status: &BTreeMap<(&str, u32), RunStatus>,
) -> Vec<(&'a Run, DateTime<Utc>, bool)> {
    finished.iter().map(|(run, end)| (*run, *end, is_rework(run, predecessor_status))).collect()
}

fn step(bin: ProductionBin) -> Duration {
    match bin {
        ProductionBin::Hour => Duration::hours(1),
        ProductionBin::Day => Duration::days(1),
        ProductionBin::Week => Duration::days(7),
    }
}

/// The bin's own calendar boundary at or before `t`: the top of the hour,
/// midnight, or the Monday that starts `t`'s week. Buckets are aligned to
/// this rather than to the window's own start, which is what makes the edge
/// bucket honestly partial instead of just an arbitrary slice.
fn floor_to(bin: ProductionBin, t: DateTime<Utc>) -> DateTime<Utc> {
    let day = t
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight always exists")
        .and_utc();
    match bin {
        ProductionBin::Hour => day + Duration::hours(t.hour() as i64),
        ProductionBin::Day => day,
        ProductionBin::Week => day - Duration::days(day.weekday().num_days_from_monday() as i64),
    }
}

/// Bucket edges from `from` to `now`, aligned to the bin's calendar unit. The
/// first bucket's real start is clipped forward to `from` and the last
/// bucket's real end is clipped back to `now` -- both legitimately short, and
/// `partial` on the resulting `ProductionBucket` says so.
fn bucket_bounds(bin: ProductionBin, from: DateTime<Utc>, now: DateTime<Utc>) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let width = step(bin);
    let mut bounds = Vec::new();
    let mut cursor = floor_to(bin, from);
    while cursor < now {
        let nominal_end = cursor + width;
        let real_from = cursor.max(from);
        let real_to = nominal_end.min(now);
        bounds.push((real_from, real_to));
        cursor = nominal_end;
    }
    bounds
}

/// Whether `run` re-attempts work that did not succeed -- the module doc
/// comment's own definition, made concrete. `predecessor_status` is the
/// `(task_id, attempt) -> status` map `Engine::production` builds once from
/// the runs it already loaded; a predecessor this fetch never saw (older
/// than its window) looks up as `None`, read the same as "did not fail":
/// no evidence of a failure to correct, so not rework.
fn is_rework(run: &Run, predecessor_status: &BTreeMap<(&str, u32), RunStatus>) -> bool {
    match run.trigger {
        // The daemon's own automatic retry of a run that just failed -- see
        // `Trigger::Retry`'s own doc comment. Always rework, whatever
        // `attempt` says.
        Trigger::Retry => true,
        // A person or a workflow re-running the task: rework only when the
        // run it is re-running (`attempt - 1`, the same task) did not
        // succeed. Re-running a task whose previous run already finished
        // `done` is new work on a standing task, not a correction.
        Trigger::Manual | Trigger::Workflow if run.attempt > 1 => {
            let previous = predecessor_status.get(&(run.task_id.as_str(), run.attempt - 1));
            matches!(previous, Some(RunStatus::Failed | RunStatus::Cancelled))
        }
        // `Schedule` (the next firing), `Bench` (an attempt count the
        // harness itself sets, not a correction) and `Agent` (a standing
        // agent's own dispatch) are never rework, whatever the previous
        // run's outcome -- see the module doc comment. `Manual`/`Workflow`
        // at `attempt == 1` has no previous run that could have failed, so
        // it falls here too.
        _ => false,
    }
}

/// Counts finished runs into calendar-aligned buckets from `from` to `now`.
/// Every internal boundary is shared between two buckets, so only the very
/// last one -- the one that ends at `now` rather than a full step later --
/// counts a run landing exactly on its own end; every other bucket is
/// half-open, or a run on a shared boundary would be counted twice.
///
/// `rework` is precomputed per run (`Engine::production`, once, before this
/// runs twice) rather than decided here, so `is_rework` never runs twice for
/// the same run.
fn bucket(finished: &[(&Run, DateTime<Utc>, bool)], bin: ProductionBin, from: DateTime<Utc>, now: DateTime<Utc>) -> Vec<ProductionBucket> {
    let bounds = bucket_bounds(bin, from, now);
    let width = step(bin);
    let last = bounds.len().saturating_sub(1);
    bounds
        .into_iter()
        .enumerate()
        .map(|(i, (b_from, b_to))| {
            let inclusive_end = i == last;
            let (mut count, mut scrapped, mut reworked, mut first_pass) = (0u32, 0u32, 0u32, 0u32);
            for (run, end, rework) in finished {
                let in_bucket = *end >= b_from && if inclusive_end { *end <= b_to } else { *end < b_to };
                if !in_bucket {
                    continue;
                }
                count += 1;
                if matches!(run.status, RunStatus::Failed | RunStatus::Cancelled) {
                    scrapped += 1;
                }
                if *rework {
                    reworked += 1;
                }
                if run.status == RunStatus::Done && !*rework {
                    first_pass += 1;
                }
            }
            ProductionBucket {
                from: b_from,
                to: b_to,
                finished: count,
                scrapped,
                reworked,
                first_pass,
                partial: b_to - b_from < width,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    /// `started`/`ended` are real `DateTime`s, deliberately -- `at()`'s base
    /// epoch is not midnight-aligned, and a day-bucketing test needs to
    /// reason about real calendar boundaries (`floor_to`), not offsets from
    /// an arbitrary instant.
    fn run(
        id: &str,
        task_id: &str,
        attempt: u32,
        status: RunStatus,
        trigger: Trigger,
        started: DateTime<Utc>,
        ended: Option<DateTime<Utc>>,
    ) -> Run {
        Run {
            id: id.into(),
            task_id: task_id.into(),
            attempt,
            status,
            trigger,
            agent: "a".into(),
            adapter: "shell".into(),
            runtime: "herdr".into(),
            session: None,
            token: None,
            result: None,
            error: None,
            started_at: started,
            ended_at: ended,
            blocked_since: None,
            blocked_source: None,
            block_suspected_since: None,
            turn_ended_at: None,
            turn_end_reason: None,
            worktree_path: None,
            worktree_branch: None,
        }
    }

    /// `classify(finished_in_scope(runs, ...), predecessor_statuses(runs))`,
    /// collapsed for tests where every run in `runs` is both counted in the
    /// bucket *and* available to build the predecessor map from -- the
    /// common case. A test that needs a predecessor present in the map but
    /// *not* itself counted (an earlier attempt that ended outside this
    /// bucket) builds `predecessor_statuses`/`classify` directly instead.
    fn classify_all(runs: &[Run]) -> Vec<(&Run, DateTime<Utc>, bool)> {
        let predecessor = predecessor_statuses(runs);
        let finished: Vec<(&Run, DateTime<Utc>)> = runs.iter().filter_map(|r| r.ended_at.map(|end| (r, end))).collect();
        classify(&finished, &predecessor)
    }

    // ------------------------------------------------------------ bucketing

    #[test]
    fn a_run_is_bucketed_by_when_it_ended_not_when_it_started() {
        // Started before midnight, ended after it: bucketing by `started_at`
        // would have put this run a day early.
        let from = floor_to(ProductionBin::Day, at(0));
        let midnight = from + Duration::days(1);
        let r = run(
            "r1",
            "t1",
            1,
            RunStatus::Done,
            Trigger::Manual,
            midnight - Duration::minutes(10),
            Some(midnight + Duration::minutes(10)),
        );
        let runs = [r];
        let finished = classify_all(&runs);

        let now = from + Duration::days(3);
        let buckets = bucket(&finished, ProductionBin::Day, from, now);

        assert_eq!(buckets[0].finished, 0, "the day it started must not count it");
        assert_eq!(buckets[1].finished, 1, "the day it ended must");
    }

    #[test]
    fn scrapped_reworked_and_first_pass_are_read_against_finished_not_tallied_apart() {
        let from = floor_to(ProductionBin::Day, at(0));
        let now = from + Duration::hours(1);
        let r1 = run("r1", "t1", 1, RunStatus::Done, Trigger::Manual, from, Some(from + Duration::seconds(100))); // first-pass
        // `Retry` is unconditionally rework -- it exists only to fix a run
        // that just failed -- so this needs no recorded predecessor.
        let r2 = run("r2", "t1", 2, RunStatus::Failed, Trigger::Retry, from, Some(from + Duration::seconds(200))); // reworked AND scrapped
        let r3 = run("r3", "t1", 1, RunStatus::Cancelled, Trigger::Manual, from, Some(from + Duration::seconds(300))); // scrapped, not rework (attempt 1, no predecessor), not first-pass (not done)
        let runs = [r1, r2, r3];
        let finished = classify_all(&runs);
        let buckets = bucket(&finished, ProductionBin::Day, from, now);
        assert_eq!(buckets.len(), 1);
        let b = &buckets[0];
        assert_eq!(b.finished, 3);
        assert_eq!(b.scrapped, 2, "failed and cancelled are both scrap");
        assert_eq!(b.reworked, 1, "only the retry-triggered run is rework");
        assert_eq!(b.first_pass, 1, "only the done, non-rework run is first-pass");
    }

    #[test]
    fn every_run_scrapped_is_zero_first_pass_not_a_perfect_one() {
        // The bug this field fixes: `1.0 - reworked/finished` reads a
        // scrap-only bucket (nothing ever reworked) as a perfect `1.0`.
        // `first_pass` must read `0` here regardless.
        let from = floor_to(ProductionBin::Day, at(0));
        let now = from + Duration::hours(1);
        let runs: Vec<Run> = (0..5)
            .map(|i| run(&format!("r{i}"), &format!("t{i}"), 1, RunStatus::Failed, Trigger::Manual, from, Some(from + Duration::seconds(i))))
            .collect();
        let finished = classify_all(&runs);
        let buckets = bucket(&finished, ProductionBin::Day, from, now);
        let b = &buckets[0];
        assert_eq!(b.finished, 5);
        assert_eq!(b.scrapped, 5);
        assert_eq!(b.reworked, 0, "each was its own task's first attempt -- no predecessor to have failed");
        assert_eq!(b.first_pass, 0, "none of them ended done, so none are first-pass");
    }

    #[test]
    fn a_manual_rerun_after_a_failed_run_is_reworked_but_not_first_pass() {
        // Attempt 1 fails, attempt 2 (a manual re-run of the same task)
        // succeeds: the successful run is rework -- its predecessor failed --
        // so it is not first-pass even though it ended `done`, and the
        // failed attempt 1 is not first-pass either (it never reached
        // `done`) -- `first_pass` is the count of runs that are both done
        // *and* not rework, not the complement of `reworked`.
        let from = floor_to(ProductionBin::Day, at(0));
        let now = from + Duration::hours(1);
        let attempt1 = run("r1", "t1", 1, RunStatus::Failed, Trigger::Manual, from, Some(from + Duration::seconds(10)));
        let attempt2 = run("r2", "t1", 2, RunStatus::Done, Trigger::Manual, from, Some(from + Duration::seconds(20)));
        let runs = [attempt1, attempt2];
        let finished = classify_all(&runs);
        let buckets = bucket(&finished, ProductionBin::Day, from, now);
        let b = &buckets[0];
        assert_eq!(b.finished, 2);
        assert_eq!(b.reworked, 1, "attempt 2 re-runs a run that failed");
        assert_eq!(b.first_pass, 0, "attempt 2 is done but rework; attempt 1 is not rework but never reached done");
    }

    #[test]
    fn a_mixed_bucket_counts_first_pass_independently_of_scrapped_and_reworked() {
        let from = floor_to(ProductionBin::Day, at(0));
        let now = from + Duration::hours(1);
        // `t2` and `t4` each carry an earlier failed attempt that ended
        // outside this bucket's own count but is still in the wider fetch
        // `predecessor_statuses` reads from -- the same separation
        // `Engine::production` draws between `runs` (the fetch) and
        // `finished_in_scope` (what a bucket actually counts).
        let counted_runs = [
            run("r1", "t1", 1, RunStatus::Done, Trigger::Manual, from, Some(from + Duration::seconds(1))), // clean: first-pass
            run("r2b", "t2", 2, RunStatus::Done, Trigger::Manual, from, Some(from + Duration::seconds(2))), // reworked, not first-pass
            run("r3", "t3", 1, RunStatus::Failed, Trigger::Manual, from, Some(from + Duration::seconds(3))), // scrapped, not first-pass
            run("r4b", "t4", 2, RunStatus::Cancelled, Trigger::Manual, from, Some(from + Duration::seconds(4))), // reworked AND scrapped
        ];
        let earlier_failed_attempts = vec![
            run("r2a", "t2", 1, RunStatus::Failed, Trigger::Manual, from, Some(from + Duration::seconds(5))),
            run("r4a", "t4", 1, RunStatus::Failed, Trigger::Manual, from, Some(from + Duration::seconds(6))),
        ];
        let all_runs: Vec<Run> = counted_runs.iter().cloned().chain(earlier_failed_attempts).collect();
        let predecessor = predecessor_statuses(&all_runs);
        let finished_subset: Vec<(&Run, DateTime<Utc>)> = counted_runs.iter().map(|r| (r, r.ended_at.unwrap())).collect();
        let finished = classify(&finished_subset, &predecessor);

        let buckets = bucket(&finished, ProductionBin::Day, from, now);
        let b = &buckets[0];
        assert_eq!(b.finished, 4);
        assert_eq!(b.scrapped, 2);
        assert_eq!(b.reworked, 2, "both second attempts re-run a run that failed");
        assert_eq!(b.first_pass, 1, "only `clean` is both done and not rework");
    }

    #[test]
    fn a_scheduled_task_with_many_done_runs_has_zero_rework_and_first_pass_equals_finished() {
        // The bug this module exists to fix: a task fired every 30 minutes
        // racks up hundreds of `attempt`s, all `Trigger::Schedule` -- each
        // the next occurrence of standing work, not a correction, however
        // high `attempt` climbs.
        let from = floor_to(ProductionBin::Day, at(0));
        let now = from + Duration::hours(1);
        let runs: Vec<Run> = (1..=10)
            .map(|attempt| {
                run(
                    &format!("r{attempt}"),
                    "t1",
                    attempt,
                    RunStatus::Done,
                    Trigger::Schedule,
                    from,
                    Some(from + Duration::seconds(attempt as i64)),
                )
            })
            .collect();
        let finished = classify_all(&runs);
        let buckets = bucket(&finished, ProductionBin::Day, from, now);
        let b = &buckets[0];
        assert_eq!(b.finished, 10);
        assert_eq!(b.reworked, 0, "a scheduled firing is never rework");
        assert_eq!(b.first_pass, 10, "every done, non-rework run is first-pass");
    }

    #[test]
    fn first_pass_excludes_a_reworked_run_even_when_it_ended_done() {
        let from = floor_to(ProductionBin::Day, at(0));
        let now = from + Duration::hours(1);
        let retried_but_done = run("r1", "t1", 5, RunStatus::Done, Trigger::Retry, from, Some(from + Duration::seconds(1)));
        let runs = [retried_but_done];
        let finished = classify_all(&runs);
        let buckets = bucket(&finished, ProductionBin::Day, from, now);
        let b = &buckets[0];
        assert_eq!(b.finished, 1);
        assert_eq!(b.reworked, 1);
        assert_eq!(b.first_pass, 0, "done and reworked -- first_pass excludes it regardless of status");
    }

    #[test]
    fn the_first_bucket_is_partial_when_the_window_does_not_start_on_a_boundary() {
        // `from` lands 90 minutes into a day.
        let from = floor_to(ProductionBin::Day, at(0)) + Duration::minutes(90);
        let now = from + Duration::days(2);
        let buckets = bucket(&[], ProductionBin::Day, from, now);
        assert!(buckets[0].partial, "clipped forward to `from`, short of a full day");
        assert_eq!(buckets[0].from, from);
    }

    #[test]
    fn the_last_bucket_is_partial_when_now_falls_mid_bucket() {
        let from = floor_to(ProductionBin::Day, at(0));
        let now = from + Duration::hours(3); // "the day is 3h old"
        let buckets = bucket(&[], ProductionBin::Day, from, now);
        assert_eq!(buckets.len(), 1);
        assert!(buckets[0].partial);
        assert_eq!(buckets[0].to, now);
    }

    #[test]
    fn a_full_bucket_is_not_marked_partial() {
        let from = floor_to(ProductionBin::Day, at(0));
        let now = from + Duration::days(1);
        let buckets = bucket(&[], ProductionBin::Day, from, now);
        assert_eq!(buckets.len(), 1);
        assert!(!buckets[0].partial);
    }

    #[test]
    fn a_run_on_a_shared_boundary_is_counted_once_not_twice() {
        let from = floor_to(ProductionBin::Day, at(0));
        let boundary = from + Duration::days(1);
        let r = run("r1", "t1", 1, RunStatus::Done, Trigger::Manual, from, Some(boundary));
        let runs = [r];
        let finished = classify_all(&runs);
        let now = from + Duration::days(2);
        let buckets = bucket(&finished, ProductionBin::Day, from, now);
        let total: u32 = buckets.iter().map(|b| b.finished).sum();
        assert_eq!(total, 1);
        assert_eq!(buckets[1].finished, 1, "a run ending exactly at midnight belongs to the day starting there");
    }

    // ------------------------------------------------------------- scope join

    #[test]
    fn only_the_named_scopes_runs_are_counted() {
        let mut scope_of: BTreeMap<&str, &str> = BTreeMap::new();
        scope_of.insert("t1", "alpha");
        scope_of.insert("t2", "beta");
        let r1 = run("r1", "t1", 1, RunStatus::Done, Trigger::Manual, at(0), Some(at(100)));
        let r2 = run("r2", "t2", 1, RunStatus::Done, Trigger::Manual, at(0), Some(at(200)));
        let runs = vec![r1, r2];

        let alpha = finished_in_scope(&runs, &scope_of, Some("alpha"));
        assert_eq!(alpha.len(), 1);
        assert_eq!(alpha[0].0.task_id, "t1");

        let all = finished_in_scope(&runs, &scope_of, None);
        assert_eq!(all.len(), 2, "no scope named is every scope");
    }

    #[test]
    fn a_run_whose_task_is_gone_matches_no_named_scope() {
        // The task-to-scope map only ever holds tasks that still exist; a run
        // whose task has since been deleted has nothing to join through, so a
        // scoped query correctly finds it in none of them.
        let scope_of: BTreeMap<&str, &str> = BTreeMap::new();
        let r = run("r1", "orphan", 1, RunStatus::Done, Trigger::Manual, at(0), Some(at(100)));
        let runs = vec![r];
        assert_eq!(finished_in_scope(&runs, &scope_of, Some("alpha")).len(), 0);
        assert_eq!(finished_in_scope(&runs, &scope_of, None).len(), 1, "unscoped still finds it");
    }

    // --------------------------------------------------------------- rework

    #[test]
    fn a_retry_triggered_run_is_always_rework() {
        let r = run("r1", "t1", 2, RunStatus::Done, Trigger::Retry, at(0), Some(at(10)));
        assert!(is_rework(&r, &BTreeMap::new()), "Retry exists only to fix a run that just failed");
    }

    #[test]
    fn a_manual_rerun_after_a_failed_previous_run_is_rework() {
        let mut predecessor = BTreeMap::new();
        predecessor.insert(("t1", 1), RunStatus::Failed);
        let r = run("r2", "t1", 2, RunStatus::Done, Trigger::Manual, at(0), Some(at(10)));
        assert!(is_rework(&r, &predecessor));
    }

    #[test]
    fn a_workflow_rerun_after_a_cancelled_previous_run_is_rework() {
        let mut predecessor = BTreeMap::new();
        predecessor.insert(("t1", 1), RunStatus::Cancelled);
        let r = run("r2", "t1", 2, RunStatus::Done, Trigger::Workflow, at(0), Some(at(10)));
        assert!(is_rework(&r, &predecessor));
    }

    #[test]
    fn a_manual_rerun_after_a_done_previous_run_is_not_rework() {
        // New work on a standing task, not a correction -- see the module
        // doc comment.
        let mut predecessor = BTreeMap::new();
        predecessor.insert(("t1", 1), RunStatus::Done);
        let r = run("r2", "t1", 2, RunStatus::Done, Trigger::Manual, at(0), Some(at(10)));
        assert!(!is_rework(&r, &predecessor));
    }

    #[test]
    fn a_scheduled_firing_after_a_failed_previous_run_is_never_rework() {
        // Unlike `Manual`/`Workflow`, a schedule's next tick is not a
        // correction -- the same reasoning `Trigger::Bench`/`Trigger::Agent`
        // get in `is_rework`.
        let mut predecessor = BTreeMap::new();
        predecessor.insert(("t1", 1), RunStatus::Failed);
        let r = run("r2", "t1", 2, RunStatus::Done, Trigger::Schedule, at(0), Some(at(10)));
        assert!(!is_rework(&r, &predecessor));
    }

    #[test]
    fn a_bench_attempt_is_never_rework() {
        let mut predecessor = BTreeMap::new();
        predecessor.insert(("t1", 1), RunStatus::Failed);
        let r = run("r2", "t1", 2, RunStatus::Failed, Trigger::Bench, at(0), Some(at(10)));
        assert!(!is_rework(&r, &predecessor));
    }

    #[test]
    fn a_predecessor_outside_the_fetched_window_reads_as_not_rework() {
        // `predecessor_status` only knows about runs the request's own fetch
        // loaded (`Engine::production`'s `earliest_fetch`, normally ~53
        // weeks). A predecessor older than that has no entry -- read as "did
        // not fail", not as a second, per-task query. See the module doc
        // comment.
        let r = run("r2", "t1", 2, RunStatus::Done, Trigger::Manual, at(0), Some(at(10)));
        assert!(!is_rework(&r, &BTreeMap::new()));
    }

    // ---------------------------------------------------------------- empty

    #[test]
    fn an_empty_store_produces_full_buckets_of_zero_not_an_error() {
        let buckets = bucket(&[], ProductionBin::Day, at(0), at(5 * 24 * 3600));
        assert!(!buckets.is_empty(), "the window still has buckets to draw, just empty ones");
        assert!(buckets.iter().all(|b| b.finished == 0 && b.scrapped == 0 && b.reworked == 0 && b.first_pass == 0));
    }

    #[test]
    fn no_finished_runs_leaves_no_earliest_run() {
        let classified: Vec<(&Run, DateTime<Utc>, bool)> = vec![];
        let earliest = classified.iter().map(|(_, end, _)| *end).min();
        assert_eq!(earliest, None, "a fresh instance has no lower bound to draw a cutoff from");
    }
}
