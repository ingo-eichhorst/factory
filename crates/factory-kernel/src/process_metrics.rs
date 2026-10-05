//! L4/L5-produced metric inputs. Plain evidence only; gathering and evaluation
//! remain with the producing service, never in the kernel.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Hour, day or week. Decided by the caller and sent with every request
/// rather than derived from the window on the server, so a view always gets
/// the granularity it actually draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductionBin {
    Hour,
    Day,
    Week,
}

/// Plain selection input for the live L4 production provider. The producer
/// owns window selection, subtree membership and bucket arithmetic.
pub struct ProductionQuery {
    pub scope: Option<String>,
    pub now: chrono::DateTime<Utc>,
    pub minutes: Option<u32>,
    pub bin: ProductionBin,
    /// The people production endpoint remains exact; metrics ask for a subtree.
    pub subtree: bool,
}

/// One period of finished runs. `scrapped`, `reworked` and `first_pass` are
/// all read against `finished`, not tallied separately from it -- a run that
/// fails on its second attempt is one run, counted once, in each of the
/// fields it qualifies for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductionBucket {
    /// The bucket's real start. Equal to the bin's own calendar boundary
    /// (the top of the hour, midnight, Monday) except for the very first
    /// bucket of a query, which is clipped forward to the window's start.
    pub from: chrono::DateTime<chrono::Utc>,
    /// The bucket's real end. Equal to the next calendar boundary except for
    /// the last bucket, which is clipped back to the moment the query ran --
    /// a bucket still filling in is not the same fact as a slow one.
    pub to: chrono::DateTime<chrono::Utc>,
    pub finished: u32,
    pub scrapped: u32,
    /// A finished run that re-attempts work that did not succeed. Not
    /// `attempt > 1`: a scheduled task's every firing bumps its own
    /// `attempt`, so that alone would count a healthy recurring task as
    /// almost entirely rework -- see `production.rs`'s `is_rework` for the
    /// real signal (`Run::trigger`, and the previous attempt's own outcome).
    pub reworked: u32,
    /// A finished run that ended `done` and was not itself rework -- the
    /// count `first_pass_yield` (`first_pass / finished`) is read from. Not
    /// `attempt == 1`: a scheduled task's hundredth firing, done clean, is
    /// exactly as much a first pass as its first ever firing, since
    /// `attempt` counts occurrences of standing work, not correction
    /// attempts -- see `production.rs`'s module doc comment for what
    /// `reworked` means and why `1 - reworked/finished` was the wrong
    /// formula.
    #[serde(default)]
    pub first_pass: u32,
    /// True when `to - from` falls short of the bin's nominal width. Decided
    /// once, here -- so a chart never has to guess whether a short bar is a
    /// quiet period or a bucket that has not finished collecting yet.
    pub partial: bool,
}

/// The run history the dashboard draws: a bucketed window for the throughput
/// chart and the KPI sparklines that read the same series, and the year of
/// daily totals the production grid always shows regardless of what window
/// is selected above it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductionFact {
    /// The bin `buckets` is drawn in. `daily` is always day-grain, whatever
    /// this says.
    pub bin: ProductionBin,
    pub from: chrono::DateTime<chrono::Utc>,
    pub to: chrono::DateTime<chrono::Utc>,
    pub buckets: Vec<ProductionBucket>,
    /// Fifty-three weeks of daily totals ending today, scoped the same as
    /// `buckets`. Independent of `bin`: the production-year grid does not
    /// rebin with the window above it.
    pub daily: Vec<ProductionBucket>,
    /// The earliest finished run this query found, scoped the same as
    /// everything else here. `None` when it found none at all. This is a
    /// lower bound on the instance's life, not its birthday -- the store
    /// does not record when the instance was set up -- so a day before it is
    /// drawn as "no record", never as "before this factory existed".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earliest_run: Option<chrono::DateTime<chrono::Utc>>,
}

/// One requested process-owned measurement. Missing values keep their
/// reason and evidence timestamp; they never become an invented zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessMetricFact {
    pub name: String,
    pub value: Option<f64>,
    pub as_of: DateTime<Utc>,
    pub reason: Option<String>,
}

/// Newest settled benchmark evidence, not a whole benchmark report and
/// not the page-read time. No settled run is represented by no fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchResolutionFact {
    pub dataset: String,
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub passed: u32,
    pub failed: u32,
}
