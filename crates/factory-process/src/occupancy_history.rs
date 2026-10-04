//! Authoritative run/journal geometry. Liveness guesses never supply these totals.
use crate::{
    occupancy::{OccupancyBlock, OccupancySegment},
    run::Run,
    task::TaskEntry,
};
use chrono::{DateTime, Duration, Utc};
use factory_kernel::{FactoryError, Result};

/// How far back the chart looks when nobody says.
pub const DEFAULT_MINUTES: u32 = 12 * 60;
/// The widest dashboard/metrics preset. The chart still defaults to today,
/// but its authoritative interval unions also back the 90-day hour metrics.
pub const MAX_MINUTES: u32 = 90 * 24 * 60;
/// Nor is one narrower than this: a run is at least a pixel or two wide.
pub const MIN_MINUTES: u32 = 5;
/// The journal kinds that move a run into or out of `blocked`: `blocked`
/// itself, the daemon's `unblocked`, and every other run status an agent's
/// report is journalled under. Anything that ends the run without one of these
/// is covered by the run's `ended_at`.
pub const TRANSITION_KINDS: &[&str] = &[
    "blocked",
    "unblocked",
    "dispatching",
    "running",
    "verifying",
    "done",
    "failed",
    "cancelled",
];

pub fn block_of(
    run: &Run,
    title: Option<&str>,
    estimate_seconds: Option<u64>,
    segments: Vec<OccupancySegment>,
) -> OccupancyBlock {
    OccupancyBlock {
        run_id: run.id.clone(),
        task_id: run.task_id.clone(),
        title: title.unwrap_or("(deleted task)").to_string(),
        status: run.status.as_str().to_string(),
        trigger: run.trigger.as_str().to_string(),
        attempt: run.attempt,
        from: run.started_at,
        to: run.ended_at,
        estimate_seconds: if run.ended_at.is_none() {
            estimate_seconds
        } else {
            None
        },
        // Settled once the whole row is known, by `pack_lanes`. A run's
        // segments are part of its block, so they share its lane.
        lane: 0,
        segments,
    }
}

/// The stretches a run spent blocked, from its journal entries in journal
/// order. `blocked` opens one -- a second `blocked` while one is open is the
/// same wait, not a new one -- and whatever moves the run on closes it: the
/// daemon's `unblocked`, or any other status the agent reports. One still
/// open when the run ended closes at `ended_at`, whatever kind of entry the
/// ending wrote, and nothing reaches past it; one open on a run that has not
/// ended runs to now, and is sent without a `to`.
pub fn blocked_segments(
    entries: &[TaskEntry],
    ended_at: Option<DateTime<Utc>>,
) -> Vec<OccupancySegment> {
    let mut out: Vec<OccupancySegment> = Vec::new();
    let mut open: Option<DateTime<Utc>> = None;
    for entry in entries {
        match entry.kind.as_str() {
            "blocked" => {
                open.get_or_insert(entry.at);
            }
            kind if TRANSITION_KINDS.contains(&kind) => {
                if let Some(from) = open.take() {
                    out.push(OccupancySegment {
                        status: "blocked".into(),
                        from,
                        to: Some(entry.at),
                    });
                }
            }
            _ => {}
        }
    }
    if let Some(from) = open {
        out.push(OccupancySegment {
            status: "blocked".into(),
            from,
            to: ended_at,
        });
    }
    if let Some(end) = ended_at {
        for segment in &mut out {
            segment.to = segment.to.map(|to| to.min(end));
        }
    }
    // A block lifted in the same instant it was set held nothing up.
    out.retain(|s| s.to.map(|to| to > s.from).unwrap_or(true));
    out
}

/// Everything a row says about its blocks as a whole: how busy it was, how
/// much of that was spent blocked, how many lanes it needs, and how many runs
/// are still open. One call, so neither place that builds a row can lay its
/// blocks out and forget the rest.
pub fn lay_out(
    blocks: &mut [OccupancyBlock],
    from: DateTime<Utc>,
    now: DateTime<Utc>,
) -> (i64, i64, u32, u32) {
    let lanes = pack_lanes(blocks, now);
    let live = blocks.iter().filter(|b| b.to.is_none()).count() as u32;
    (
        busy_seconds(blocks, from, now),
        blocked_seconds(blocks, from, now),
        lanes,
        live,
    )
}

/// Give every block a lane so that no two in one lane overlap, and say how many
/// lanes that took. Greedy interval packing: in start order, each block goes
/// into the first lane whose last block has ended by the time it starts -- a
/// run that begins the moment another ends shares its lane. An open run holds
/// its lane up to `now`. The blocks are left in start order.
///
/// The count comes from the blocks alone. An agent's `max_sessions` is parsed
/// and not enforced, so config cannot say how many runs will overlap.
pub fn pack_lanes(blocks: &mut [OccupancyBlock], now: DateTime<Utc>) -> u32 {
    blocks.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.run_id.cmp(&b.run_id)));
    let mut ends: Vec<DateTime<Utc>> = Vec::new();
    for block in blocks.iter_mut() {
        let end = block.to.unwrap_or(now).max(block.from);
        let lane = match ends.iter().position(|&e| e <= block.from) {
            Some(free) => free,
            None => {
                ends.push(end);
                ends.len() - 1
            }
        };
        ends[lane] = end;
        block.lane = lane as u32;
    }
    ends.len().max(1) as u32
}

/// Seconds of the window at least one run held: the union of the blocks, so
/// runs side by side never count the same wall-clock second twice. Clipped to
/// the window at both ends, so a run that started yesterday counts only the
/// part that is on the chart.
pub fn busy_seconds(blocks: &[OccupancyBlock], from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
    union_seconds(blocks.iter().map(|b| (b.from, b.to)), from, to)
}

/// Seconds of the window at least one run was blocked, counted the way
/// `busy_seconds` counts runs: two runs waiting side by side wait one minute a
/// minute. A part of `busy_seconds`, never taken out of it.
pub fn blocked_seconds(blocks: &[OccupancyBlock], from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
    union_seconds(
        blocks
            .iter()
            .flat_map(|b| b.segments.iter())
            .map(|s| (s.from, s.to)),
        from,
        to,
    )
}

/// The union of some intervals, clipped to `from..to`, in whole seconds. An
/// interval with no end runs to `to`.
pub fn union_seconds(
    intervals: impl Iterator<Item = (DateTime<Utc>, Option<DateTime<Utc>>)>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> i64 {
    let mut spans: Vec<(DateTime<Utc>, DateTime<Utc>)> = intervals
        .map(|(start, end)| (start.max(from), end.unwrap_or(to).min(to)))
        .filter(|(start, end)| end > start)
        .collect();
    spans.sort();
    let mut total = Duration::zero();
    let mut open: Option<(DateTime<Utc>, DateTime<Utc>)> = None;
    for (start, end) in spans {
        open = match open {
            Some((s, e)) if start <= e => Some((s, e.max(end))),
            Some((s, e)) => {
                total += e - s;
                Some((start, end))
            }
            None => Some((start, end)),
        };
    }
    if let Some((s, e)) = open {
        total += e - s;
    }
    total.num_seconds()
}

/// The chart's window: default minutes with a quarter-window ahead, or an
/// explicit range clamped to MIN_MINUTES..MAX_MINUTES at its right edge.
pub fn window_bounds(
    minutes: Option<u32>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let minutes = minutes
        .unwrap_or(DEFAULT_MINUTES)
        .clamp(MIN_MINUTES, MAX_MINUTES) as i64;
    let (from, to) = match (from, to) {
        (None, None) => {
            let ahead = (minutes / 4).max(1);
            return Ok((
                now - Duration::minutes(minutes),
                now + Duration::minutes(ahead),
            ));
        }
        (Some(from), None) => (from, from + Duration::minutes(minutes)),
        (None, Some(to)) => (to - Duration::minutes(minutes), to),
        (Some(from), Some(to)) => (from, to),
    };
    if to <= from {
        return Err(FactoryError::BadRequest(format!(
            "an occupancy window ends after it begins; got from {} and to {}",
            from.to_rfc3339(),
            to.to_rfc3339()
        )));
    }
    let span = (to - from).clamp(
        Duration::minutes(MIN_MINUTES as i64),
        Duration::minutes(MAX_MINUTES as i64),
    );
    Ok((to - span, to))
}

/// Run and blocked-journal evidence grouped by canonical (scope, agent).
/// Chart-only titles, estimates, planned firings and liveness are not read.
/// Both hour metrics and chart geometry count interval unions per row, not
/// summed run durations or states inferred from a terminal.
pub async fn read_blocks(
    store: &dyn crate::store::TaskStore,
    scopes: &factory_kernel::ScopeTree,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<std::collections::BTreeMap<(String, String), Vec<OccupancyBlock>>> {
    use std::collections::{BTreeMap, BTreeSet};
    let runs = if from <= now {
        store.runs_between(from, now.min(to)).await?
    } else {
        Vec::new()
    };
    let tasks = store.list(&Default::default()).await?;
    let scope_of: BTreeMap<&str, String> = tasks
        .iter()
        .map(|t| (t.id.as_str(), scopes.canonical_scope_name(&t.scope)))
        .collect();
    let on_chart: BTreeSet<&str> = runs.iter().map(|r| r.id.as_str()).collect();
    let mut transitions: BTreeMap<String, Vec<TaskEntry>> = BTreeMap::new();
    if let Some(earliest) = runs.iter().map(|r| r.started_at).min() {
        for (_, entry) in store
            .entries_of_kinds(TRANSITION_KINDS, earliest - Duration::seconds(1))
            .await?
        {
            let Some(run_id) = entry.run_id.as_deref() else {
                continue;
            };
            if on_chart.contains(run_id) {
                transitions
                    .entry(run_id.to_string())
                    .or_default()
                    .push(entry);
            }
        }
    }
    let mut blocks: BTreeMap<(String, String), Vec<OccupancyBlock>> = BTreeMap::new();
    for run in &runs {
        let scope = scope_of
            .get(run.task_id.as_str())
            .cloned()
            .unwrap_or_default();
        blocks
            .entry((scope, run.agent.clone()))
            .or_default()
            .push(block_of(
                run,
                None,
                None,
                blocked_segments(
                    transitions
                        .get(&run.id)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    run.ended_at,
                ),
            ));
    }
    Ok(blocks)
}
