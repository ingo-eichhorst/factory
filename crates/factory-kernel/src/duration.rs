//! A freshness window, written in the catalogue as `Nd`, `Nh`, or `Nw`
//! (days, hours, weeks). Stored as whole hours, so two durations compare
//! and take a minimum exactly, with nothing to round at the edges.
//! `chrono::Duration` has no serde support of its own and no `Ord`, so this
//! is its own small type rather than a wrapper around that one.
//!
//! Moved here unchanged from `factory_core::policy` (#193, phase 1, F7): it
//! is imported below L6 -- `factory-core`'s `dependencies.rs` (L2),
//! `quality.rs` (L5) and `config.rs`, and the daemon's `dependencies.rs`
//! tests -- so it belongs in the L0 kernel, not in the L6 module that first
//! defined it. `policy.rs` keeps `pub use factory_kernel::Duration`, so its
//! own callers and the wire (`PolicyControlDetail.max_age`, etc.) see no
//! change at all, and the serialized form -- `"30d"`, `"12h"`, `"2w"` --
//! stays exactly what it was.

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct Duration {
    hours: u64,
}

impl Duration {
    pub fn from_hours(hours: u64) -> Self {
        Self { hours }
    }

    pub fn as_hours(&self) -> u64 {
        self.hours
    }

    /// This window as a `chrono::TimeDelta`, capped at `TimeDelta::MAX`
    /// rather than panicking. The grammar happily parses `9999999999999999h`,
    /// which is far past what a `TimeDelta` holds, and `TimeDelta::hours`
    /// panics on that; a window that long means "never stale" either way.
    /// The one conversion every freshness check in `policy.rs` and
    /// `quality.rs` goes through.
    pub fn as_time_delta(&self) -> chrono::TimeDelta {
        i64::try_from(self.hours)
            .ok()
            .and_then(chrono::TimeDelta::try_hours)
            .unwrap_or(chrono::TimeDelta::MAX)
    }
}

impl std::str::FromStr for Duration {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let bad = || format!("{s:?} is not a duration like 30d, 12h, or 2w");
        if s.len() < 2 {
            return Err(bad());
        }
        let (num, unit) = s.split_at(s.len() - 1);
        let n: u64 = num.parse().map_err(|_| bad())?;
        let hours = match unit {
            "h" => Some(n),
            "d" => n.checked_mul(24),
            "w" => n.checked_mul(24 * 7),
            _ => None,
        }
        .ok_or_else(bad)?;
        Ok(Duration { hours })
    }
}

impl std::fmt::Display for Duration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.hours != 0 && self.hours.is_multiple_of(24 * 7) {
            write!(f, "{}w", self.hours / (24 * 7))
        } else if self.hours != 0 && self.hours.is_multiple_of(24) {
            write!(f, "{}d", self.hours / 24)
        } else {
            write!(f, "{}h", self.hours)
        }
    }
}

impl TryFrom<String> for Duration {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Duration> for String {
    fn from(d: Duration) -> String {
        d.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_parses_days_hours_and_weeks() {
        assert_eq!("30d".parse::<Duration>().unwrap().as_hours(), 30 * 24);
        assert_eq!("12h".parse::<Duration>().unwrap().as_hours(), 12);
        assert_eq!("2w".parse::<Duration>().unwrap().as_hours(), 2 * 24 * 7);
    }

    #[test]
    fn an_absurdly_long_duration_caps_instead_of_panicking() {
        let huge: Duration = "9999999999999999h".parse().unwrap();
        assert_eq!(huge.as_time_delta(), chrono::TimeDelta::MAX);
        assert_eq!(
            "2d".parse::<Duration>().unwrap().as_time_delta(),
            chrono::TimeDelta::hours(48)
        );
    }

    #[test]
    fn a_duration_refuses_an_unknown_unit_or_a_bare_number() {
        assert!("30m".parse::<Duration>().is_err());
        assert!("30".parse::<Duration>().is_err());
        assert!("abc".parse::<Duration>().is_err());
    }

    /// Pins `Display`'s own rollover rules -- `36h` has no exact day or week
    /// count so it stays hours, `168h` is exactly one week -- since these
    /// are exactly what a catalogue's `max_age` round-trips through the wire
    /// as (see `factory-core`'s own round-trip lock on
    /// `PolicyControlDetail.max_age`).
    #[test]
    fn display_prefers_the_largest_exact_unit() {
        assert_eq!(Duration::from_hours(30 * 24).to_string(), "30d");
        assert_eq!(Duration::from_hours(12).to_string(), "12h");
        assert_eq!(Duration::from_hours(2 * 24 * 7).to_string(), "2w");
        assert_eq!(
            Duration::from_hours(36).to_string(),
            "36h",
            "no exact day or week count"
        );
        assert_eq!(
            Duration::from_hours(168).to_string(),
            "1w",
            "24*7 hours is exactly one week"
        );
    }

    /// The wire form (e.g. `PolicyControlDetail.max_age`) is `Option<Duration>`
    /// serialized through `#[serde(try_from = "String", into = "String")]` --
    /// this pins that exact JSON shape, byte for byte, independent of which
    /// crate the type lives in.
    #[test]
    fn a_duration_round_trips_through_json_byte_identically() {
        for (text, hours) in [("30d", 30 * 24), ("12h", 12), ("2w", 2 * 24 * 7)] {
            let d: Duration = text.parse().unwrap();
            let json = serde_json::to_string(&d).unwrap();
            assert_eq!(
                json,
                format!("{text:?}"),
                "serialized form must stay byte-identical"
            );
            let back: Duration = serde_json::from_str(&json).unwrap();
            assert_eq!(back.as_hours(), hours);
        }
    }
}
