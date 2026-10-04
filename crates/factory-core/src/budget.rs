//! L6 authored budget intent and pure monthly assessments (#164).
//! Spend is an L4 fact; neither this catalogue nor its views invent prices
//! or derive a limit from a provider plan. No aggregate store is maintained.
use crate::usage::{CostGroupBy, CostReport, CostRow};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MAX_CATALOGUE_BYTES: u64 = 1024 * 1024;

pub fn default_group_by() -> CostGroupBy {
    CostGroupBy::Scope
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonthlyLimit {
    pub monthly_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalogue {
    pub version: u32,
    #[serde(default, deserialize_with = "unique_scopes")]
    pub scopes: BTreeMap<String, MonthlyLimit>,
}

impl Default for Catalogue {
    fn default() -> Self {
        Self {
            version: 1,
            scopes: BTreeMap::new(),
        }
    }
}

fn unique_scopes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, MonthlyLimit>, D::Error> {
    struct Unique;
    impl<'de> serde::de::Visitor<'de> for Unique {
        type Value = BTreeMap<String, MonthlyLimit>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("unique scope ids mapped to monthly USD limits")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut out = BTreeMap::new();
            while let Some((id, limit)) = map.next_entry::<String, MonthlyLimit>()? {
                if out.insert(id.clone(), limit).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate budget scope id {id:?}"
                    )));
                }
            }
            Ok(out)
        }
    }
    d.deserialize_map(Unique)
}

impl Catalogue {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let value: Self = serde_yaml_ng::from_str(raw).map_err(|e| e.to_string())?;
        if value.version != 1 {
            return Err(format!(
                "unsupported budget catalogue version {}",
                value.version
            ));
        }
        for (id, limit) in &value.scopes {
            if id.is_empty() || id.trim() != id {
                return Err("budget scope ids must be nonempty and unpadded".into());
            }
            if !limit.monthly_usd.is_finite() || limit.monthly_usd < 0.0 {
                return Err(format!(
                    "scope {id:?}: monthly_usd must be finite and nonnegative"
                ));
            }
        }
        Ok(value)
    }
}

pub fn budgets_dir(root: &Path) -> PathBuf {
    root.join(".factory/budgets")
}
pub fn catalogue_path(root: &Path) -> PathBuf {
    budgets_dir(root).join("limits.yaml")
}

/// Re-read authored intent, never create it. Missing means no limit;
/// malformed, oversized, symlink or special-file intent is never ignored.
pub fn load(root: &Path) -> Result<Catalogue, String> {
    use std::io::Read;
    let path = catalogue_path(root);
    let metadata = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Catalogue::default()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
        Ok(metadata) => metadata,
    };
    if !metadata.is_file() || metadata.len() > MAX_CATALOGUE_BYTES {
        return Err(format!(
            "{}: expected a regular catalogue file of at most 1 MiB",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|file| file.take(MAX_CATALOGUE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_CATALOGUE_BYTES {
        return Err(format!("{}: catalogue exceeds 1 MiB", path.display()));
    }
    let raw = std::str::from_utf8(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Catalogue::parse(raw).map_err(|e| format!("{}: {e}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Month {
    pub from: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub as_of: DateTime<Utc>,
}

impl Month {
    pub fn at(now: DateTime<Utc>) -> Result<Self, String> {
        let from = Utc
            .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
            .single()
            .ok_or("month start is outside the supported date range")?;
        let (year, next) = if now.month() == 12 {
            (now.year().checked_add(1).ok_or("year overflow")?, 1)
        } else {
            (now.year(), now.month() + 1)
        };
        let until = Utc
            .with_ymd_and_hms(year, next, 1, 0, 0, 0)
            .single()
            .ok_or("month end is outside the supported date range")?;
        Ok(Self {
            from,
            until,
            as_of: now,
        })
    }
    pub fn elapsed_fraction(&self) -> f64 {
        let elapsed = (self.as_of - self.from).num_milliseconds().max(0) as f64;
        let total = (self.until - self.from).num_milliseconds() as f64;
        if total <= 0.0 {
            return 0.0;
        }
        (elapsed / total).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Within,
    Over,
    Unknown,
    Unconfigured,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assessment {
    pub state: State,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_fraction: Option<f64>,
    /// A disclosed linear pace estimate, never a repricing or safe verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_month_usd: Option<f64>,
}

pub fn assess(
    limit: Option<f64>,
    spend: &CostRow,
    unattributed_runs: u32,
    month: &Month,
) -> Assessment {
    let mut out = Assessment {
        state: State::Unknown,
        reason: String::new(),
        remaining_usd: None,
        used_fraction: None,
        projected_month_usd: None,
    };
    let Some(limit) = limit else {
        out.state = State::Unconfigured;
        out.reason =
            "No authored monthly USD limit; provider plan allowances are not budgets".into();
        return out;
    };
    if !limit.is_finite() || limit < 0.0 || !spend.cost_usd.is_finite() || spend.cost_usd < 0.0 {
        out.reason = "The limit or recorded cost is not a finite nonnegative USD amount".into();
        return out;
    }
    let missing = spend.runs_unknown.saturating_add(spend.runs_cost_unknown);
    let incomplete = missing > 0 || spend.runs_partial > 0 || unattributed_runs > 0;
    if spend.cost_usd > limit {
        out.state = State::Over;
        out.reason = format!(
            "${:.2} known exceeds the ${limit:.2} monthly limit{}",
            spend.cost_usd,
            if incomplete {
                "; additional spend is unknown"
            } else {
                ""
            }
        );
    } else if incomplete {
        out.reason = format!("${:.2} known; {missing} unmeasured, {} partial and {unattributed_runs} unattributed runs prevent a safe under-budget verdict", spend.cost_usd, spend.runs_partial);
    } else {
        out.state = State::Within;
        out.reason = format!(
            "${:.2} measured within the ${limit:.2} monthly limit",
            spend.cost_usd
        );
    }
    if !incomplete {
        out.remaining_usd = Some(limit - spend.cost_usd);
        out.used_fraction = if limit > 0.0 {
            Some(spend.cost_usd / limit)
        } else if spend.cost_usd == 0.0 {
            Some(0.0)
        } else {
            None
        };
        let elapsed = month.elapsed_fraction();
        if elapsed > 0.0 {
            let projected = spend.cost_usd / elapsed;
            if projected.is_finite() {
                out.projected_month_usd = Some(projected);
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeBudget {
    pub id: String,
    pub scope: String,
    pub path: String,
    /// Parent cards can include spending outside the selected subtree.
    pub relation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_usd: Option<f64>,
    pub spent: CostRow,
    pub unattributed_runs: u32,
    pub daily: Vec<factory_kernel::DailySpend>,
    pub assessment: Assessment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub catalogue: String,
    pub month: Month,
    pub group_by: CostGroupBy,
    /// Exactly the selected scope's spend; never a sum of overlapping caps.
    pub spend: CostReport,
    pub budgets: Vec<ScopeBudget>,
    pub findings: Vec<String>,
}

/// Authored intent passed down as configuration alongside unchanged L4
/// spend facts. This is not a new L6-produced live fact or status read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub catalogue: Option<Catalogue>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyCap {
    pub scope: String,
    pub monthly_usd: f64,
    pub spend: CostReport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyInput {
    pub month: Month,
    /// Selected scope and configured ancestors only. Each parent cap's
    /// spend covers its full subtree, never just the selected child.
    pub caps: Vec<PolicyCap>,
    pub error: Option<String>,
}

/// None is an unknown fact, not false or a safe under-budget verdict.
/// Policy's established Open control state carries that explicit reason.
pub fn within(input: Option<&PolicyInput>, now: DateTime<Utc>) -> (Option<bool>, String) {
    let Some(input) = input else {
        return (
            None,
            "budget_within: unknown — spend and authored limits were not resolved".into(),
        );
    };
    if let Some(error) = &input.error {
        return (None, format!("budget_within: unknown — {error}"));
    }
    if input.caps.is_empty() {
        return (
            None,
            "budget_within: unknown — no applicable authored monthly USD limit".into(),
        );
    }
    let current = match Month::at(now) {
        Ok(month) => month,
        Err(error) => return (None, format!("budget_within: unknown — {error}")),
    };
    if input.month.from != current.from
        || input.month.until != current.until
        || input.month.as_of != now
    {
        return (
            None,
            "budget_within: unknown — spend is not from the current UTC month".into(),
        );
    }
    let mut unknown = Vec::new();
    let mut over = Vec::new();
    for cap in &input.caps {
        let report = &cap.spend;
        if report.basis != factory_kernel::SpendBasis::Started
            || report.from != input.month.from
            || report.to != input.month.as_of
            || report.scope.as_deref() != Some(&cap.scope)
        {
            unknown.push(format!(
                "{}: spend window/scope does not match its authored monthly cap",
                cap.scope
            ));
            continue;
        }
        let assessment = assess(
            Some(cap.monthly_usd),
            &report.total,
            report.unattributed_runs,
            &input.month,
        );
        let reason = format!("{}: {}", cap.scope, assessment.reason);
        match assessment.state {
            State::Over => over.push(reason),
            State::Unknown | State::Unconfigured => unknown.push(reason),
            State::Within => {}
        }
    }
    if !over.is_empty() {
        return (
            Some(false),
            format!("budget_within: over — {}", over.join("; ")),
        );
    }
    if !unknown.is_empty() {
        return (
            None,
            format!("budget_within: unknown — {}", unknown.join("; ")),
        );
    }
    (
        Some(true),
        format!(
            "budget_within: {} applicable authored cap(s) measured within their monthly limits",
            input.caps.len()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy_input() -> PolicyInput {
        let month = Month::at(now("2026-10-16T12:00:00Z")).unwrap();
        let cap = |scope: &str, spent: f64, limit: f64| PolicyCap {
            scope: scope.into(),
            monthly_usd: limit,
            spend: CostReport {
                basis: Default::default(),
                finished: None,
                group_by: CostGroupBy::Scope,
                from: month.from,
                to: month.as_of,
                scope: Some(scope.into()),
                rows: Vec::new(),
                total: CostRow {
                    runs: 1,
                    cost_usd: spent,
                    ..Default::default()
                },
                unattributed_runs: 0,
                daily: Vec::new(),
            },
        };
        PolicyInput {
            caps: vec![cap("parent", 30.0, 50.0), cap("child", 10.0, 20.0)],
            month,
            error: None,
        }
    }

    #[test]
    fn policy_caps_are_independent_and_known_overrides_unknown_not_the_reverse() {
        let mut input = policy_input();
        let now = input.month.as_of;
        assert_eq!(within(Some(&input), now).0, Some(true));
        input.caps[0].spend.total.runs_unknown = 1;
        assert_eq!(within(Some(&input), now).0, None);
        input.caps[1].monthly_usd = 5.0;
        assert_eq!(within(Some(&input), now).0, Some(false));
        input.caps[0].spend.total.runs_unknown = 0;
        input.caps[1].monthly_usd = 20.0;
        input.caps[0].monthly_usd = 25.0;
        assert_eq!(
            within(Some(&input), now).0,
            Some(false),
            "parent's full subtree still counts"
        );
    }

    #[test]
    fn policy_budget_absence_bad_config_wrong_scope_window_and_incomplete_spend_are_unknown() {
        let baseline = policy_input();
        let now = baseline.month.as_of;
        assert_eq!(within(None, now).0, None);
        for mutation in 0..8 {
            let mut input = baseline.clone();
            match mutation {
                0 => input.caps.clear(),
                1 => input.error = Some("malformed authored catalogue".into()),
                2 => input.caps[0].spend.scope = Some("different".into()),
                3 => input.caps[0].spend.from = now,
                4 => input.caps[0].spend.basis = factory_kernel::SpendBasis::Finished,
                5 => input.caps[0].spend.total.runs_partial = 1,
                6 => input.caps[0].spend.total.runs_cost_unknown = 1,
                _ => input.caps[0].spend.unattributed_runs = 1,
            }
            let (value, reason) = within(Some(&input), now);
            assert_eq!(value, None, "mutation {mutation}");
            assert!(reason.contains("unknown"));
        }
        assert_eq!(
            within(Some(&baseline), now + chrono::Duration::days(32)).0,
            None
        );
        assert_eq!(
            within(Some(&baseline), now - chrono::Duration::seconds(1)).0,
            None
        );
        assert_eq!(
            within(Some(&baseline), now + chrono::Duration::seconds(1)).0,
            None
        );
    }
    fn now(raw: &str) -> DateTime<Utc> {
        raw.parse().unwrap()
    }
    #[test]
    fn authored_limits_are_strict_finite_and_unique_with_no_inferred_default() {
        assert!(Catalogue::default().scopes.is_empty());
        let c = Catalogue::parse("version: 1\nscopes:\n  stable-id: {monthly_usd: 50}\n").unwrap();
        assert_eq!(c.scopes["stable-id"].monthly_usd, 50.0);
        assert_eq!(
            Catalogue::parse(&serde_yaml_ng::to_string(&c).unwrap()).unwrap(),
            c
        );
        for raw in [
            "version: 2",
            "version: 1\nunknown: 1",
            "version: 1\nscopes: {s: {monthly_usd: -1}}",
            "version: 1\nscopes: {s: {monthly_usd: .nan}}",
            "version: 1\nscopes: {s: {monthly_usd: .inf}}",
            "version: 1\nscopes: {s: {monthly_usd: 1, guessed: 2}}",
            "version: 1\nscopes:\n  s: {monthly_usd: 1}\n  s: {monthly_usd: 2}",
        ] {
            assert!(Catalogue::parse(raw).is_err(), "{raw}");
        }
        assert!(Catalogue::parse("version: 1\nscopes: {s: {monthly_usd: 0}}").is_ok());
    }
    #[test]
    fn month_boundaries_are_utc_calendar_months_including_leap_years() {
        let month = Month::at(now("2024-02-15T00:00:00Z")).unwrap();
        assert_eq!(month.from, now("2024-02-01T00:00:00Z"));
        assert_eq!(month.until, now("2024-03-01T00:00:00Z"));
        assert!((month.elapsed_fraction() - 14.0 / 29.0).abs() < 1e-12);
        assert_eq!(
            Month::at(now("2026-12-31T23:59:59Z")).unwrap().until,
            now("2027-01-01T00:00:00Z")
        );
        assert_eq!(
            Month::at(now("2026-10-01T00:00:00Z"))
                .unwrap()
                .elapsed_fraction(),
            0.0
        );
    }
    #[test]
    fn unknown_partial_and_unattributed_spend_is_never_safe_or_free() {
        let month = Month::at(now("2026-10-16T12:00:00Z")).unwrap();
        let row = CostRow {
            cost_usd: 25.0,
            runs: 1,
            ..Default::default()
        };
        let known = assess(Some(50.0), &row, 0, &month);
        assert_eq!(known.state, State::Within);
        assert_eq!(known.remaining_usd, Some(25.0));
        assert_eq!(known.projected_month_usd, Some(50.0));
        assert_eq!(assess(None, &row, 0, &month).state, State::Unconfigured);
        for (unknown, partial, cost_unknown, unattributed) in
            [(1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1)]
        {
            let row = CostRow {
                runs_unknown: unknown,
                runs_partial: partial,
                runs_cost_unknown: cost_unknown,
                ..row.clone()
            };
            let uncertain = assess(Some(50.0), &row, unattributed, &month);
            assert_eq!(uncertain.state, State::Unknown);
            assert!(uncertain.remaining_usd.is_none() && uncertain.projected_month_usd.is_none());
            assert_eq!(
                assess(Some(10.0), &row, unattributed, &month).state,
                State::Over,
                "known overrun is decisive"
            );
        }
        assert_eq!(
            assess(Some(0.0), &CostRow::default(), 0, &month).state,
            State::Within
        );
        assert!(assess(
            Some(50.0),
            &CostRow {
                cost_usd: f64::NAN,
                ..Default::default()
            },
            0,
            &month
        )
        .remaining_usd
        .is_none());
    }
    #[test]
    fn file_reads_do_not_create_intent_and_bad_intent_is_not_treated_as_absent() {
        let root = std::env::temp_dir().join(format!("factory-budget-{}", uuid::Uuid::new_v4()));
        assert!(load(&root).unwrap().scopes.is_empty());
        assert!(!root.exists());
        std::fs::create_dir_all(budgets_dir(&root)).unwrap();
        std::fs::write(
            catalogue_path(&root),
            "version: 1\nscopes: {s: {monthly_usd: -1}}",
        )
        .unwrap();
        let error = load(&root).unwrap_err();
        assert!(error.contains("limits.yaml") && error.contains("nonnegative"));
        std::fs::write(
            catalogue_path(&root),
            vec![b' '; MAX_CATALOGUE_BYTES as usize + 1],
        )
        .unwrap();
        assert!(load(&root).unwrap_err().contains("1 MiB"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
