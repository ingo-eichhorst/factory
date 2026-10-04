//! Shared scheduling values; firing and retry decisions belong to their owners.
use serde::{Deserialize, Serialize};

/// When a task fires on its own. `Cron` is a five- or six-field expression,
/// read in a timezone of its own if it names one; `Every` is a plain
/// interval for the common "every N minutes" case, which no timezone
/// changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Schedule {
    Cron(CronSchedule),
    Every { seconds: u64 },
}

/// A cron expression and the timezone its fields are read in. Without a
/// timezone the fields are UTC, which is what every schedule meant before
/// this existed -- a schedule written then keeps firing exactly when it did.
/// With one (an IANA name, `Europe/Berlin`), `0 9 * * 1` is nine o'clock on
/// that wall clock, summer and winter alike.
///
/// On the wire, and in every row already stored, a schedule with no
/// timezone is the bare expression string it always was:
/// `{"cron": "0 7 * * 1"}`. Only one that names a timezone takes the object
/// form, `{"cron": {"expr": "0 9 * * 1", "timezone": "Europe/Berlin"}}`, so
/// nothing written before this -- a database row, a script, the UI -- has
/// to change to keep working.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "CronRepr", into = "CronRepr")]
pub struct CronSchedule {
    pub expr: String,
    /// An IANA timezone name. `None` is UTC. Checked when the schedule is
    /// set -- see `schedule::next_after` in the daemon -- not when it fires.
    pub timezone: Option<String>,
}

impl CronSchedule {
    /// How a schedule reads to a person: `0 9 * * 1`, or
    /// `0 9 * * 1 (Europe/Berlin)`.
    pub fn describe(&self) -> String {
        match &self.timezone {
            Some(tz) => format!("{} ({tz})", self.expr),
            None => self.expr.clone(),
        }
    }
}

/// A UTC schedule from a bare expression -- what `Schedule::Cron("…".into())`
/// has always meant.
impl From<&str> for CronSchedule {
    fn from(expr: &str) -> Self {
        Self {
            expr: expr.to_string(),
            timezone: None,
        }
    }
}

impl From<String> for CronSchedule {
    fn from(expr: String) -> Self {
        Self {
            expr,
            timezone: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum CronRepr {
    Bare(String),
    Zoned {
        expr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
}

impl From<CronRepr> for CronSchedule {
    fn from(repr: CronRepr) -> Self {
        match repr {
            CronRepr::Bare(expr) => Self {
                expr,
                timezone: None,
            },
            CronRepr::Zoned { expr, timezone } => Self {
                expr,
                // An empty name is no name, not a timezone called "".
                timezone: timezone.filter(|t| !t.trim().is_empty()),
            },
        }
    }
}

impl From<CronSchedule> for CronRepr {
    fn from(cron: CronSchedule) -> Self {
        match cron.timezone {
            None => CronRepr::Bare(cron.expr),
            timezone => CronRepr::Zoned {
                expr: cron.expr,
                timezone,
            },
        }
    }
}
