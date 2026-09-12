//! What a hall on the site plan is allowed to say about its scope.
//!
//! Two signals live here, and the whole point of putting them in one module is
//! that they must not be confused:
//!
//! * **size** -- how much repository there is. Files, the bytes behind them,
//!   and the directories they sit in, all from the one bounded walk `site.rs`
//!   already does. It moves when somebody commits, which is rarely, and it is
//!   the only thing allowed to decide the building's *structure*: how many
//!   floors it has, how wide its footprint is, how many window bays a floor
//!   carries.
//! * **activity** -- what Factory is doing in that scope right now. Runs in
//!   flight, tasks queued at the door, agents standing up. It moves every few
//!   seconds, and it is only allowed to decide what is *lit*: how far up the
//!   building the lights go, what the roof beacon says, how fast it beats.
//!
//! No cue may reach a dimension. That is not a stylistic preference: a hall's
//! footprint is what `layoutHalls` places the site from, so a run starting
//! would physically move its neighbours -- and a taller building on a busy
//! morning would mean nobody could read height as size again.
//! `activity_never_moves_the_building` in the tests below holds the line.
//!
//! Both mappings are stepped, and both steps are sticky. A metric sitting on a
//! threshold would otherwise flip the hall between two shapes on every poll,
//! so `tier_for` and `level_for` take the previous answer and only leave it
//! once the number has cleared the boundary by a margin. The exact states --
//! nothing measured, nothing tracked, nothing happening -- carry no dead band,
//! because there is nothing to oscillate around.

use serde::{Deserialize, Serialize};

// ------------------------------------------------------------ what was found

/// What one walk of a scope counted. Every field is something a person could
/// count by hand and get the same answer -- nothing here is sampled, modelled
/// or estimated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoMetrics {
    /// False when the scope's directory could not be read at all. Every count
    /// below is then zero, which is not the same fact as a scope that was
    /// walked and found empty -- and the two are drawn differently.
    pub known: bool,
    pub files: u64,
    /// Of those, the ones with a source-code extension. Shown in words, never
    /// scored: what counts as source is a judgement, and a judgement has no
    /// business deciding how tall a building is.
    pub source_files: u64,
    pub bytes: u64,
    /// Directories holding at least one counted file -- the modules.
    pub directories: u64,
    /// True when the walk stopped at its cap. The numbers are then a lower
    /// bound, so the building is drawn at what was counted and says so rather
    /// than claiming a measurement it does not have.
    pub truncated: bool,
}

/// What Factory is doing in a scope right now. Every number comes from the
/// daemon's own records -- runs, tasks, standing agents -- and none of it is
/// read off a terminal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// Runs in flight: dispatching, running, or blocked.
    pub active_runs: u32,
    /// Of those, the ones waiting on a person.
    pub blocked_runs: u32,
    /// Tasks assigned to this scope that have not started.
    pub queued_tasks: u32,
    /// Standing agents with a session up.
    pub live_agents: u32,
    /// Standing agents that failed to start.
    pub failed_agents: u32,
    /// Agents the scope declares. The capacity the load is read against, so
    /// two runs mean something different in a one-agent scope and a six-agent
    /// one.
    pub declared_agents: u32,
}

/// A queued task is real work, but it is not work in progress: it counts for
/// half a run when the load is worked out.
const QUEUED_WEIGHT: u32 = 50;
/// Past this there is nothing left to say -- every light is already on.
const LOAD_CAP: u32 = 600;

impl Activity {
    /// What the load is read against. A scope that declares no agents still
    /// gets one, or the first run in it would read as infinite load.
    pub fn capacity(&self) -> u32 {
        self.declared_agents.max(1)
    }

    /// Work in flight and waiting, in hundredths of a run.
    pub fn demand(&self) -> u32 {
        self.active_runs * 100 + self.queued_tasks * QUEUED_WEIGHT
    }

    /// Demand against capacity, as a percentage. A hundred means every
    /// declared agent has a run; more than that is a backlog.
    pub fn load_pct(&self) -> u32 {
        (self.demand() / self.capacity()).min(LOAD_CAP)
    }

    /// Whether anything is happening at all. Exact, and so not subject to the
    /// dead band: when the last run ends the scope is idle now, not once a
    /// margin has been cleared.
    pub fn is_idle(&self) -> bool {
        self.active_runs == 0 && self.queued_tasks == 0
    }
}

// ----------------------------------------------------------------- the score

/// Where each count stops counting. A repository past a cap is drawn at the
/// cap: the largest building has to be some size, and this is it.
const FILES_CAP: f64 = 20_000.0;
const BYTES_CAP: f64 = 256.0 * 1024.0 * 1024.0;
const DIRS_CAP: f64 = 1_500.0;

/// Files carry the most weight because file count is what separates real
/// repositories from each other; bytes the least, because one vendored
/// binary can be worth a thousand source files and should not be a storey.
const FILES_WEIGHT: f64 = 0.55;
const BYTES_WEIGHT: f64 = 0.20;
const DIRS_WEIGHT: f64 = 0.25;

/// How much repository there is, on one bounded scale: 0 for nothing, 1000 for
/// a repository at or past every cap.
///
/// Logarithmic in each count, because repositories are: ten files against a
/// hundred is the same kind of difference as a thousand against ten thousand,
/// and on a linear scale every repository but the largest would be a hut.
pub fn size_score(metrics: &RepoMetrics) -> u32 {
    if !metrics.known {
        return 0;
    }
    let part = |value: u64, cap: f64| -> f64 {
        let value = (value as f64).min(cap);
        ((1.0 + value).ln() / (1.0 + cap).ln()).clamp(0.0, 1.0)
    };
    let score = FILES_WEIGHT * part(metrics.files, FILES_CAP)
        + BYTES_WEIGHT * part(metrics.bytes, BYTES_CAP)
        + DIRS_WEIGHT * part(metrics.directories, DIRS_CAP);
    (score * 1000.0).round().clamp(0.0, 1000.0) as u32
}

// ------------------------------------------------------------------ the tier

/// How developed a building is. Steps rather than a continuum, so a person can
/// learn what a shape means and recognise it across the site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// The scope's directory could not be read. Drawn as a plain building of
    /// no particular size, next to a floor that says "not recorded" -- not as
    /// a small one, which would be a measurement nobody made.
    Unknown,
    /// Walked, and there is nothing in it. A marked-out plot.
    Plot,
    Shed,
    Workshop,
    Works,
    Plant,
}

/// Where each tier begins and the next one takes over. `Unknown` and `Plot`
/// are not bands: they are the two exact states.
const TIER_BANDS: &[(Tier, u32, u32)] = &[
    (Tier::Shed, 1, 300),
    (Tier::Workshop, 300, 480),
    (Tier::Works, 480, 700),
    (Tier::Plant, 700, 1001),
];

/// How far past a boundary the score has to go before the building changes
/// shape. A fortieth of the scale: wide enough that a file added and removed
/// cannot rebuild a storey, narrow enough that real growth still shows.
const TIER_MARGIN: u32 = 25;

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Plot => "plot",
            Self::Shed => "shed",
            Self::Workshop => "workshop",
            Self::Works => "works",
            Self::Plant => "plant",
        }
    }

    /// The fewest and the most floors a building of this tier may have.
    pub fn floors(self) -> (u32, u32) {
        match self {
            Self::Unknown => (2, 2),
            Self::Plot => (1, 1),
            Self::Shed => (1, 2),
            Self::Workshop => (3, 4),
            Self::Works => (5, 6),
            Self::Plant => (7, 8),
        }
    }

    /// The narrowest and widest footprint of this tier, in tenths of a grid
    /// unit -- tenths so the wire carries integers and two daemons drawing the
    /// same repository cannot disagree in the eighth decimal place.
    pub fn width_tenths(self) -> (u32, u32) {
        match self {
            Self::Unknown => (40, 40),
            Self::Plot => (28, 28),
            Self::Shed => (30, 40),
            Self::Workshop => (40, 55),
            Self::Works => (55, 75),
            Self::Plant => (75, 105),
        }
    }

    fn band(self) -> (u32, u32) {
        TIER_BANDS
            .iter()
            .find(|(tier, _, _)| *tier == self)
            .map(|(_, from, to)| (*from, *to))
            .unwrap_or((0, 1))
    }

    /// The tier a score lands in with no memory of what came before.
    fn from_score(score: u32) -> Self {
        if score == 0 {
            return Self::Plot;
        }
        TIER_BANDS
            .iter()
            .find(|(_, _, to)| score < *to)
            .map(|(tier, _, _)| *tier)
            .unwrap_or(Self::Plant)
    }

    /// Whether this tier came from a measurement at all. The two that did not
    /// stay out of the dead band: there is no wobbling between "unreadable"
    /// and "readable".
    fn is_measured(self) -> bool {
        !matches!(self, Self::Unknown | Self::Plot)
    }
}

/// The tier for a score, given the tier the hall already had.
///
/// Sticky in both directions: to move up, the score has to clear the next
/// tier's threshold by `TIER_MARGIN`; to move down, it has to fall that far
/// below its own. Shifting the score and taking the larger (or smaller) of the
/// two answers handles a jump of several tiers correctly, which a single
/// boundary check does not.
pub fn tier_for(metrics: &RepoMetrics, score: u32, previous: Option<Tier>) -> Tier {
    if !metrics.known {
        return Tier::Unknown;
    }
    if score == 0 {
        return Tier::Plot;
    }
    let plain = Tier::from_score(score);
    let Some(previous) = previous.filter(|t| t.is_measured()) else {
        return plain;
    };
    match plain.cmp(&previous) {
        std::cmp::Ordering::Equal => previous,
        std::cmp::Ordering::Greater => {
            Tier::from_score(score.saturating_sub(TIER_MARGIN)).max(previous)
        }
        std::cmp::Ordering::Less => Tier::from_score(score + TIER_MARGIN).min(previous),
    }
}

/// The building itself. Every field is a function of the size score and of
/// nothing else -- see the module note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shape {
    pub tier: Tier,
    /// The score the shape came from, so a details panel can show its working.
    pub score: u32,
    pub floors: u32,
    /// Footprint in tenths of a grid unit. The page divides by ten; a grid
    /// unit is whatever the site plan says it is.
    pub width_tenths: u32,
    pub depth_tenths: u32,
    /// Window bays across one face of one floor.
    pub bays: u32,
}

/// How deep a building is against its width. The site plan's halls have been
/// this shape since the prototype; keeping the ratio here means a wider hall
/// stays the same building rather than turning into a different one.
const DEPTH_RATIO: u32 = 86; // per cent of the width

/// The most and fewest window bays a face is drawn with. A wider building gets
/// a wider footprint past the cap, not an ever-denser grid of smaller panes.
const MIN_BAYS: u32 = 2;
const MAX_BAYS: u32 = 7;

/// The building a tier and a score make. The tier sets the range, the score
/// says where in it, so a repository growing inside its tier still visibly
/// grows instead of waiting for the next threshold.
pub fn shape_for(tier: Tier, score: u32) -> Shape {
    let (from, to) = tier.band();
    // A sticky tier can hold a score from outside its own band, and the two
    // exact tiers have no band at all. Clamping is the honest answer: the
    // building sits at the end of the range it is actually in.
    let progress = if to > from + 1 {
        ((score.clamp(from, to - 1) - from) as f64) / ((to - 1 - from) as f64)
    } else {
        0.0
    };
    let step = |(min, max): (u32, u32)| -> u32 { min + ((max - min) as f64 * progress).round() as u32 };
    let width_tenths = step(tier.width_tenths());
    Shape {
        tier,
        score,
        floors: step(tier.floors()),
        width_tenths,
        depth_tenths: (width_tenths * DEPTH_RATIO).div_ceil(100),
        bays: (width_tenths / 10).clamp(MIN_BAYS, MAX_BAYS),
    }
}

// -------------------------------------------------------------- the activity

/// How busy a scope is, in four steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityLevel {
    /// No run, no queue. The building may still be staffed.
    Idle,
    Low,
    Busy,
    /// More work than there are agents to do it.
    Peak,
}

/// Where each level begins, as a percentage of capacity.
const LEVEL_BANDS: &[(ActivityLevel, u32, u32)] = &[
    (ActivityLevel::Low, 1, 60),
    (ActivityLevel::Busy, 60, 140),
    (ActivityLevel::Peak, 140, u32::MAX),
];

/// The dead band on the load, in points of percent. A run that ends and is
/// retried a second later must not restage the whole facade twice.
const LEVEL_MARGIN: u32 = 12;

impl ActivityLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Low => "low",
            Self::Busy => "busy",
            Self::Peak => "peak",
        }
    }

    fn from_load(load_pct: u32) -> Self {
        if load_pct == 0 {
            return Self::Idle;
        }
        LEVEL_BANDS
            .iter()
            .find(|(_, _, to)| load_pct < *to)
            .map(|(level, _, _)| *level)
            .unwrap_or(Self::Peak)
    }
}

/// The level for an activity, given the level the hall already had. Sticky the
/// way the tier is, except that idle is exact: when the last run ends the
/// lights come down on that poll, not on the one after the margin.
pub fn level_for(activity: &Activity, previous: Option<ActivityLevel>) -> ActivityLevel {
    if activity.is_idle() {
        return ActivityLevel::Idle;
    }
    let load = activity.load_pct();
    let plain = ActivityLevel::from_load(load);
    let Some(previous) = previous.filter(|l| *l != ActivityLevel::Idle) else {
        return plain;
    };
    match plain.cmp(&previous) {
        std::cmp::Ordering::Equal => previous,
        std::cmp::Ordering::Greater => {
            ActivityLevel::from_load(load.saturating_sub(LEVEL_MARGIN)).max(previous)
        }
        std::cmp::Ordering::Less => ActivityLevel::from_load(load + LEVEL_MARGIN).min(previous),
    }
}

/// What the roof beacon is saying. The order these are decided in is the order
/// `hallState` in the site plan has always used: a person waiting on is the
/// most urgent thing a hall can show, ahead of work in progress; an agent that
/// failed to start is a real problem but a quieter one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Beacon {
    Off,
    /// Work is at the door and no run has taken it yet.
    Waiting,
    /// A run is going.
    Working,
    /// A standing agent failed to start.
    Fault,
    /// A run is waiting on a person.
    Blocked,
}

impl Beacon {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Waiting => "waiting",
            Self::Working => "working",
            Self::Fault => "fault",
            Self::Blocked => "blocked",
        }
    }
}

/// Everything activity is allowed to change. Emissive only: not one field here
/// is a dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cues {
    pub level: ActivityLevel,
    pub load_pct: u32,
    /// Floors lit from the ground up, never more than the building has.
    pub lit_floors: u32,
    pub beacon: Beacon,
    /// How long one beat of the beacon takes. Zero means it does not move --
    /// a still building is the honest drawing of a scope where nothing is
    /// running, including one where somebody is being waited on.
    pub pulse_ms: u32,
    /// How hard the lit floors glow, 0..=100.
    pub glow_pct: u32,
}

/// The slowest and fastest the beacon beats, and what each concurrent run
/// takes off the period.
const PULSE_SLOWEST_MS: u32 = 2400;
const PULSE_FASTEST_MS: u32 = 900;
const PULSE_STEP_MS: u32 = 300;

/// What to light on a building of this shape doing this much.
///
/// Takes the shape because the lights are drawn on it -- the floor count
/// bounds how many can be lit -- and never the other way round.
pub fn cues_for(shape: &Shape, activity: &Activity, level: ActivityLevel) -> Cues {
    let load_pct = activity.load_pct();

    let lit_floors = if activity.is_idle() {
        // Nothing running, but somebody is in: the ground floor stays on. A
        // scope with no live agent and no work is dark, and being able to see
        // that across a whole site at a glance is the point of the cue.
        u32::from(activity.live_agents > 0)
    } else {
        let share = (shape.floors as f64 * load_pct.min(100) as f64 / 100.0).round() as u32;
        share.clamp(1, shape.floors)
    }
    .min(shape.floors);

    let beacon = if activity.blocked_runs > 0 {
        Beacon::Blocked
    } else if activity.active_runs > 0 {
        Beacon::Working
    } else if activity.failed_agents > 0 {
        Beacon::Fault
    } else if activity.queued_tasks > 0 {
        Beacon::Waiting
    } else {
        Beacon::Off
    };

    // Only a run that is actually going makes the beacon move, and more of
    // them make it move faster. A blocked run is not progress: it holds still
    // and waits for a person, which is exactly what it is doing.
    let pulse_ms = match beacon {
        Beacon::Working => PULSE_SLOWEST_MS
            .saturating_sub(PULSE_STEP_MS * activity.active_runs.saturating_sub(1))
            .max(PULSE_FASTEST_MS),
        Beacon::Blocked | Beacon::Fault | Beacon::Waiting | Beacon::Off => 0,
    };

    let glow_pct = match level {
        ActivityLevel::Idle => u32::from(lit_floors > 0) * 20,
        ActivityLevel::Low => 45,
        ActivityLevel::Busy => 75,
        ActivityLevel::Peak => 100,
    };

    Cues {
        level,
        load_pct,
        lit_floors,
        beacon,
        pulse_ms,
        glow_pct,
    }
}

/// The whole mapping in one call: what a scope is, into what it looks like.
/// `previous` is the pair this same scope was drawn with last time, which is
/// what makes both steps sticky; `None` is a hall being drawn for the first
/// time, which has nothing to be sticky about.
pub fn appearance(
    metrics: &RepoMetrics,
    activity: &Activity,
    previous: Option<(Tier, ActivityLevel)>,
) -> (Shape, Cues) {
    let score = size_score(metrics);
    let tier = tier_for(metrics, score, previous.map(|(t, _)| t));
    let shape = shape_for(tier, score);
    let level = level_for(activity, previous.map(|(_, l)| l));
    let cues = cues_for(&shape, activity, level);
    (shape, cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walked(files: u64, bytes: u64, directories: u64) -> RepoMetrics {
        RepoMetrics {
            known: true,
            files,
            source_files: files,
            bytes,
            directories,
            truncated: false,
        }
    }

    fn busy(active_runs: u32, queued_tasks: u32, declared_agents: u32) -> Activity {
        Activity {
            active_runs,
            blocked_runs: 0,
            queued_tasks,
            live_agents: declared_agents,
            failed_agents: 0,
            declared_agents,
        }
    }

    fn shape(metrics: &RepoMetrics) -> Shape {
        appearance(metrics, &Activity::default(), None).0
    }

    fn cues(metrics: &RepoMetrics, activity: &Activity) -> Cues {
        appearance(metrics, activity, None).1
    }

    // -------------------------------------------------------------- the score

    #[test]
    fn a_bigger_repository_never_scores_lower() {
        let mut last = 0;
        for files in [0u64, 1, 5, 40, 300, 2_000, 12_000, 20_000, 200_000] {
            let score = size_score(&walked(files, files * 3_000, files / 8));
            assert!(score >= last, "{files} files scored {score}, below the size under it ({last})");
            last = score;
        }
    }

    #[test]
    fn the_score_is_bounded_at_both_ends_and_stops_at_the_caps() {
        assert_eq!(size_score(&walked(0, 0, 0)), 0);
        let at_cap = size_score(&walked(20_000, 256 * 1024 * 1024, 1_500));
        let past_cap = size_score(&walked(u64::MAX, u64::MAX, u64::MAX));
        assert_eq!(at_cap, past_cap, "past every cap the score stops climbing");
        assert!(past_cap <= 1000);
    }

    #[test]
    fn the_same_repository_always_scores_the_same() {
        let metrics = walked(412, 3_200_000, 61);
        assert_eq!(size_score(&metrics), size_score(&metrics.clone()));
        assert_eq!(shape(&metrics), shape(&metrics.clone()));
    }

    // --------------------------------------------------------- size to shape

    #[test]
    fn an_empty_scope_is_a_plot_and_is_still_drawable() {
        let s = shape(&walked(0, 0, 0));
        assert_eq!(s.tier, Tier::Plot);
        assert_eq!(s.floors, 1, "one floor, never zero -- nothing is invisible");
        assert!(s.width_tenths >= 20 && s.depth_tenths >= 20, "a plot still has a footprint");
        assert!(s.bays >= MIN_BAYS);
    }

    #[test]
    fn a_scope_that_could_not_be_read_is_not_drawn_as_a_small_one() {
        let s = shape(&RepoMetrics::default()); // known: false
        assert_eq!(s.tier, Tier::Unknown);
        assert_eq!(s.score, 0);
        assert!(
            s.width_tenths > shape(&walked(0, 0, 0)).width_tenths,
            "unreadable is not the same fact as empty, and must not read as smaller"
        );
        assert!(s.floors >= 1 && s.bays >= MIN_BAYS, "and it is still a building");
    }

    #[test]
    fn a_bigger_repository_is_a_bigger_building_and_the_biggest_is_bounded() {
        let small = shape(&walked(5, 20_000, 2));
        let medium = shape(&walked(500, 8_000_000, 60));
        let large = shape(&walked(8_000, 120 * 1024 * 1024, 900));
        let absurd = shape(&walked(4_000_000, 900 * 1024 * 1024 * 1024, 400_000));

        assert!(small.floors < medium.floors && medium.floors < large.floors);
        assert!(small.width_tenths < medium.width_tenths && medium.width_tenths < large.width_tenths);
        assert_eq!(large.tier, Tier::Plant);
        assert_eq!(absurd.tier, Tier::Plant);
        assert_eq!(absurd.floors, Tier::Plant.floors().1, "the tallest is still a height");
        assert_eq!(absurd.width_tenths, Tier::Plant.width_tenths().1);
        assert!(absurd.bays <= MAX_BAYS, "a face gets wider, not denser");
    }

    #[test]
    fn every_tier_draws_a_building_with_a_floor_a_footprint_and_windows() {
        for tier in [Tier::Unknown, Tier::Plot, Tier::Shed, Tier::Workshop, Tier::Works, Tier::Plant] {
            for score in [0, 1, 299, 300, 479, 480, 699, 700, 1000] {
                let s = shape_for(tier, score);
                assert!(s.floors >= 1, "{tier:?} at {score} had no floors");
                assert!(s.width_tenths >= 20 && s.depth_tenths >= 20, "{tier:?} at {score} had no footprint");
                assert!((MIN_BAYS..=MAX_BAYS).contains(&s.bays), "{tier:?} at {score} had {} bays", s.bays);
                let (lo, hi) = tier.floors();
                assert!((lo..=hi).contains(&s.floors), "{tier:?} at {score} left its own floor range");
            }
        }
    }

    #[test]
    fn growth_inside_a_tier_still_shows() {
        let low = shape_for(Tier::Plant, 700);
        let high = shape_for(Tier::Plant, 1000);
        assert!(high.floors > low.floors, "a tier is a range, not one shape");
        assert!(high.width_tenths > low.width_tenths);
    }

    // ----------------------------------------------- the line between signals

    #[test]
    fn activity_never_moves_the_building() {
        // The hard version of the rule: `layoutHalls` places the site from
        // these numbers, so a cue reaching one of them would shove a hall's
        // neighbours across the apron every time a run started.
        let metrics = walked(900, 9_000_000, 120);
        let quiet = appearance(&metrics, &busy(0, 0, 2), None);
        let loud = appearance(&metrics, &busy(6, 9, 2), None);
        assert_eq!(quiet.0, loud.0, "activity may light a building, never build one");
        assert_ne!(quiet.1, loud.1, "and it has to change something");
        assert!(loud.1.lit_floors > quiet.1.lit_floors);
    }

    #[test]
    fn activity_never_lights_more_floors_than_there_are() {
        let tiny = walked(3, 900, 1);
        for (runs, queued) in [(1, 0), (4, 0), (40, 200), (1, 99)] {
            let c = cues(&tiny, &busy(runs, queued, 1));
            let s = shape(&tiny);
            assert!(c.lit_floors <= s.floors, "{runs} runs, {queued} queued lit {} of {}", c.lit_floors, s.floors);
            assert!(c.lit_floors >= 1, "work in a building lights something");
        }
    }

    #[test]
    fn an_idle_scope_with_somebody_in_it_is_not_a_dark_one() {
        let metrics = walked(400, 2_000_000, 40);
        let staffed = Activity { live_agents: 1, declared_agents: 1, ..Default::default() };
        let empty = Activity { live_agents: 0, declared_agents: 1, ..Default::default() };
        assert_eq!(cues(&metrics, &staffed).lit_floors, 1);
        assert_eq!(cues(&metrics, &empty).lit_floors, 0);
        assert_eq!(cues(&metrics, &staffed).level, ActivityLevel::Idle, "staffed is not busy");
    }

    #[test]
    fn a_scope_at_peak_is_lit_all_the_way_up() {
        let metrics = walked(2_000, 20_000_000, 200);
        let c = cues(&metrics, &busy(4, 6, 1));
        assert_eq!(c.level, ActivityLevel::Peak);
        assert_eq!(c.lit_floors, shape(&metrics).floors);
        assert_eq!(c.glow_pct, 100);
    }

    #[test]
    fn the_same_work_reads_against_the_agents_the_scope_has() {
        let metrics = walked(2_000, 20_000_000, 200);
        let one = cues(&metrics, &busy(2, 0, 1));
        let six = cues(&metrics, &busy(2, 0, 6));
        assert!(one.lit_floors > six.lit_floors, "two runs is all of one agent and a third of six");
        assert_eq!(shape(&metrics), shape(&metrics), "and neither is a bigger building");
    }

    #[test]
    fn the_beacon_says_which_kind_of_busy_it_is() {
        let m = walked(400, 2_000_000, 40);
        assert_eq!(cues(&m, &busy(0, 0, 2)).beacon, Beacon::Off);
        assert_eq!(cues(&m, &busy(0, 0, 2)).pulse_ms, 0, "a still scope does not animate");

        let waiting = cues(&m, &busy(0, 3, 2));
        assert_eq!(waiting.beacon, Beacon::Waiting);
        assert_eq!(waiting.pulse_ms, 0, "waiting is not progress");

        let working = cues(&m, &busy(1, 0, 2));
        assert_eq!(working.beacon, Beacon::Working);
        assert!(working.pulse_ms > 0);

        let faster = cues(&m, &busy(4, 0, 4));
        assert!(faster.pulse_ms < working.pulse_ms, "more runs, a faster beat");
        assert!(faster.pulse_ms >= PULSE_FASTEST_MS, "and never a strobe");

        let blocked = cues(&m, &Activity { active_runs: 2, blocked_runs: 1, declared_agents: 2, live_agents: 2, ..Default::default() });
        assert_eq!(blocked.beacon, Beacon::Blocked, "a person is wanted, and that comes first");
        assert_eq!(blocked.pulse_ms, 0, "nothing moves until they come");

        let fault = cues(&m, &Activity { failed_agents: 1, declared_agents: 2, ..Default::default() });
        assert_eq!(fault.beacon, Beacon::Fault);
    }

    // ---------------------------------------------------------- the dead bands

    #[test]
    fn a_score_wobbling_on_a_boundary_does_not_rebuild_the_hall() {
        let m = walked(1, 1, 1);
        let mut tier = tier_for(&m, 510, None);
        assert_eq!(tier, Tier::Works);
        for score in [479, 481, 478, 482, 470, 485] {
            tier = tier_for(&m, score, Some(tier));
            assert_eq!(tier, Tier::Works, "score {score} should not have moved it");
        }
        // A move that means something still happens.
        assert_eq!(tier_for(&m, 440, Some(tier)), Tier::Workshop);
    }

    #[test]
    fn climbing_a_tier_takes_more_than_touching_the_threshold() {
        let m = walked(1, 1, 1);
        assert_eq!(tier_for(&m, 485, Some(Tier::Workshop)), Tier::Workshop, "a toe over the line is not a storey");
        assert_eq!(tier_for(&m, 510, Some(Tier::Workshop)), Tier::Works);
    }

    #[test]
    fn a_jump_of_several_tiers_lands_where_it_should() {
        let m = walked(1, 1, 1);
        assert_eq!(tier_for(&m, 900, Some(Tier::Shed)), Tier::Plant);
        assert_eq!(tier_for(&m, 50, Some(Tier::Plant)), Tier::Shed);
    }

    #[test]
    fn the_two_exact_states_answer_at_once_whatever_stood_there_before() {
        let emptied = walked(0, 0, 0);
        assert_eq!(tier_for(&emptied, 0, Some(Tier::Plant)), Tier::Plot);
        let gone = RepoMetrics::default();
        assert_eq!(tier_for(&gone, 0, Some(Tier::Plant)), Tier::Unknown);
        // And a scope that comes back is measured again straight away rather
        // than being held at the shape it had while it was unreadable.
        let back = walked(8_000, 120 * 1024 * 1024, 900);
        assert_eq!(tier_for(&back, size_score(&back), Some(Tier::Unknown)), Tier::Plant);
    }

    #[test]
    fn a_load_wobbling_on_a_boundary_does_not_restage_the_facade() {
        // 140% of capacity is where peak begins. With two agents that is two
        // runs and a queued task (150%) against two runs alone (100%).
        let mut level = level_for(&busy(2, 0, 2), None);
        assert_eq!(level, ActivityLevel::Busy);
        for (runs, queued) in [(2, 1), (2, 0), (2, 1), (2, 0)] {
            level = level_for(&busy(runs, queued, 2), Some(level));
            assert_eq!(level, ActivityLevel::Busy, "{runs} running and {queued} queued is the same kind of busy");
        }
        assert_eq!(level_for(&busy(4, 0, 2), Some(level)), ActivityLevel::Peak, "twice the agents' worth of work is not");
    }

    #[test]
    fn the_lights_come_down_the_moment_the_work_stops() {
        let level = level_for(&busy(6, 6, 1), None);
        assert_eq!(level, ActivityLevel::Peak);
        assert_eq!(
            level_for(&busy(0, 0, 1), Some(level)),
            ActivityLevel::Idle,
            "idle is exact -- it does not wait out a margin"
        );
    }

    #[test]
    fn a_hall_drawn_twice_from_the_same_facts_is_drawn_the_same_way() {
        let metrics = walked(700, 4_000_000, 80);
        let activity = busy(1, 0, 2);
        let first = appearance(&metrics, &activity, None);
        let second = appearance(&metrics, &activity, Some((first.0.tier, first.1.level)));
        assert_eq!(first, second);
    }
}
