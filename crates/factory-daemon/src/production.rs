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
//! * **reworked** -- a finished run with `attempt > 1`. The only rework
//!   signal the domain has: it says a task was tried again, not that
//!   anyone rejected anything.
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
use factory_core::run::{Run, RunStatus};
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

        let earliest_run = finished.iter().map(|(_, end)| *end).min();

        Ok(Production {
            bin,
            from,
            to: now,
            buckets: bucket(&finished, bin, from, now),
            daily: bucket(&finished, ProductionBin::Day, year_from, now),
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

/// Counts finished runs into calendar-aligned buckets from `from` to `now`.
/// Every internal boundary is shared between two buckets, so only the very
/// last one -- the one that ends at `now` rather than a full step later --
/// counts a run landing exactly on its own end; every other bucket is
/// half-open, or a run on a shared boundary would be counted twice.
fn bucket(finished: &[(&Run, DateTime<Utc>)], bin: ProductionBin, from: DateTime<Utc>, now: DateTime<Utc>) -> Vec<ProductionBucket> {
    let bounds = bucket_bounds(bin, from, now);
    let width = step(bin);
    let last = bounds.len().saturating_sub(1);
    bounds
        .into_iter()
        .enumerate()
        .map(|(i, (b_from, b_to))| {
            let inclusive_end = i == last;
            let (mut count, mut scrapped, mut reworked) = (0u32, 0u32, 0u32);
            for (run, end) in finished {
                let in_bucket = *end >= b_from && if inclusive_end { *end <= b_to } else { *end < b_to };
                if !in_bucket {
                    continue;
                }
                count += 1;
                if matches!(run.status, RunStatus::Failed | RunStatus::Cancelled) {
                    scrapped += 1;
                }
                if run.attempt > 1 {
                    reworked += 1;
                }
            }
            ProductionBucket {
                from: b_from,
                to: b_to,
                finished: count,
                scrapped,
                reworked,
                partial: b_to - b_from < width,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::run::Trigger;

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
        started: DateTime<Utc>,
        ended: Option<DateTime<Utc>>,
    ) -> Run {
        Run {
            id: id.into(),
            task_id: task_id.into(),
            attempt,
            status,
            trigger: Trigger::Manual,
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
            worktree_path: None,
            worktree_branch: None,
        }
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
            midnight - Duration::minutes(10),
            Some(midnight + Duration::minutes(10)),
        );
        let finished: Vec<(&Run, DateTime<Utc>)> = vec![(&r, r.ended_at.unwrap())];

        let now = from + Duration::days(3);
        let buckets = bucket(&finished, ProductionBin::Day, from, now);

        assert_eq!(buckets[0].finished, 0, "the day it started must not count it");
        assert_eq!(buckets[1].finished, 1, "the day it ended must");
    }

    #[test]
    fn scrapped_and_reworked_are_read_against_finished_not_tallied_apart() {
        let from = floor_to(ProductionBin::Day, at(0));
        let now = from + Duration::hours(1);
        let r1 = run("r1", "t1", 1, RunStatus::Done, from, Some(from + Duration::seconds(100)));
        let r2 = run("r2", "t1", 2, RunStatus::Failed, from, Some(from + Duration::seconds(200))); // reworked AND scrapped
        let r3 = run("r3", "t1", 1, RunStatus::Cancelled, from, Some(from + Duration::seconds(300)));
        let finished: Vec<(&Run, DateTime<Utc>)> = vec![
            (&r1, r1.ended_at.unwrap()),
            (&r2, r2.ended_at.unwrap()),
            (&r3, r3.ended_at.unwrap()),
        ];
        let buckets = bucket(&finished, ProductionBin::Day, from, now);
        assert_eq!(buckets.len(), 1);
        let b = &buckets[0];
        assert_eq!(b.finished, 3);
        assert_eq!(b.scrapped, 2, "failed and cancelled are both scrap");
        assert_eq!(b.reworked, 1, "only the attempt > 1 run is rework");
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
        let r = run("r1", "t1", 1, RunStatus::Done, from, Some(boundary));
        let finished: Vec<(&Run, DateTime<Utc>)> = vec![(&r, r.ended_at.unwrap())];
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
        let r1 = run("r1", "t1", 1, RunStatus::Done, at(0), Some(at(100)));
        let r2 = run("r2", "t2", 1, RunStatus::Done, at(0), Some(at(200)));
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
        let r = run("r1", "orphan", 1, RunStatus::Done, at(0), Some(at(100)));
        let runs = vec![r];
        assert_eq!(finished_in_scope(&runs, &scope_of, Some("alpha")).len(), 0);
        assert_eq!(finished_in_scope(&runs, &scope_of, None).len(), 1, "unscoped still finds it");
    }

    // ---------------------------------------------------------------- empty

    #[test]
    fn an_empty_store_produces_full_buckets_of_zero_not_an_error() {
        let buckets = bucket(&[], ProductionBin::Day, at(0), at(5 * 24 * 3600));
        assert!(!buckets.is_empty(), "the window still has buckets to draw, just empty ones");
        assert!(buckets.iter().all(|b| b.finished == 0 && b.scrapped == 0 && b.reworked == 0));
    }

    #[test]
    fn no_finished_runs_leaves_no_earliest_run() {
        let finished: Vec<(&Run, DateTime<Utc>)> = vec![];
        let earliest = finished.iter().map(|(_, end)| *end).min();
        assert_eq!(earliest, None, "a fresh instance has no lower bound to draw a cutoff from");
    }
}
