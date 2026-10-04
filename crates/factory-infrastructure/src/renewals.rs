//! #236: strict metadata declarations and pure important-date precedence/alerts.
use chrono::{DateTime, Duration, NaiveDate, Utc};
pub use factory_kernel::renewals::*;
use factory_kernel::{FactoryError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const DEFAULT_LEAD_SECONDS: u64 = 30 * 86400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenewalsNotify {
    /// Owner-authored standing opt-in. Safe alert metadata is supplied on stdin.
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<crate::environments::Span>,
}

pub fn parse_expiry(text: &str) -> std::result::Result<Option<DateTime<Utc>>, String> {
    if text == "never" {
        return Ok(None);
    }
    if let Ok(date) = DateTime::parse_from_rfc3339(text) {
        return Ok(Some(date.with_timezone(&Utc)));
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|date| Some(date.and_utc()))
        .ok_or_else(|| "expires must be RFC3339, a YYYY-MM-DD UTC date or never".into())
}

pub trait RenewalDeclExt {
    fn lead_seconds(&self) -> u64;
    fn expiry(&self) -> Option<DateTime<Utc>>;
    fn no_expiry(&self) -> bool;
}
impl RenewalDeclExt for RenewalDecl {
    fn lead_seconds(&self) -> u64 {
        self.lead
            .as_ref()
            .map(|lead| lead.as_hours().saturating_mul(3600))
            .unwrap_or(DEFAULT_LEAD_SECONDS)
    }
    fn expiry(&self) -> Option<DateTime<Utc>> {
        self.expires
            .as_deref()
            .and_then(|date| parse_expiry(date).ok().flatten())
    }
    fn no_expiry(&self) -> bool {
        self.expires.as_deref() == Some("never")
    }
}

pub fn validate(declarations: &[RenewalDecl], notify: Option<&RenewalsNotify>) -> Result<()> {
    let bad = |message: &str| FactoryError::BadRequest(message.into());
    let mut names = BTreeSet::new();
    for decl in declarations {
        if decl.name.trim().is_empty() || decl.name.len() > 200 || !names.insert(&decl.name) {
            return Err(bad("renewals need unique nonempty names of at most 200 bytes within their declaration scope"));
        }
        if let Some(date) = &decl.expires {
            parse_expiry(date).map_err(|message| bad(&message))?;
        }
        if decl.lead_seconds() == 0 || decl.lead_seconds() > 10 * 365 * 86400 {
            return Err(bad("renewals.lead must be positive and at most ten years"));
        }
        if decl.owner.trim().is_empty()
            || decl
                .observe
                .as_ref()
                .is_some_and(|id| id.trim().is_empty() || id.len() > 500)
        {
            return Err(bad(
                "renewals need an owner and a nonempty bounded observe identity when named",
            ));
        }
        if decl.renew.len() > 4000
            || decl.affects.len() > 100
            || decl
                .affects
                .iter()
                .any(|item| item.trim().is_empty() || item.len() > 500)
        {
            return Err(bad(
                "renewal metadata exceeds its bounded text or dependency limit",
            ));
        }
    }
    if let Some(notify) = notify {
        if notify.command.trim().is_empty()
            || notify.command.len() > 4000
            || notify
                .timeout
                .as_ref()
                .is_some_and(|timeout| timeout.seconds() > 60)
        {
            return Err(bad(
                "renewals_notify needs a command and a timeout between 1s and 60s",
            ));
        }
    }
    Ok(())
}

/// Exact dates observed from the thing outrank declarations. A person's date
/// outranks an approximate metadata-derived lifetime; retain both explanations.
pub fn entry(
    mut observation: ExpiryObservation,
    declaration: Option<&RenewalDecl>,
    now: DateTime<Utc>,
    runs: &[ScheduledRunDate],
) -> ImportantDateEntry {
    let declared = declaration.and_then(RenewalDecl::expiry);
    let declared_never = declaration.is_some_and(RenewalDecl::no_expiry);
    let has_declaration = declaration.is_some_and(|decl| decl.expires.is_some());
    let authoritative_observation = observation.basis == DateBasis::Observed
        && (observation.expires_at.is_some() || observation.no_expiry);
    let conflict = authoritative_observation
        && has_declaration
        && (observation.expires_at != declared || observation.no_expiry != declared_never);
    if let Some(declaration) = declaration {
        observation.name = declaration.name.clone();
        observation.kind = declaration.kind;
        observation.lead_seconds = declaration.lead_seconds();
        if !declaration.renew.is_empty() {
            observation.renew = declaration.renew.clone();
        }
        observation.owner = declaration.owner.clone();
        if !authoritative_observation && has_declaration {
            observation.expires_at = declared;
            observation.no_expiry = declared_never;
            observation.basis = DateBasis::Declared;
        }
    }
    let stale = observation.issue.is_some() && observation.basis != DateBasis::Declared;
    let state = if stale {
        DateState::Unknown
    } else if observation.no_expiry {
        DateState::Ok
    } else if let Some(expires) = observation.expires_at {
        if expires <= now {
            DateState::Overdue
        } else if expires - now <= Duration::seconds(observation.lead_seconds as i64) {
            DateState::DueSoon
        } else {
            DateState::Ok
        }
    } else {
        DateState::Unknown
    };
    let scheduled_risks: Vec<_> = runs
        .iter()
        .filter(|run| {
            observation
                .expires_at
                .is_some_and(|expiry| expiry <= run.next_run_at)
                && observation.affects.iter().any(|dependency| {
                    dependency.scope.as_deref() == Some(run.scope.as_str())
                        && (dependency.agent.is_some()
                            || (dependency.environment.is_none() && dependency.provider.is_none()))
                        && dependency
                            .agent
                            .as_deref()
                            .is_none_or(|agent| agent == run.agent)
                })
        })
        .cloned()
        .collect();
    let native = matches!(
        observation.source,
        DateSource::PolicyAttestation | DateSource::CraDeadline
    );
    let milestone = if native {
        None
    } else {
        observation.expires_at.and_then(|expires| {
            let remaining = expires - now;
            if remaining <= Duration::zero() {
                Some(RenewalMilestone::Expired)
            } else if remaining <= Duration::days(1) {
                Some(RenewalMilestone::OneDay)
            } else if remaining <= Duration::days(7) {
                Some(RenewalMilestone::SevenDays)
            } else if remaining <= Duration::seconds(observation.lead_seconds as i64) {
                Some(RenewalMilestone::Lead)
            } else if !scheduled_risks.is_empty() {
                Some(RenewalMilestone::ScheduledRun)
            } else {
                None
            }
        })
    };
    ImportantDateEntry {
        observation,
        declared_expires_at: declared,
        declared_no_expiry: declared_never,
        conflict,
        state,
        milestone,
        scheduled_risks,
        href: "#all/dates".into(),
        resolved: false,
    }
}

pub fn report(
    mut entries: Vec<ImportantDateEntry>,
    now: DateTime<Utc>,
    observation_issues: Vec<String>,
) -> ImportantDatesReport {
    entries.sort_by(|a, b| {
        a.observation
            .expires_at
            .is_none()
            .cmp(&b.observation.expires_at.is_none())
            .then_with(|| a.observation.expires_at.cmp(&b.observation.expires_at))
            .then_with(|| a.observation.id.cmp(&b.observation.id))
    });
    let due_soon = entries
        .iter()
        .filter(|entry| !entry.resolved && entry.state == DateState::DueSoon)
        .count();
    let overdue = entries
        .iter()
        .filter(|entry| !entry.resolved && entry.state == DateState::Overdue)
        .count();
    let unknown = entries
        .iter()
        .filter(|entry| !entry.resolved && entry.state == DateState::Unknown)
        .count();
    let next_expiry = entries
        .iter()
        .filter(|entry| !entry.resolved)
        .filter_map(|entry| entry.observation.expires_at)
        .filter(|expires| *expires > now)
        .min();
    ImportantDatesReport {
        at: now,
        entries,
        next_expiry,
        due_soon,
        overdue,
        unknown,
        observation_issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn now() -> DateTime<Utc> {
        "2026-10-04T00:00:00Z".parse().unwrap()
    }
    fn observed(days: i64) -> ExpiryObservation {
        ExpiryObservation {
            id: "github-token".into(),
            name: "GitHub token".into(),
            kind: DateKind::Credential,
            scope: None,
            expires_at: Some(now() + Duration::days(days)),
            no_expiry: false,
            basis: DateBasis::Observed,
            source: DateSource::GithubAuth,
            detail: "expiry response header".into(),
            observed_at: Some(now()),
            attempted_at: now(),
            issue: None,
            affects: vec![DateDependency {
                scope: Some("demo".into()),
                agent: Some("curator".into()),
                environment: None,
                provider: Some("github".into()),
                label: "demo/curator".into(),
            }],
            lead_seconds: DEFAULT_LEAD_SECONDS,
            renew: "renew in GitHub".into(),
            owner: "owner".into(),
        }
    }
    #[test]
    fn dates_are_strict_utc_and_never_is_explicit_not_unknown() {
        assert_eq!(parse_expiry("2026-10-04").unwrap(), Some(now()));
        assert_eq!(
            parse_expiry("2026-10-04T01:00:00+01:00").unwrap(),
            Some(now())
        );
        assert_eq!(parse_expiry("never").unwrap(), None);
        assert!(parse_expiry("next week").is_err());
        let mut item = observed(2);
        item.expires_at = None;
        assert_eq!(
            entry(item.clone(), None, now(), &[]).state,
            DateState::Unknown
        );
        item.no_expiry = true;
        assert_eq!(entry(item, None, now(), &[]).state, DateState::Ok);
    }
    #[test]
    fn observed_wins_conflicts_but_an_exact_declaration_beats_an_estimate() {
        let decl: RenewalDecl =
            serde_yaml_ng::from_str("name: github-token\nkind: credential\nexpires: 2027-01-01\n")
                .unwrap();
        let item = entry(observed(-1), Some(&decl), now(), &[]);
        assert!(item.conflict);
        assert_eq!(item.state, DateState::Overdue);
        assert_eq!(item.declared_expires_at, decl.expiry());
        let mut derived = observed(-1);
        derived.basis = DateBasis::Derived;
        let item = entry(derived, Some(&decl), now(), &[]);
        assert_eq!(item.state, DateState::Ok);
        assert_eq!(item.observation.basis, DateBasis::Declared);
    }
    #[test]
    fn milestones_choose_one_current_item_not_every_missed_threshold() {
        for (days, milestone) in [
            (40, None),
            (30, Some(RenewalMilestone::Lead)),
            (7, Some(RenewalMilestone::SevenDays)),
            (1, Some(RenewalMilestone::OneDay)),
            (0, Some(RenewalMilestone::Expired)),
            (-10, Some(RenewalMilestone::Expired)),
        ] {
            assert_eq!(entry(observed(days), None, now(), &[]).milestone, milestone);
        }
        let mut native = observed(-1);
        native.source = DateSource::CraDeadline;
        assert_eq!(
            entry(native, None, now(), &[]).milestone,
            None,
            "native clock owns its Inbox logic"
        );
    }
    #[test]
    fn scheduled_risk_is_immediate_and_agent_specific_even_before_the_lead_window() {
        let run = ScheduledRunDate {
            task: "t".into(),
            title: "future curator".into(),
            scope: "demo".into(),
            agent: "curator".into(),
            next_run_at: now() + Duration::days(90),
        };
        let item = entry(observed(60), None, now(), &[run.clone()]);
        assert_eq!(item.state, DateState::Ok);
        assert_eq!(item.milestone, Some(RenewalMilestone::ScheduledRun));
        assert_eq!(item.scheduled_risks, vec![run.clone()]);
        let mut unrelated = run;
        unrelated.agent = "other".into();
        assert!(entry(observed(60), None, now(), &[unrelated])
            .scheduled_risks
            .is_empty());
    }
    #[test]
    fn an_observation_failure_is_unknown_and_never_erases_a_last_known_warning() {
        let mut failed = observed(-1);
        failed.issue = Some("observation unavailable".into());
        let item = entry(failed, None, now(), &[]);
        assert_eq!(item.state, DateState::Unknown);
        assert_eq!(item.milestone, Some(RenewalMilestone::Expired));
    }
    #[test]
    fn declarations_reject_credentials_and_duplicate_names() {
        assert!(serde_yaml_ng::from_str::<RenewalDecl>(
            "name: token\nkind: credential\ncredential: secret\n"
        )
        .is_err());
        let decl: RenewalDecl = serde_yaml_ng::from_str(
            "name: token\nkind: credential\nexpires: 2027-01-01\nlead: 30d\n",
        )
        .unwrap();
        validate(&[decl.clone()], None).unwrap();
        assert_eq!(decl.lead_seconds(), 30 * 86400);
        assert!(validate(&[decl.clone(), decl], None).is_err());
    }
}
