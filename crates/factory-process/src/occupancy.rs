//! What a bay was doing, and when.
//!
//! Two different kinds of fact live here and they must not be confused. A
//! **block** is a run: Factory started it, the agent reported on it, and the
//! record is authoritative. A **span** is liveness as the runtime saw it --
//! herdr watching a pane, or the agent telling herdr it is busy. A span is an
//! observation, and the chart draws it fainter for that reason.
//!
//! Liveness has no history anywhere but here. herdr answers "what is this
//! agent doing now" and keeps no past, so Factory writes the past down itself:
//! one row per observed change, from the moment recording is switched on.
//! Nothing before that exists, and the view says so rather than drawing a
//! flat line back to the beginning of time.

use chrono::{DateTime, Utc};
use factory_agents::runtime::RuntimeStatus;
use serde::{Deserialize, Serialize};

/// One observed change of one session's liveness. Append-only: a span is two
/// rows, not one row that gets an end written into it later, so a daemon that
/// dies mid-span leaves a record that is merely short instead of one that is
/// wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusChange {
    /// `<scope>/<name>` for a standing agent, `run:<run_id>` for a task run.
    pub subject: String,
    pub scope: String,
    /// The agent's name in its scope.
    pub agent: String,
    pub status: RuntimeStatus,
    /// When the daemon saw it, not when it happened: the runtime is polled, so
    /// a flip and a flip back between two ticks is invisible here.
    pub at: DateTime<Utc>,
}

/// The chart, assembled by the daemon so every interface draws the same thing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Occupancy {
    pub from: DateTime<Utc>,
    /// The right-hand edge of the chart, which is deliberately in the future:
    /// a schedule drawn at the very edge is a schedule nobody can read.
    pub to: DateTime<Utc>,
    /// Where `now` falls between the two. Everything left of it happened;
    /// everything right of it is a promise.
    pub now: DateTime<Utc>,
    /// When liveness recording began. Spans before this do not exist because
    /// nothing was writing them down, which is not the same as idle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness_since: Option<DateTime<Utc>>,
    pub scopes: Vec<OccupancyScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OccupancyScope {
    pub name: String,
    pub path: String,
    pub rows: Vec<OccupancyRow>,
}

/// One row of the chart. A row is an agent, not a bay: Factory binds a run to
/// an agent, and there is no slot in between to name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OccupancyRow {
    pub agent: String,
    pub adapter: String,
    /// `permanent`, `temporary`, or `task`.
    pub lifetime: String,
    pub role: String,
    /// The standing agent's current state, or `task` for one that only exists
    /// while a run does.
    pub state: String,
    /// Runs that touched the window. What actually happened.
    pub blocks: Vec<OccupancyBlock>,
    /// Runs this agent is scheduled to take, drawn ahead of now.
    pub planned: Vec<OccupancyPlan>,
    /// Liveness as the runtime reported it. Weaker evidence than a block.
    pub spans: Vec<OccupancySpan>,
    /// Seconds of the window covered by at least one run. The union of the
    /// blocks, not their sum: three runs side by side for a minute are one
    /// busy minute, never three.
    pub busy_seconds: i64,
    /// How many lanes the row's blocks need so no two overlap: 1 for an agent
    /// that ran one thing at a time. Taken from the blocks, never from
    /// config -- nothing caps how many sessions one agent runs at once.
    #[serde(default = "one_lane")]
    pub lanes: u32,
    /// Runs of this agent still open at `now`. More than one is an agent
    /// doing several things at once, which the chart says in so many words.
    #[serde(default)]
    pub live: u32,
    /// Seconds of the window at least one run spent blocked, waiting for a
    /// human: the union of the blocks' `segments`, clipped like
    /// `busy_seconds`. Reported beside it, never taken out of it -- whether a
    /// held slot that does no work counts as busy is the owner's call (#121).
    #[serde(default)]
    pub blocked_seconds: i64,
}

fn one_lane() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OccupancyBlock {
    pub run_id: String,
    pub task_id: String,
    pub title: String,
    /// The run's status: `pending`, `running`, `done`, `failed`, `cancelled`.
    pub status: String,
    pub trigger: String,
    pub attempt: u32,
    pub from: DateTime<Utc>,
    /// Absent while the run is still going.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<DateTime<Utc>>,
    /// The task's user-authored expected duration, carried only while this run
    /// is open. It is a projection for the chart, never a completion signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_seconds: Option<u64>,
    /// Which of the row's lanes the block is drawn in, from 0 at the top. Its
    /// estimate outline is drawn in the same one.
    #[serde(default)]
    pub lane: u32,
    /// The stretches of the run spent blocked, from its journal. The block's
    /// `status` is how the run ended; these say what it waited on on the way,
    /// which the run record forgets once it ends. Empty for a run that never
    /// blocked, and then left out of the payload altogether.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<OccupancySegment>,
}

/// Part of a run, by what it was doing then. Only `blocked` is recorded so far:
/// the rest of the run is its block, in the block's own status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OccupancySegment {
    pub status: String,
    pub from: DateTime<Utc>,
    /// Absent while the run is still blocked -- it runs to now, the same
    /// convention as the block's own `to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<DateTime<Utc>>,
}

/// A run that has not happened yet. The width is the median of what this task
/// took before -- its own history, nothing modelled. A task that has never
/// finished yields no estimate, and the view draws a marker instead of a bar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OccupancyPlan {
    pub task_id: String,
    pub title: String,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_seconds: Option<u64>,
    /// How many finished runs the historical estimate came from. Zero for a
    /// user estimate, or when there is no estimate at all.
    pub samples: u32,
    /// True when the task's explicit estimate won over its run history.
    #[serde(default)]
    pub user_estimate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OccupancySpan {
    pub status: RuntimeStatus,
    pub from: DateTime<Utc>,
    /// Absent means "still, as far as we know" -- the span runs to now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<DateTime<Utc>>,
}

/// Turn the append-only rows for one subject into spans. The rows must be in
/// ascending time order; each one ends the one before it.
pub fn spans_from(changes: &[StatusChange], window_end: DateTime<Utc>) -> Vec<OccupancySpan> {
    let mut out: Vec<OccupancySpan> = Vec::new();
    for change in changes {
        if let Some(last) = out.last_mut() {
            if last.to.is_none() {
                last.to = Some(change.at);
            }
        }
        // Gone is an end, not a state. It closes whatever was open and starts
        // nothing: there is no session left to be in a state.
        if change.status == RuntimeStatus::Gone {
            continue;
        }
        out.push(OccupancySpan {
            status: change.status,
            from: change.at,
            to: None,
        });
    }
    if let Some(last) = out.last_mut() {
        if last.to.is_none() {
            last.to = Some(window_end);
        }
    }
    out.retain(|s| s.to.map(|to| to > s.from).unwrap_or(true));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    fn change(status: RuntimeStatus, secs: i64) -> StatusChange {
        StatusChange {
            subject: "demo/assistant".into(),
            scope: "demo".into(),
            agent: "assistant".into(),
            status,
            at: at(secs),
        }
    }

    #[test]
    fn each_change_closes_the_one_before_it() {
        let spans = spans_from(
            &[
                change(RuntimeStatus::Idle, 0),
                change(RuntimeStatus::Working, 60),
                change(RuntimeStatus::Idle, 100),
            ],
            at(200),
        );
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].to, Some(at(60)));
        assert_eq!(spans[1].from, at(60));
        assert_eq!(spans[1].to, Some(at(100)));
        // The last one runs to the end of the window, not to the last poll.
        assert_eq!(spans[2].to, Some(at(200)));
    }

    #[test]
    fn a_gone_session_does_not_keep_drawing() {
        let spans = spans_from(
            &[
                change(RuntimeStatus::Working, 0),
                change(RuntimeStatus::Gone, 30),
            ],
            at(200),
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].status, RuntimeStatus::Working);
        assert_eq!(spans[0].to, Some(at(30)));
    }

    #[test]
    fn a_session_that_comes_back_starts_a_new_span() {
        let spans = spans_from(
            &[
                change(RuntimeStatus::Working, 0),
                change(RuntimeStatus::Gone, 30),
                change(RuntimeStatus::Idle, 90),
            ],
            at(200),
        );
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].to, Some(at(30)));
        // Nothing is drawn across the gap: 30..90 is not idle, it is nothing.
        assert_eq!(spans[1].from, at(90));
        assert_eq!(spans[1].to, Some(at(200)));
    }

    #[test]
    fn nothing_recorded_draws_nothing() {
        assert!(spans_from(&[], at(200)).is_empty());
    }
}
