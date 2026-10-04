//! Pure shared calendar-grid arithmetic; task admission/misfire decisions stay with owners.

use crate::error::{FactoryError, Result};
use crate::Schedule;
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use croner::Cron;
use std::str::FromStr;

pub fn next_after(schedule: &Schedule, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    Parsed::new(schedule)?.next_after(after)
}

/// A schedule with its cron expression and timezone already parsed, so a
/// walk over many calendar slots parses once rather than once
/// per step.
#[doc(hidden)]
pub enum Parsed<'a> {
    Every(i64),
    // Boxed: a parsed cron is a few hundred bytes, an interval eight.
    Cron {
        expr: &'a str,
        cron: Box<Cron>,
        tz: Option<Tz>,
    },
}

impl<'a> Parsed<'a> {
    pub fn new(schedule: &'a Schedule) -> Result<Self> {
        match schedule {
            Schedule::Every { seconds } => Ok(Self::Every((*seconds).max(1) as i64)),
            Schedule::Cron(schedule) => {
                let expr = schedule.expr.as_str();
                let cron = Cron::from_str(expr).map_err(|e| {
                    FactoryError::BadRequest(format!("{expr:?} is not a cron expression: {e}"))
                })?;
                let tz = schedule.timezone.as_deref().map(timezone).transpose()?;
                Ok(Self::Cron {
                    expr,
                    cron: Box::new(cron),
                    tz,
                })
            }
        }
    }

    pub fn next_after(&self, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
        match self {
            Self::Every(seconds) => Ok(after + Duration::seconds(*seconds)),
            Self::Cron { expr, cron, tz } => {
                let never =
                    |e| FactoryError::BadRequest(format!("{expr:?} never fires again: {e}"));
                match tz {
                    None => cron.find_next_occurrence(&after, false).map_err(never),
                    // The fields are that wall clock's, so the search runs on
                    // it. croner handles the two days a year the clock jumps:
                    // a firing that lands in a spring-forward gap runs at the
                    // first minute after it, and one in an autumn overlap runs
                    // once, not twice.
                    Some(tz) => cron
                        .find_next_occurrence(&after.with_timezone(tz), false)
                        .map(|next| next.with_timezone(&Utc))
                        .map_err(never),
                }
            }
        }
    }
}

/// An IANA timezone by name. Every path that sets a schedule computes its
/// next firing through `next_after`, so a name this refuses is refused when
/// the schedule is set -- with the name in the message -- rather than
/// accepted and then never firing.
pub fn timezone(name: &str) -> Result<Tz> {
    Tz::from_str(name.trim()).map_err(|_| {
        FactoryError::BadRequest(format!(
            "{name:?} is not a timezone Factory knows; use an IANA name such as Europe/Berlin or UTC"
        ))
    })
}
