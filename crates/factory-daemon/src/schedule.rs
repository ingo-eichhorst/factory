//! When a scheduled task fires next.

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
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
        Schedule::Cron(schedule) => {
            let expr = &schedule.expr;
            let cron = Cron::from_str(expr).map_err(|e| {
                FactoryError::BadRequest(format!("{expr:?} is not a cron expression: {e}"))
            })?;
            let never = |e| FactoryError::BadRequest(format!("{expr:?} never fires again: {e}"));
            match schedule.timezone.as_deref() {
                None => cron.find_next_occurrence(&after, false).map_err(never),
                // The fields are that wall clock's, so the search runs on
                // it. croner handles the two days a year the clock jumps:
                // a firing that lands in a spring-forward gap runs at the
                // first minute after it, and one in an autumn overlap runs
                // once, not twice.
                Some(name) => {
                    let tz = timezone(name)?;
                    cron.find_next_occurrence(&after.with_timezone(&tz), false)
                        .map(|next| next.with_timezone(&Utc))
                        .map_err(never)
                }
            }
        }
    }
}

/// The slots a schedule passed over: every one strictly after `fired` up
/// to and including `now`, which firing `fired` late and then computing the
/// next firing from `now` means will never run. `None` when there are none
/// -- the ordinary case, a slot fired on time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skipped {
    pub count: u32,
    pub first: DateTime<Utc>,
    pub last: DateTime<Utc>,
    /// The walk stopped at `SKIP_WALK_CAP` slots; `count` and `last` are a
    /// floor, not the whole of it. An every-minute schedule over a week of
    /// downtime is ten thousand slots, and naming the first and the fact of
    /// a cap says everything a person needs.
    pub capped: bool,
}

const SKIP_WALK_CAP: u32 = 10_000;

/// Walked with `next_after` itself, so a cron schedule's skipped slots are
/// its own grid in its own timezone and an `Every` schedule's are `fired`
/// plus whole intervals -- the same slots the schedule would have fired.
/// A schedule that no longer parses skips nothing here; `next_after`
/// refuses it on its own a moment later.
pub fn skipped_between(schedule: &Schedule, fired: DateTime<Utc>, now: DateTime<Utc>) -> Option<Skipped> {
    let mut found: Option<Skipped> = None;
    let mut cursor = fired;
    loop {
        let Ok(next) = next_after(schedule, cursor) else { break };
        if next > now || next <= cursor {
            break;
        }
        match &mut found {
            None => found = Some(Skipped { count: 1, first: next, last: next, capped: false }),
            Some(s) => {
                s.count += 1;
                s.last = next;
                if s.count >= SKIP_WALK_CAP {
                    s.capped = true;
                    break;
                }
            }
        }
        cursor = next;
    }
    found
}

/// An IANA timezone by name. Every path that sets a schedule computes its
/// next firing through `next_after`, so a name this refuses is refused when
/// the schedule is set -- with the name in the message -- rather than
/// accepted and then never firing.
fn timezone(name: &str) -> Result<Tz> {
    Tz::from_str(name.trim()).map_err(|_| {
        FactoryError::BadRequest(format!(
            "{name:?} is not a timezone Factory knows; use an IANA name such as Europe/Berlin or UTC"
        ))
    })
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

    fn t(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn a_slot_fired_on_time_skips_nothing() {
        let hourly = Schedule::Cron("0 * * * *".into());
        assert_eq!(skipped_between(&hourly, t("2026-09-11T10:00:00Z"), t("2026-09-11T10:00:05Z")), None);
    }

    #[test]
    fn downtime_through_several_slots_names_count_first_and_last() {
        // Fired the 10:00 slot at 13:20: 11:00, 12:00 and 13:00 never run.
        let hourly = Schedule::Cron("0 * * * *".into());
        let skipped = skipped_between(&hourly, t("2026-09-11T10:00:00Z"), t("2026-09-11T13:20:00Z")).unwrap();
        assert_eq!(skipped.count, 3);
        assert_eq!(skipped.first, t("2026-09-11T11:00:00Z"));
        assert_eq!(skipped.last, t("2026-09-11T13:00:00Z"));
        assert!(!skipped.capped);
    }

    #[test]
    fn an_every_schedule_skips_whole_intervals_from_the_fired_slot() {
        let every = Schedule::Every { seconds: 600 };
        let skipped = skipped_between(&every, t("2026-09-11T10:00:00Z"), t("2026-09-11T10:25:00Z")).unwrap();
        assert_eq!(skipped.count, 2);
        assert_eq!(skipped.first, t("2026-09-11T10:10:00Z"));
        assert_eq!(skipped.last, t("2026-09-11T10:20:00Z"));
    }

    #[test]
    fn a_long_walk_stops_at_the_cap_and_says_so() {
        let every = Schedule::Every { seconds: 1 };
        let skipped = skipped_between(&every, t("2026-09-11T00:00:00Z"), t("2026-09-12T00:00:00Z")).unwrap();
        assert_eq!(skipped.count, SKIP_WALK_CAP);
        assert!(skipped.capped);
    }

    #[test]
    fn cron_fires_on_the_hour() {
        let now = "2026-09-11T10:17:00Z".parse::<DateTime<Utc>>().unwrap();
        let next = next_after(&Schedule::Cron("0 * * * *".into()), now).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-09-11T11:00:00+00:00");
    }

    fn berlin(expr: &str) -> Schedule {
        Schedule::Cron(factory_core::task::CronSchedule {
            expr: expr.into(),
            timezone: Some("Europe/Berlin".into()),
        })
    }

    fn at(rfc3339: &str) -> DateTime<Utc> {
        rfc3339.parse().unwrap()
    }

    #[test]
    fn a_zoned_schedule_keeps_its_wall_clock_across_the_october_change() {
        // The weekly audit, as it should read: nine o'clock Berlin on
        // Mondays. Summer time (UTC+2) before 25 October 2026, winter
        // (UTC+1) after -- the UTC firing moves, the Berlin one does not.
        let audit = berlin("0 9 * * 1");
        assert_eq!(next_after(&audit, at("2026-09-22T12:00:00Z")).unwrap(), at("2026-09-28T07:00:00Z"));
        assert_eq!(next_after(&audit, at("2026-10-20T12:00:00Z")).unwrap(), at("2026-10-26T08:00:00Z"));
    }

    #[test]
    fn a_utc_schedule_still_fires_on_utc_whatever_berlin_does() {
        // No timezone is UTC -- every schedule stored before this existed.
        let old = Schedule::Cron("0 7 * * 1".into());
        assert_eq!(next_after(&old, at("2026-10-20T12:00:00Z")).unwrap(), at("2026-10-26T07:00:00Z"));
    }

    #[test]
    fn a_firing_in_the_spring_forward_gap_runs_just_after_it_once() {
        // 29 March 2026: Berlin's 02:00 jumps to 03:00. 02:30 does not exist.
        let next = next_after(&berlin("30 2 * * *"), at("2026-03-28T12:00:00Z")).unwrap();
        assert_eq!(next, at("2026-03-29T01:00:00Z"), "03:00 CEST, the first minute after the gap");
        let after = next_after(&berlin("30 2 * * *"), next).unwrap();
        assert_eq!(after, at("2026-03-30T00:30:00Z"), "and back to 02:30 the next day");
    }

    #[test]
    fn a_firing_in_the_autumn_overlap_runs_once_not_twice() {
        // 25 October 2026: Berlin's 03:00 falls back to 02:00, so 02:30
        // happens twice. A daily job must still run once that day.
        let first = next_after(&berlin("30 2 * * *"), at("2026-10-24T12:00:00Z")).unwrap();
        let second = next_after(&berlin("30 2 * * *"), first).unwrap();
        assert_eq!(first.with_timezone(&Tz::Europe__Berlin).date_naive().to_string(), "2026-10-25");
        assert_eq!(
            second.with_timezone(&Tz::Europe__Berlin).date_naive().to_string(),
            "2026-10-26",
            "the second 02:30 on the 25th is not a second run: {first} then {second}"
        );
    }

    #[test]
    fn an_unknown_timezone_is_refused_with_its_name() {
        let typo = Schedule::Cron(factory_core::task::CronSchedule {
            expr: "0 9 * * 1".into(),
            timezone: Some("Europe/Berlinn".into()),
        });
        let err = next_after(&typo, Utc::now()).unwrap_err().to_string();
        assert!(err.contains("Europe/Berlinn") && err.contains("IANA"), "{err}");
    }

    #[test]
    fn an_every_schedule_has_no_wall_clock_to_keep() {
        let now = at("2026-10-25T00:30:00Z");
        assert_eq!(next_after(&Schedule::Every { seconds: 3600 }, now).unwrap(), at("2026-10-25T01:30:00Z"));
    }

    #[test]
    fn nonsense_is_refused_with_the_expression_in_the_message() {
        let err = next_after(&Schedule::Cron("not a cron".into()), Utc::now()).unwrap_err();
        assert!(err.to_string().contains("not a cron"), "{err}");
    }
}
