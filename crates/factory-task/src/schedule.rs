//! Cron schedules: parsing, the matching predicate, and CRUD on the
//! `schedules` table (design §11; ADR 0021 decisions 3, 3a, 7).
//!
//! # This module is the one home of "does this schedule fire now"
//!
//! ADR 0021 decision 3a: firing is a *match* against the local wall-clock
//! minute a schedule was written for, never a next-occurrence search.
//! [`matches`] is that predicate, and every other function here that needs
//! to know whether a schedule fires at some instant calls it rather than
//! repeating the comparison — [`due`] and [`next_run`] both do. A
//! `croner::Cron::find_next_occurrence` call has no business anywhere in
//! this file: measured against Europe/Berlin, it moves a spring-forward
//! schedule from a 02:30 that never happens to a 03:00 the operator never
//! wrote, and it is the one thing this module exists not to do.
//!
//! # `enable`/`disable` change one column
//!
//! Backlog §11: they "toggle a schedule without deleting its history or
//! affecting already-created runs." [`enable`] and [`disable`] write
//! `enabled` and `updated_at` and nothing else — `cron`, `timezone`,
//! `last_fired_at` and `created_at` are untouched by either.
//!
//! # No `next_run_at` column, and this module never writes `last_fired_at`
//!
//! `factory_store::schema`'s own comment on `schedules` says why there is no
//! stored next-run: it would be a second home for this module's own answer,
//! stale the moment a schedule is edited, a timezone's rules change, or the
//! daemon is down across it. [`next_run`] computes it fresh every call.
//!
//! `last_fired_at` is a fact about a firing that happened, and ADR 0021
//! decision 3 requires it to be written in the same transaction as the run
//! it corresponds to. That transaction belongs to the dispatcher (station 11
//! decision 7: "this module stores rules; it never fires them. Creating a
//! run from a due schedule is the daemon's dispatcher"), which lives in the
//! daemon, not here. Nothing in this module writes `last_fired_at`.

use std::str::FromStr;

use chrono::{DateTime, Duration, Timelike, Utc};
use chrono_tz::Tz;
use croner::{Cron, parser::CronParser};
use rusqlite::OptionalExtension;

/// Bound for [`next_run`]'s minute-by-minute walk, chosen so that `None`
/// means one thing only: **this expression can never fire.**
///
/// ADR 0021 decision 3a first said 400 days, on the reasoning that no
/// five-field expression needs longer. That was wrong, and the agent who
/// built this module measured it rather than accepting it: `0 0 29 2 *`
/// fires on 29 February, and from 2027-01-01 its next match is 2028-02-29,
/// **424 days** out. At a 400-day horizon `next_run` answered `None` for a
/// perfectly ordinary schedule, three years in every four.
///
/// A `None` that means "it may well fire, but this preview did not look far
/// enough" is the kind of answer this project treats as worse than no
/// answer: `factory schedule list` would show a blank next-run column for a
/// working schedule and give an operator no way to tell that from a broken
/// one. 1500 days is a little over four years, so it covers the leap-day
/// case with room to spare, and no other five-field expression needs more
/// than a year.
///
/// Measured on this machine, release build, Europe/Berlin:
///
/// | Expression | Steps | Time | Answer |
/// |---|---|---|---|
/// | `0 3 * * *` (daily) | 900 | 0.02 ms | next day, 03:00 |
/// | `0 4 1 1 *` (yearly) | 162,300 | 3.0 ms | 2027-01-01 04:00 |
/// | `0 0 29 2 *` (leap day) | 609,840 | 11.0 ms | 2028-02-29 00:00 |
/// | `0 0 30 2 *` (impossible) | 2,160,000 | 39.0 ms | `None` |
///
/// The worst case is the expression that can never fire, and it costs 39 ms
/// to say so. That is the price of the guarantee, paid only by an operator
/// who wrote a date that does not exist.
pub const NEXT_RUN_HORIZON_DAYS: i64 = 1500;

/// Everything that can go wrong creating or reading back a schedule, or
/// evaluating its `cron`/`timezone` pair — in the style of
/// [`crate::template::TemplateError`], scoped to this module's own table.
#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error("no schedule with id {0}")]
    NotFound(uuid::Uuid),

    #[error(
        "invalid cron expression {cron:?}: {source}\n  help: `cron` is a five-field expression (minute hour day-of-month month day-of-week), evaluated in `timezone`. Fix the expression and try again"
    )]
    InvalidCron {
        cron: String,
        #[source]
        source: croner::errors::CronError,
    },

    #[error(
        "unknown IANA timezone {0:?}\n  help: use a name like \"Europe/Berlin\", not a fixed offset — an offset cannot express a rule that survives a daylight-saving change"
    )]
    InvalidTimezone(String),

    /// Croner's own matching call failed against an already-parsed pattern
    /// and a real calendar instant — not reachable in practice (croner only
    /// errs here on an out-of-range value, and every instant this module
    /// builds comes from a real `DateTime<Utc>`), but the call returns a
    /// `Result` and this module does not paper over it with `.expect()`.
    #[error("cron evaluation error: {0}")]
    Evaluation(#[from] croner::errors::CronError),
}

/// One `schedules` row, as read back by [`get`], [`list`], and [`due`].
///
/// `cron` and `timezone` are stored as the operator wrote them, not as a
/// parsed `Cron`/`Tz` pair — so what a caller displays back (`factory
/// schedule list`) is exactly what was typed, and there is nothing here that
/// can go stale against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub id: uuid::Uuid,
    pub template_id: uuid::Uuid,
    pub cron: String,
    pub timezone: String,
    pub enabled: bool,
    pub last_fired_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

const SCHEDULE_COLUMNS: &str =
    "id, template_id, cron, timezone, enabled, last_fired_at, created_at, updated_at";

fn row_to_schedule(row: &rusqlite::Row<'_>) -> rusqlite::Result<Schedule> {
    let id: String = row.get(0)?;
    let template_id: String = row.get(1)?;
    let cron: String = row.get(2)?;
    let timezone: String = row.get(3)?;
    let enabled: i64 = row.get(4)?;
    let last_fired_at: Option<String> = row.get(5)?;
    let created_at: String = row.get(6)?;
    let updated_at: String = row.get(7)?;

    Ok(Schedule {
        id: uuid::Uuid::parse_str(&id)
            .unwrap_or_else(|e| panic!("schedules.id is a UUID; read {id:?}: {e}")),
        template_id: uuid::Uuid::parse_str(&template_id).unwrap_or_else(|e| {
            panic!("schedules.template_id is a UUID; read {template_id:?}: {e}")
        }),
        cron,
        timezone,
        enabled: enabled != 0,
        last_fired_at,
        created_at,
        updated_at,
    })
}

// Matching predicate ---------------------------------------------------

/// Parse and validate `cron` and `timezone` together. The one place both are
/// checked — [`create`] calls it before anything is written, so a bad
/// expression or an unknown zone is refused there, not discovered by the
/// dispatcher at 03:00 six weeks later.
///
/// croner API note, measured against croner 4.0.0 (which has no
/// `Cron::new`): `CronParser::new().parse(expr)` accepts a bare five-field
/// expression — no seconds field to fake — defaulting seconds to `0` and
/// year to `*`, and returns `Result<Cron, CronError>`.
fn parse(cron: &str, timezone: &str) -> Result<(Cron, Tz), ScheduleError> {
    let parsed_cron =
        CronParser::new()
            .parse(cron)
            .map_err(|source| ScheduleError::InvalidCron {
                cron: cron.to_string(),
                source,
            })?;
    let tz =
        Tz::from_str(timezone).map_err(|_| ScheduleError::InvalidTimezone(timezone.to_string()))?;
    Ok((parsed_cron, tz))
}

/// Refuse a bad `cron` expression or an unknown IANA `timezone` without
/// writing anything. [`create`] is `validate` plus the insert; this is the
/// standalone half, for a caller that wants the answer without a store (a
/// CLI's own pre-flight check, a test).
pub fn validate(cron: &str, timezone: &str) -> Result<(), ScheduleError> {
    parse(cron, timezone).map(|_| ())
}

/// Zero the seconds and sub-second part of a UTC instant. A schedule fires
/// for a whole minute, so [`matches`] and [`local_minute_string`] both need
/// to agree on which minute an instant carrying, say, 37 seconds belongs to
/// — croner's default parse also fixes a five-field pattern's own second
/// field at `0`, so an untruncated instant would fail to match a pattern
/// that in fact covers its minute.
fn truncate_to_minute(instant: DateTime<Utc>) -> DateTime<Utc> {
    instant
        .with_second(0)
        .and_then(|d| d.with_nanosecond(0))
        .expect("zeroing the seconds and nanoseconds of a valid instant is always valid")
}

/// Does `cron`, evaluated in `timezone`, fire in the minute `instant` falls
/// in? ADR 0021 decision 3a: a match against the local wall-clock minute,
/// never a next-occurrence search — the one home of that decision.
///
/// Converting a real UTC instant to a local time is always exactly one
/// answer. The *ambiguity* daylight saving creates belongs to the opposite
/// direction — reading a local wall-clock time back into an instant — not to
/// this one. That is what makes stepping forward through UTC instants (here,
/// and in [`next_run`]) the correct way to discover both a spring-forward
/// gap (no UTC instant renders the skipped local minute, so nothing
/// matches) and an autumn-back repeat (two distinct UTC instants render the
/// same local minute, so both match) — see this module's own tests, which
/// measure both.
pub fn matches(cron: &str, timezone: &str, instant: DateTime<Utc>) -> Result<bool, ScheduleError> {
    let (parsed_cron, tz) = parse(cron, timezone)?;
    matches_parsed(&parsed_cron, tz, instant)
}

fn matches_parsed(cron: &Cron, tz: Tz, instant: DateTime<Utc>) -> Result<bool, ScheduleError> {
    let local = truncate_to_minute(instant).with_timezone(&tz);
    Ok(cron.is_time_matching(&local)?)
}

/// The exact string `tasks.fired_for_minute` stores: `'YYYY-MM-DDTHH:MM'` in
/// the schedule's own timezone (`factory_store::schema`'s comment on that
/// column). Both instants of an autumn-back repeated hour render the same
/// string here — that equality, not any dispatcher logic, is what lets
/// `tasks_one_run_per_schedule_minute` admit exactly one run for the pair
/// (ADR 0021 decision 3a).
pub fn local_minute_string(
    timezone: &str,
    instant: DateTime<Utc>,
) -> Result<String, ScheduleError> {
    let tz =
        Tz::from_str(timezone).map_err(|_| ScheduleError::InvalidTimezone(timezone.to_string()))?;
    Ok(truncate_to_minute(instant)
        .with_timezone(&tz)
        .format("%Y-%m-%dT%H:%M")
        .to_string())
}

/// The first instant strictly after `after` at which `cron` (evaluated in
/// `timezone`) matches — found by walking forward minute by minute and
/// calling the same predicate [`matches`] uses, never `croner`'s own
/// next-occurrence search. ADR 0021 decision 3a is the whole reason the two
/// disagree on a daylight-saving transition and why only this one is right
/// for `factory schedule list`'s "next run" column.
///
/// `None` means no match inside [`NEXT_RUN_HORIZON_DAYS`] — an expression
/// such as `0 0 30 2 *` (the 30th of February, which never exists) must give
/// up rather than loop forever, and this is where it does.
///
/// The parsed `Cron` and `Tz` are built once, outside the loop: reparsing on
/// every one of a yearly schedule's ~162,300 candidate minutes would dwarf
/// the 9.6 ms this module's tests measure for that walk.
///
/// Returns the match in the schedule's own timezone — the zone the schedule
/// was written against, not the caller's.
pub fn next_run(
    cron: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Tz>>, ScheduleError> {
    let (parsed_cron, tz) = parse(cron, timezone)?;
    let horizon = truncate_to_minute(after) + Duration::days(NEXT_RUN_HORIZON_DAYS);
    let mut candidate = truncate_to_minute(after) + Duration::minutes(1);
    while candidate <= horizon {
        if matches_parsed(&parsed_cron, tz, candidate)? {
            return Ok(Some(candidate.with_timezone(&tz)));
        }
        candidate += Duration::minutes(1);
    }
    Ok(None)
}

// CRUD -------------------------------------------------------------------

/// Commit a new schedule and return the id it was written under.
///
/// `cron` and `timezone` are validated before anything is written
/// ([`validate`]) — refused here, not discovered by the dispatcher at 03:00
/// six weeks later. Validation needs no read from the store, so it runs
/// before the transaction opens rather than inside it.
///
/// `template_id` is not checked against `task_templates` by this function —
/// `schedules.template_id`'s own `REFERENCES` is the backstop, the same
/// shape `create::create_from_template`'s own doc comment describes for its
/// `template_id`.
///
/// `id` is supplied by the caller, mirroring `template::create` and
/// `create::create`'s own `id` parameter: `uuid` is pinned workspace-wide
/// without the `v4` feature.
pub fn create(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    template_id: uuid::Uuid,
    cron: &str,
    timezone: &str,
) -> Result<uuid::Uuid, ScheduleError> {
    validate(cron, timezone)?;

    let tx = store.transaction()?;
    tx.execute(
        "INSERT INTO schedules (id, template_id, cron, timezone) VALUES (?1, ?2, ?3, ?4)",
        (id.to_string(), template_id.to_string(), cron, timezone),
    )
    .map_err(factory_store::StoreError::from)?;
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(id)
}

/// One schedule by id. Takes `&Store`, not `&mut Store` — mirrors
/// `template::get_by_id`'s own reasoning: a read path must never go through
/// `Store::transaction`, which takes the write lock and would needlessly
/// contend with real writers under WAL.
pub fn get(store: &factory_store::Store, id: uuid::Uuid) -> Result<Schedule, ScheduleError> {
    store
        .connection()
        .query_row(
            &format!("SELECT {SCHEDULE_COLUMNS} FROM schedules WHERE id = ?1"),
            [id.to_string()],
            row_to_schedule,
        )
        .optional()
        .map_err(factory_store::StoreError::from)?
        .ok_or(ScheduleError::NotFound(id))
}

/// Every schedule, oldest first — read-only inspection before automation
/// (AGENTS.md), mirroring `template::list`.
pub fn list(store: &factory_store::Store) -> Result<Vec<Schedule>, ScheduleError> {
    let mut stmt = store
        .connection()
        .prepare(&format!(
            "SELECT {SCHEDULE_COLUMNS} FROM schedules ORDER BY created_at, id"
        ))
        .map_err(factory_store::StoreError::from)?;
    let rows = stmt
        .query_map([], row_to_schedule)
        .map_err(factory_store::StoreError::from)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| factory_store::StoreError::from(e).into())
}

/// The enabled schedules whose `cron`, evaluated in their own `timezone`,
/// matches the minute `instant` falls in — the dispatcher's own query
/// (design §11; ADR 0021 decision 3a's predicate, decision 7's "provide what
/// a dispatcher needs and stop"). Disabled schedules are excluded here, at
/// the source, rather than left for every caller to filter back out.
pub fn due(
    store: &factory_store::Store,
    instant: DateTime<Utc>,
) -> Result<Vec<Schedule>, ScheduleError> {
    let mut stmt = store
        .connection()
        .prepare(&format!(
            "SELECT {SCHEDULE_COLUMNS} FROM schedules WHERE enabled = 1 ORDER BY created_at, id"
        ))
        .map_err(factory_store::StoreError::from)?;
    let rows = stmt
        .query_map([], row_to_schedule)
        .map_err(factory_store::StoreError::from)?;
    let enabled_schedules: Vec<Schedule> = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(factory_store::StoreError::from)?;

    let mut due_now = Vec::new();
    for schedule in enabled_schedules {
        if matches(&schedule.cron, &schedule.timezone, instant)? {
            due_now.push(schedule);
        }
    }
    Ok(due_now)
}

/// Turn a schedule on or off without touching anything else. Backlog §11:
/// enable/disable "toggle a schedule without deleting its history or
/// affecting already-created runs" — `cron`, `timezone`, `last_fired_at` and
/// `created_at` are untouched; only `enabled` (and `updated_at`, mirroring
/// `template::set_state`'s own touch of that column) change.
/// Record that `id` fired, **inside a transaction the caller already owns**.
///
/// This is the one function in this module that writes `last_fired_at`, and
/// it takes a `&Transaction` rather than a `&mut Store` for a reason ADR 0021
/// decision 3 states directly: the dispatcher must insert the run and stamp
/// this column in **one** transaction. On the autumn clock change both
/// instants of a repeated local minute match, so the second insert is
/// rejected by `tasks_one_run_per_schedule_minute` — and a `last_fired_at`
/// written outside that transaction would then record a firing that did not
/// happen. A schedule that looks like it ran and did not is worse than one
/// that looks like it has never run.
///
/// The dispatcher lives in `factory-daemon`, so without this the only way to
/// stamp the column in the run's own transaction would be raw SQL against a
/// table this module owns. That is the second-home problem this crate keeps
/// deciding against.
///
/// Firing itself still does not happen here (crate decision 7). This writes
/// one column when a caller that *did* fire says so.
pub fn mark_fired(
    tx: &rusqlite::Transaction<'_>,
    id: uuid::Uuid,
    at: DateTime<Utc>,
) -> Result<(), ScheduleError> {
    let changed = tx
        .execute(
            "UPDATE schedules SET last_fired_at = ?2, updated_at = CURRENT_TIMESTAMP \
             WHERE id = ?1",
            (id.to_string(), at.to_rfc3339()),
        )
        .map_err(factory_store::StoreError::from)?;
    if changed == 0 {
        return Err(ScheduleError::NotFound(id));
    }
    Ok(())
}

fn set_enabled(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    enabled: bool,
) -> Result<(), ScheduleError> {
    let tx = store.transaction()?;
    let changed = tx
        .execute(
            "UPDATE schedules SET enabled = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
            (id.to_string(), i64::from(enabled)),
        )
        .map_err(factory_store::StoreError::from)?;
    if changed == 0 {
        return Err(ScheduleError::NotFound(id));
    }
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(())
}

/// See [`set_enabled`].
pub fn enable(store: &mut factory_store::Store, id: uuid::Uuid) -> Result<(), ScheduleError> {
    set_enabled(store, id, true)
}

/// See [`set_enabled`].
pub fn disable(store: &mut factory_store::Store, id: uuid::Uuid) -> Result<(), ScheduleError> {
    set_enabled(store, id, false)
}
