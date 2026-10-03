//! The CRA Art. 14 reporting clock (`#157`, phase 1): the 24-hour early
//! warning and 72-hour notification deadlines only -- the 14-day final
//! report waits on a source for the corrective-measure time, and stays out
//! of scope here.
//!
//! Pure, like `policy.rs`: `compute` takes the L2 exploited findings, the L4
//! confirmed security reports, and the attestation store's rows, and folds
//! them into a clock with no status table of its own (ADR 0004) -- the same
//! reasoning `policy::evaluate` is built on. `factory-daemon/src/policies/
//! clock.rs` is the only caller, and it never reads these inputs for any
//! other reason.
//!
//! A clock **item** is either an exploited finding or a confirmed security
//! report ([`ClockItemRef`]). A submission against one is recorded as an
//! ordinary [`crate::policy::Attestation`] carrying a [`ClockMark`] --
//! `policy.attest`'s existing store and grant, not a new one.

use crate::dependencies::ExploitedFinding;
use crate::intake::ConfirmedSecurityReport;
use crate::policy::{Attestation, ControlRef};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The one control a clock submission is ever recorded against in phase 1.
/// A later phase's 14-day final report would add a third deadline to this
/// same control, not a second one.
pub fn art_14() -> ControlRef {
    ControlRef::new("cra", "art-14")
}

/// One thing the clock counts: an exploited L2 finding or a confirmed L4
/// security report. The CLI's text form (`factory policy attest
/// --clock-item`) is `finding:<scope>:<vulnerability>` -- parsed by finding
/// the *last* `:` in `<scope>:<vulnerability>`, so everything after it is
/// the vulnerability id and everything before is the scope -- or
/// `report:<task-id>`; see [`std::str::FromStr`] below. Every vulnerability
/// id this clock actually sees is a bare CVE/GHSA/EUVD id with no `:` of its
/// own, so this only ever matters for the ordinary, single-colon case; a
/// hypothetical colon-bearing id (a raw CPE URI, never how CycloneDX names a
/// vulnerability) would parse with everything but its last segment folded
/// into `scope`. The wire form (`ClockMark.item`) is this tagged enum, not
/// that string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClockItemRef {
    Finding { scope: String, vulnerability: String },
    Report { item: String },
}

impl std::fmt::Display for ClockItemRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClockItemRef::Finding { scope, vulnerability } => {
                write!(f, "finding:{scope}:{vulnerability}")
            }
            ClockItemRef::Report { item } => write!(f, "report:{item}"),
        }
    }
}

impl std::str::FromStr for ClockItemRef {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let bad = || format!("{s:?} is not finding:<scope>:<vulnerability> or report:<task-id>");
        let (kind, rest) = s.split_once(':').ok_or_else(bad)?;
        match kind {
            "finding" => {
                let (scope, vulnerability) = rest.rsplit_once(':').ok_or_else(bad)?;
                if scope.is_empty() || vulnerability.is_empty() {
                    return Err(bad());
                }
                Ok(ClockItemRef::Finding {
                    scope: scope.to_string(),
                    vulnerability: vulnerability.to_string(),
                })
            }
            "report" => {
                if rest.is_empty() {
                    return Err(bad());
                }
                Ok(ClockItemRef::Report { item: rest.to_string() })
            }
            _ => Err(bad()),
        }
    }
}

/// Which of phase 1's two deadlines a submission counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockDeadlineKind {
    EarlyWarning,
    Notification,
}

impl ClockDeadlineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EarlyWarning => "early_warning",
            Self::Notification => "notification",
        }
    }

    /// This deadline's offset from awareness: 24h for the early warning,
    /// 72h for the notification (Art. 14(2)(a)/(b)). Phase 1 stops there --
    /// the 14-day final report has no source yet for the corrective-measure
    /// time it would run from.
    fn offset(self) -> ChronoDuration {
        match self {
            Self::EarlyWarning => ChronoDuration::hours(24),
            Self::Notification => ChronoDuration::hours(72),
        }
    }
}

impl std::fmt::Display for ClockDeadlineKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ClockDeadlineKind {
    type Err = String;

    /// Accepts both the wire spelling (`early_warning`) and the CLI's own
    /// hyphenated one (`early-warning`) -- a deliberate looseness on the one
    /// argument a person types by hand, not extended to anything else here.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.replace('-', "_").as_str() {
            "early_warning" => Ok(Self::EarlyWarning),
            "notification" => Ok(Self::Notification),
            _ => Err(format!("{s:?} is not early-warning or notification")),
        }
    }
}

/// What a submission against the clock records, carried on an ordinary
/// [`Attestation`] (`Attestation.clock`) rather than a status table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockMark {
    pub item: ClockItemRef,
    pub deadline: ClockDeadlineKind,
}

/// One deadline's state, at the `now` `compute` was asked about. `met`/`late`
/// both mean a live (unwithdrawn) submission exists; the clock ignores
/// `expires_at` entirely -- only a withdrawal ever un-meets a deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockDeadlineState {
    Due,
    Overdue,
    Met,
    Late,
}

impl ClockDeadlineState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Due => "due",
            Self::Overdue => "overdue",
            Self::Met => "met",
            Self::Late => "late",
        }
    }
}

impl std::fmt::Display for ClockDeadlineState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The earliest live submission against one item's one deadline, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockSubmission {
    pub attestation: String,
    pub at: DateTime<Utc>,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockDeadline {
    pub deadline: ClockDeadlineKind,
    pub due_at: DateTime<Utc>,
    pub state: ClockDeadlineState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission: Option<ClockSubmission>,
}

/// One item's whole clock: its awareness time, whether it is excluded (a
/// finding only), whether the newest evidence still reports it, and its
/// deadlines -- empty for an excluded item, since CRA has nothing left to
/// report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockItem {
    pub item: ClockItemRef,
    pub scope: String,
    pub awareness_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded: Option<String>,
    #[serde(default)]
    pub reported_now: bool,
    #[serde(default)]
    pub deadlines: Vec<ClockDeadline>,
}

/// `Request::PolicyClock`'s answer: every item over the asked subtree,
/// computed fresh at `now` -- no status table (ADR 0004).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportingClock {
    pub now: DateTime<Utc>,
    pub items: Vec<ClockItem>,
}

/// The pure core: every exploited finding becomes one item; every confirmed
/// report is folded to its split chain's root and becomes one item too, at
/// the root's own `awareness_at`. Deterministic order: awareness time, then
/// the item's own identity.
pub fn compute(
    findings: &[ExploitedFinding],
    reports: &[ConfirmedSecurityReport],
    attestations: &[Attestation],
    now: DateTime<Utc>,
) -> ReportingClock {
    let mut items = Vec::with_capacity(findings.len() + reports.len());

    for finding in findings {
        let item = ClockItemRef::Finding {
            scope: finding.scope.clone(),
            vulnerability: finding.vulnerability.clone(),
        };
        let excluded = finding.latest_vex.clone();
        let deadlines = if excluded.is_some() {
            Vec::new()
        } else {
            deadlines_for(&item, finding.first_seen_at, attestations, now)
        };
        items.push(ClockItem {
            item,
            scope: finding.scope.clone(),
            awareness_at: finding.first_seen_at,
            excluded,
            reported_now: finding.reported_now,
            deadlines,
        });
    }

    let by_item: BTreeMap<&str, &ConfirmedSecurityReport> =
        reports.iter().map(|r| (r.item.as_str(), r)).collect();
    let mut roots: BTreeMap<&str, &ConfirmedSecurityReport> = BTreeMap::new();
    for report in reports {
        let root = resolve_root(report, &by_item);
        roots.entry(root.item.as_str()).or_insert(root);
    }
    for root in roots.values() {
        let item = ClockItemRef::Report { item: root.item.clone() };
        let deadlines = deadlines_for(&item, root.awareness_at, attestations, now);
        items.push(ClockItem {
            item,
            scope: root.scope.clone(),
            awareness_at: root.awareness_at,
            excluded: None,
            reported_now: true,
            deadlines,
        });
    }

    items.sort_by(|a, b| a.awareness_at.cmp(&b.awareness_at).then_with(|| a.item.cmp(&b.item)));
    ReportingClock { now, items }
}

/// Follow `report.parent` upward while it names another report in `reports`
/// itself -- a dangling or missing parent (out of scope, not itself
/// confirmed, or simply absent) stops the walk at whatever was reached, and
/// a visited set guards against a cycle no author should ever write by hand.
fn resolve_root<'a>(
    report: &'a ConfirmedSecurityReport,
    by_item: &BTreeMap<&str, &'a ConfirmedSecurityReport>,
) -> &'a ConfirmedSecurityReport {
    let mut current = report;
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    seen.insert(current.item.as_str());
    while let Some(parent_id) = current.parent.as_deref() {
        let Some(parent) = by_item.get(parent_id) else { break };
        if !seen.insert(parent.item.as_str()) {
            break;
        }
        current = parent;
    }
    current
}

/// Both of phase 1's deadlines for one item: `due_at` computed from
/// `awareness_at`, and `state` folded against whatever live submission
/// already exists -- `met`/`late` when one does (`now` never overrides a
/// submission once made), `overdue`/`due` against `now` when none does.
fn deadlines_for(
    item: &ClockItemRef,
    awareness_at: DateTime<Utc>,
    attestations: &[Attestation],
    now: DateTime<Utc>,
) -> Vec<ClockDeadline> {
    [ClockDeadlineKind::EarlyWarning, ClockDeadlineKind::Notification]
        .into_iter()
        .map(|kind| {
            let due_at = awareness_at + kind.offset();
            let submission = earliest_live_submission(item, kind, attestations);
            let state = match &submission {
                Some(s) if s.at <= due_at => ClockDeadlineState::Met,
                Some(_) => ClockDeadlineState::Late,
                None if now > due_at => ClockDeadlineState::Overdue,
                None => ClockDeadlineState::Due,
            };
            ClockDeadline { deadline: kind, due_at, state, submission }
        })
        .collect()
}

/// The earliest unwithdrawn attestation carrying a [`ClockMark`] for this
/// exact item and deadline -- "earliest" because no index makes `(item,
/// deadline)` unique, so a race that lands two rows still resolves to one
/// answer (`policies::mod`'s `policy_attest` refuses a second one anyway,
/// this is the read side's own defense). Expiry is never consulted: only a
/// withdrawal ever un-meets a deadline.
fn earliest_live_submission(
    item: &ClockItemRef,
    deadline: ClockDeadlineKind,
    attestations: &[Attestation],
) -> Option<ClockSubmission> {
    attestations
        .iter()
        .filter(|a| a.withdrawn.is_none())
        .filter_map(|a| {
            let mark = a.clock.as_ref()?;
            (&mark.item == item && mark.deadline == deadline).then(|| ClockSubmission {
                attestation: a.id.clone(),
                at: a.attested_at,
                by: a.attested_by.clone(),
            })
        })
        .min_by_key(|s| s.at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dependencies::LifecycleState;
    use crate::intake::IntakeSource;
    use crate::policy::Withdrawal;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn finding(scope: &str, vuln: &str, first_seen_at: DateTime<Utc>) -> ExploitedFinding {
        ExploitedFinding {
            scope: scope.into(),
            vulnerability: vuln.into(),
            components: Vec::new(),
            states: vec![LifecycleState::Built],
            first_seen_at,
            first_document: crate::dependencies::Attachment {
                id: "doc-1".into(),
                kind: crate::dependencies::AttachmentKind::Vulnerabilities,
                scope: scope.into(),
                run_id: "r1".into(),
                task_id: "t1".into(),
                attempt: 1,
                attached_at: first_seen_at,
                filename: "vulns.cdx.json".into(),
                spec_version: "1.6".into(),
                states: Vec::new(),
            },
            latest_vex: None,
            reported_now: true,
        }
    }

    fn attestation(item: ClockItemRef, deadline: ClockDeadlineKind, attested_at: DateTime<Utc>) -> Attestation {
        Attestation {
            id: uuid::Uuid::new_v4().to_string(),
            control: art_14(),
            scope: "demo".into(),
            evidence: "https://example.com/notice".into(),
            note: None,
            attested_by: "owner".into(),
            attested_at,
            expires_at: attested_at + ChronoDuration::days(30 * 365),
            withdrawn: None,
            clock: Some(ClockMark { item, deadline }),
        }
    }

    fn report(item: &str, scope: &str, awareness_at: DateTime<Utc>, parent: Option<&str>) -> ConfirmedSecurityReport {
        ConfirmedSecurityReport {
            item: item.into(),
            scope: scope.into(),
            awareness_at,
            source: IntakeSource { kind: crate::intake::SourceKind::Cli, reference: None, provider: None, relayed_by: None, repository: None, number: None, external_id: None },
            confirmed_by: "owner".into(),
            confirmed_at: awareness_at,
            parent: parent.map(str::to_string),
        }
    }

    #[test]
    fn deadlines_run_24h_and_72h_from_awareness_and_track_due_overdue_met_late() {
        let awareness = at("2026-09-24T10:01:00Z");
        let f = finding("demo", "CVE-2026-1234", awareness);

        // Nothing submitted, well before either deadline: both `due`.
        let now = at("2026-09-24T11:00:00Z");
        let clock = compute(std::slice::from_ref(&f), &[], &[], now);
        let item = &clock.items[0];
        assert_eq!(item.deadlines[0].due_at, awareness + ChronoDuration::hours(24));
        assert_eq!(item.deadlines[1].due_at, awareness + ChronoDuration::hours(72));
        assert_eq!(item.deadlines[0].state, ClockDeadlineState::Due);
        assert_eq!(item.deadlines[1].state, ClockDeadlineState::Due);

        // Past the early-warning deadline, still nothing submitted: `overdue`.
        let now = awareness + ChronoDuration::hours(25);
        let clock = compute(std::slice::from_ref(&f), &[], &[], now);
        assert_eq!(clock.items[0].deadlines[0].state, ClockDeadlineState::Overdue);
        assert_eq!(clock.items[0].deadlines[1].state, ClockDeadlineState::Due);

        // Submitted within 24h: `met`.
        let item_ref = ClockItemRef::Finding { scope: "demo".into(), vulnerability: "CVE-2026-1234".into() };
        let met = attestation(item_ref.clone(), ClockDeadlineKind::EarlyWarning, awareness + ChronoDuration::hours(2));
        let clock = compute(std::slice::from_ref(&f), &[], &[met], awareness + ChronoDuration::hours(30));
        assert_eq!(clock.items[0].deadlines[0].state, ClockDeadlineState::Met);
        assert_eq!(clock.items[0].deadlines[0].submission.as_ref().unwrap().at, awareness + ChronoDuration::hours(2));

        // Submitted after 24h: `late`.
        let late = attestation(item_ref, ClockDeadlineKind::EarlyWarning, awareness + ChronoDuration::hours(30));
        let clock = compute(&[f], &[], &[late], awareness + ChronoDuration::hours(40));
        assert_eq!(clock.items[0].deadlines[0].state, ClockDeadlineState::Late);
    }

    #[test]
    fn a_withdrawn_submission_leaves_the_deadline_overdue() {
        let awareness = at("2026-09-24T10:01:00Z");
        let f = finding("demo", "CVE-2026-1234", awareness);
        let item_ref = ClockItemRef::Finding { scope: "demo".into(), vulnerability: "CVE-2026-1234".into() };
        let mut withdrawn = attestation(item_ref, ClockDeadlineKind::EarlyWarning, awareness + ChronoDuration::hours(2));
        withdrawn.withdrawn = Some(Withdrawal { at: awareness + ChronoDuration::hours(3), by: "owner".into(), reason: None });

        let now = awareness + ChronoDuration::hours(30);
        let clock = compute(&[f], &[], &[withdrawn], now);
        assert_eq!(clock.items[0].deadlines[0].state, ClockDeadlineState::Overdue);
        assert!(clock.items[0].deadlines[0].submission.is_none());
    }

    #[test]
    fn an_excluded_item_has_no_deadlines() {
        let awareness = at("2026-09-24T10:01:00Z");
        let mut f = finding("demo", "CVE-2026-1234", awareness);
        f.latest_vex = Some("not_affected".into());
        let clock = compute(&[f], &[], &[], awareness + ChronoDuration::hours(1));
        assert_eq!(clock.items[0].excluded.as_deref(), Some("not_affected"));
        assert!(clock.items[0].deadlines.is_empty());
    }

    #[test]
    fn a_split_chain_counts_once_at_the_roots_awareness() {
        let root_awareness = at("2026-09-20T08:00:00Z");
        let child_awareness = at("2026-09-24T10:00:00Z"); // the split's own (later) receipt time
        let root = report("root", "demo", root_awareness, None);
        let child = report("child", "demo", child_awareness, Some("root"));
        let grandchild = report("grandchild", "demo", child_awareness, Some("child"));

        let clock = compute(&[], &[root, child, grandchild], &[], root_awareness + ChronoDuration::hours(1));
        assert_eq!(clock.items.len(), 1, "one item for the whole chain");
        assert_eq!(clock.items[0].item, ClockItemRef::Report { item: "root".into() });
        assert_eq!(clock.items[0].awareness_at, root_awareness, "never a part's own later receipt time");
    }

    #[test]
    fn a_dangling_parent_stops_the_walk_rather_than_panicking() {
        let awareness = at("2026-09-24T10:00:00Z");
        let orphan = report("orphan", "demo", awareness, Some("nobody-confirmed-this"));
        let clock = compute(&[], &[orphan], &[], awareness);
        assert_eq!(clock.items.len(), 1);
        assert_eq!(clock.items[0].item, ClockItemRef::Report { item: "orphan".into() });
    }

    #[test]
    fn clock_item_ref_round_trips_through_its_cli_text_form() {
        let finding = ClockItemRef::Finding { scope: "demo".into(), vulnerability: "CVE-2026-1234".into() };
        assert_eq!(finding.to_string(), "finding:demo:CVE-2026-1234");
        assert_eq!("finding:demo:CVE-2026-1234".parse::<ClockItemRef>().unwrap(), finding);

        let report = ClockItemRef::Report { item: "task-1".into() };
        assert_eq!(report.to_string(), "report:task-1");
        assert_eq!("report:task-1".parse::<ClockItemRef>().unwrap(), report);

        assert!("bogus".parse::<ClockItemRef>().is_err());
        assert!("finding:onlyscope".parse::<ClockItemRef>().is_err());
    }

    /// The documented limitation of splitting on the *last* `:`: a
    /// colon-bearing vulnerability id (never how CVE/GHSA/EUVD or CycloneDX
    /// itself names one) folds everything but its last segment into
    /// `scope`. Pinned so a future change to the split direction is a
    /// deliberate one.
    #[test]
    fn a_colon_bearing_vulnerability_id_folds_into_scope_on_the_last_colon() {
        let parsed: ClockItemRef = "finding:demo:cpe:2.3:a:x".parse().unwrap();
        assert_eq!(parsed, ClockItemRef::Finding { scope: "demo:cpe:2.3:a".into(), vulnerability: "x".into() });
    }

    #[test]
    fn deadline_kind_accepts_the_clis_hyphenated_spelling_too() {
        assert_eq!("early-warning".parse::<ClockDeadlineKind>().unwrap(), ClockDeadlineKind::EarlyWarning);
        assert_eq!("early_warning".parse::<ClockDeadlineKind>().unwrap(), ClockDeadlineKind::EarlyWarning);
        assert_eq!("notification".parse::<ClockDeadlineKind>().unwrap(), ClockDeadlineKind::Notification);
        assert!("whenever".parse::<ClockDeadlineKind>().is_err());
    }
}
