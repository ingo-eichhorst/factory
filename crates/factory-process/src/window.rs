use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

fn seconds(d: Duration) -> f64 {
    d.num_milliseconds().max(0) as f64 / 1000.0
}

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
        Self {
            from: now - Duration::days(days),
            to: now,
        }
    }

    pub fn contains(&self, at: DateTime<Utc>) -> bool {
        at > self.from && at <= self.to
    }

    pub fn days(&self) -> f64 {
        seconds(self.to - self.from) / 86_400.0
    }

    pub fn previous(&self) -> Self {
        let len = self.to - self.from;
        Self {
            from: self.from - len,
            to: self.from,
        }
    }
}
