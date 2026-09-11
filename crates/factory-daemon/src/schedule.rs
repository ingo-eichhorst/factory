//! When a scheduled task fires next.

use chrono::{DateTime, Duration, Utc};
use croner::Cron;
use factory_core::error::{FactoryError, Result};
use factory_core::task::Schedule;
use std::str::FromStr;

pub fn next_after(schedule: &Schedule, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    match schedule {
        Schedule::Every { seconds } => {
            let seconds = (*seconds).max(1) as i64;
            Ok(after + Duration::seconds(seconds))
        }
        Schedule::Cron(expr) => {
            let cron = Cron::from_str(expr).map_err(|e| {
                FactoryError::BadRequest(format!("{expr:?} is not a cron expression: {e}"))
            })?;
            cron.find_next_occurrence(&after, false).map_err(|e| {
                FactoryError::BadRequest(format!("{expr:?} never fires again: {e}"))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_adds_its_interval() {
        let now = Utc::now();
        let next = next_after(&Schedule::Every { seconds: 90 }, now).unwrap();
        assert_eq!((next - now).num_seconds(), 90);
    }

    #[test]
    fn a_zero_interval_still_moves_forward() {
        let now = Utc::now();
        let next = next_after(&Schedule::Every { seconds: 0 }, now).unwrap();
        assert!(next > now, "a zero interval would be a busy loop");
    }

    #[test]
    fn cron_fires_on_the_hour() {
        let now = "2026-09-11T10:17:00Z".parse::<DateTime<Utc>>().unwrap();
        let next = next_after(&Schedule::Cron("0 * * * *".into()), now).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-09-11T11:00:00+00:00");
    }

    #[test]
    fn nonsense_is_refused_with_the_expression_in_the_message() {
        let err = next_after(&Schedule::Cron("not a cron".into()), Utc::now()).unwrap_err();
        assert!(err.to_string().contains("not a cron"), "{err}");
    }
}
