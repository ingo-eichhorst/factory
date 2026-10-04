//! L5 budget-check evaluation. Authored catalogues and intent stay in L6.
use chrono::{DateTime, Datelike, TimeZone, Utc};
use factory_kernel::{CostReport, CostRow};
use serde::{Deserialize, Serialize};

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
    use factory_kernel::CostGroupBy;
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
}
