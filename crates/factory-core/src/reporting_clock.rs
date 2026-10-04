//! The CRA Art. 14 vulnerability reporting clock: 24h/72h from awareness,
//! and a 14-day final report from evidenced corrective-measure availability.
//! No final-report timestamp is invented before that evidence is recorded.
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

/// The control every clock submission and corrective-measure record uses.
pub fn art_14() -> ControlRef {
    ControlRef::new("cra", "art-14")
}

pub use factory_kernel::{ClockItemRef, ClockDeadlineKind, ClockMark, CorrectiveMeasureMark};

/// Offset from awareness for 24h/72h, or from corrective-measure
/// availability for the final report (Art. 14(2)(c)).
fn deadline_offset(kind: ClockDeadlineKind) -> ChronoDuration {
    match kind {
        ClockDeadlineKind::EarlyWarning => ChronoDuration::hours(24),
        ClockDeadlineKind::Notification => ChronoDuration::hours(72),
        ClockDeadlineKind::FinalReport => ChronoDuration::days(14),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectiveMeasure {
    pub attestation: String,
    pub available_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub by: String,
    pub evidence: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrective_measure: Option<CorrectiveMeasure>,
    /// Every confirmed report in this root's split chain, for UI lookup.
    /// Findings have none. Membership is computed, not persisted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub report_items: Vec<String>,
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
            corrective_measure: corrective_measure(&item, attestations),
            report_items: Vec::new(),
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
    let mut members: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for report in reports {
        let root = resolve_root(report, &by_item);
        roots.entry(root.item.as_str()).or_insert(root);
        members
            .entry(root.item.as_str())
            .or_default()
            .insert(report.item.clone());
    }
    for root in roots.values() {
        let item = ClockItemRef::Report {
            item: root.item.clone(),
        };
        let deadlines = deadlines_for(&item, root.awareness_at, attestations, now);
        items.push(ClockItem {
            corrective_measure: corrective_measure(&item, attestations),
            report_items: members
                .remove(root.item.as_str())
                .unwrap_or_default()
                .into_iter()
                .collect(),
            item,
            scope: root.scope.clone(),
            awareness_at: root.awareness_at,
            excluded: None,
            reported_now: true,
            deadlines,
        });
    }

    items.sort_by(|a, b| {
        a.awareness_at
            .cmp(&b.awareness_at)
            .then_with(|| a.item.cmp(&b.item))
    });
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
        let Some(parent) = by_item.get(parent_id) else {
            break;
        };
        if !seen.insert(parent.item.as_str()) {
            break;
        }
        current = parent;
    }
    current
}

/// Deadlines for one item: 24h/72h from awareness, final report only when
/// corrective-measure evidence supplies its anchor. State folds against a live submission
/// already exists -- `met`/`late` when one does (`now` never overrides a
/// submission once made), `overdue`/`due` against `now` when none does.
fn deadlines_for(
    item: &ClockItemRef,
    awareness_at: DateTime<Utc>,
    attestations: &[Attestation],
    now: DateTime<Utc>,
) -> Vec<ClockDeadline> {
    let measure = corrective_measure(item, attestations);
    [
        ClockDeadlineKind::EarlyWarning,
        ClockDeadlineKind::Notification,
        ClockDeadlineKind::FinalReport,
    ]
    .into_iter()
    .filter_map(|kind| {
        let anchor = if kind == ClockDeadlineKind::FinalReport {
            measure.as_ref()?.available_at
        } else {
            awareness_at
        };
        let due_at = anchor + deadline_offset(kind);
        let submission = earliest_live_submission(item, kind, attestations);
        let state = match &submission {
            Some(s) if s.at <= due_at => ClockDeadlineState::Met,
            Some(_) => ClockDeadlineState::Late,
            None if now > due_at => ClockDeadlineState::Overdue,
            None => ClockDeadlineState::Due,
        };
        Some(ClockDeadline {
            deadline: kind,
            due_at,
            state,
            submission,
        })
    })
    .collect()
}

/// Earliest live availability claim. A duplicate write never moves the
/// deadline later; expiry does not erase history, explicit withdrawal does.
fn corrective_measure(
    item: &ClockItemRef,
    attestations: &[Attestation],
) -> Option<CorrectiveMeasure> {
    attestations
        .iter()
        .filter(|a| a.withdrawn.is_none())
        .filter_map(|a| {
            let mark = a.corrective.as_ref()?;
            (&mark.item == item && a.control == art_14()).then(|| CorrectiveMeasure {
                attestation: a.id.clone(),
                available_at: mark.available_at,
                recorded_at: a.attested_at,
                by: a.attested_by.clone(),
                evidence: a.evidence.clone(),
            })
        })
        .min_by_key(|m| (m.available_at, m.recorded_at))
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

    fn attestation(
        item: ClockItemRef,
        deadline: ClockDeadlineKind,
        attested_at: DateTime<Utc>,
    ) -> Attestation {
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
            corrective: None,
            clock: Some(ClockMark { item, deadline }),
        }
    }

    fn report(
        item: &str,
        scope: &str,
        awareness_at: DateTime<Utc>,
        parent: Option<&str>,
    ) -> ConfirmedSecurityReport {
        ConfirmedSecurityReport {
            item: item.into(),
            scope: scope.into(),
            awareness_at,
            source: IntakeSource {
                kind: crate::intake::SourceKind::Cli,
                reference: None,
                provider: None,
                relayed_by: None,
                repository: None,
                number: None,
                external_id: None,
            },
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
        assert_eq!(
            item.deadlines[0].due_at,
            awareness + ChronoDuration::hours(24)
        );
        assert_eq!(
            item.deadlines[1].due_at,
            awareness + ChronoDuration::hours(72)
        );
        assert_eq!(item.deadlines[0].state, ClockDeadlineState::Due);
        assert_eq!(item.deadlines[1].state, ClockDeadlineState::Due);

        // Past the early-warning deadline, still nothing submitted: `overdue`.
        let now = awareness + ChronoDuration::hours(25);
        let clock = compute(std::slice::from_ref(&f), &[], &[], now);
        assert_eq!(
            clock.items[0].deadlines[0].state,
            ClockDeadlineState::Overdue
        );
        assert_eq!(clock.items[0].deadlines[1].state, ClockDeadlineState::Due);

        // Submitted within 24h: `met`.
        let item_ref = ClockItemRef::Finding {
            scope: "demo".into(),
            vulnerability: "CVE-2026-1234".into(),
        };
        let met = attestation(
            item_ref.clone(),
            ClockDeadlineKind::EarlyWarning,
            awareness + ChronoDuration::hours(2),
        );
        let clock = compute(
            std::slice::from_ref(&f),
            &[],
            &[met],
            awareness + ChronoDuration::hours(30),
        );
        assert_eq!(clock.items[0].deadlines[0].state, ClockDeadlineState::Met);
        assert_eq!(
            clock.items[0].deadlines[0].submission.as_ref().unwrap().at,
            awareness + ChronoDuration::hours(2)
        );

        // Submitted after 24h: `late`.
        let late = attestation(
            item_ref,
            ClockDeadlineKind::EarlyWarning,
            awareness + ChronoDuration::hours(30),
        );
        let clock = compute(&[f], &[], &[late], awareness + ChronoDuration::hours(40));
        assert_eq!(clock.items[0].deadlines[0].state, ClockDeadlineState::Late);
    }

    #[test]
    fn a_withdrawn_submission_leaves_the_deadline_overdue() {
        let awareness = at("2026-09-24T10:01:00Z");
        let f = finding("demo", "CVE-2026-1234", awareness);
        let item_ref = ClockItemRef::Finding {
            scope: "demo".into(),
            vulnerability: "CVE-2026-1234".into(),
        };
        let mut withdrawn = attestation(
            item_ref,
            ClockDeadlineKind::EarlyWarning,
            awareness + ChronoDuration::hours(2),
        );
        withdrawn.withdrawn = Some(Withdrawal {
            at: awareness + ChronoDuration::hours(3),
            by: "owner".into(),
            reason: None,
        });

        let now = awareness + ChronoDuration::hours(30);
        let clock = compute(&[f], &[], &[withdrawn], now);
        assert_eq!(
            clock.items[0].deadlines[0].state,
            ClockDeadlineState::Overdue
        );
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

        let clock = compute(
            &[],
            &[root, child, grandchild],
            &[],
            root_awareness + ChronoDuration::hours(1),
        );
        assert_eq!(clock.items.len(), 1, "one item for the whole chain");
        assert_eq!(
            clock.items[0].item,
            ClockItemRef::Report {
                item: "root".into()
            }
        );
        assert_eq!(
            clock.items[0].awareness_at, root_awareness,
            "never a part's own later receipt time"
        );
        assert_eq!(clock.items[0].report_items, ["child", "grandchild", "root"]);
    }

    #[test]
    fn a_dangling_parent_stops_the_walk_rather_than_panicking() {
        let awareness = at("2026-09-24T10:00:00Z");
        let orphan = report("orphan", "demo", awareness, Some("nobody-confirmed-this"));
        let clock = compute(&[], &[orphan], &[], awareness);
        assert_eq!(clock.items.len(), 1);
        assert_eq!(
            clock.items[0].item,
            ClockItemRef::Report {
                item: "orphan".into()
            }
        );
    }

    #[test]
    fn clock_item_ref_round_trips_through_its_cli_text_form() {
        let finding = ClockItemRef::Finding {
            scope: "demo".into(),
            vulnerability: "CVE-2026-1234".into(),
        };
        assert_eq!(finding.to_string(), "finding:demo:CVE-2026-1234");
        assert_eq!(
            "finding:demo:CVE-2026-1234"
                .parse::<ClockItemRef>()
                .unwrap(),
            finding
        );

        let report = ClockItemRef::Report {
            item: "task-1".into(),
        };
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
        assert_eq!(
            parsed,
            ClockItemRef::Finding {
                scope: "demo:cpe:2.3:a".into(),
                vulnerability: "x".into()
            }
        );
    }

    #[test]
    fn deadline_kind_accepts_the_clis_hyphenated_spelling_too() {
        assert_eq!(
            "early-warning".parse::<ClockDeadlineKind>().unwrap(),
            ClockDeadlineKind::EarlyWarning
        );
        assert_eq!(
            "early_warning".parse::<ClockDeadlineKind>().unwrap(),
            ClockDeadlineKind::EarlyWarning
        );
        assert_eq!(
            "notification".parse::<ClockDeadlineKind>().unwrap(),
            ClockDeadlineKind::Notification
        );
        assert_eq!(
            "final-report".parse::<ClockDeadlineKind>().unwrap(),
            ClockDeadlineKind::FinalReport
        );
        assert!("whenever".parse::<ClockDeadlineKind>().is_err());
    }

    fn measure(
        item: ClockItemRef,
        available_at: DateTime<Utc>,
        recorded_at: DateTime<Utc>,
    ) -> Attestation {
        let mut a = attestation(item.clone(), ClockDeadlineKind::FinalReport, recorded_at);
        a.clock = None;
        a.corrective = Some(CorrectiveMeasureMark { item, available_at });
        a
    }

    #[test]
    fn final_report_uses_measure_availability_never_awareness_or_record_time() {
        let awareness = at("2026-09-01T09:00:00Z");
        let available = at("2026-09-20T09:00:00Z");
        let recorded = at("2026-09-23T09:00:00Z");
        let reports = [report("root", "demo", awareness, None)];
        let item = ClockItemRef::Report {
            item: "root".into(),
        };
        assert_eq!(
            compute(&[], &reports, &[], recorded).items[0]
                .deadlines
                .len(),
            2
        );
        let anchor = measure(item.clone(), available, recorded);
        let due = available + ChronoDuration::days(14);
        let clock = compute(&[], &reports, std::slice::from_ref(&anchor), due);
        let row = &clock.items[0];
        assert_eq!(row.awareness_at, awareness);
        assert_eq!(
            row.deadlines[0].due_at,
            awareness + ChronoDuration::hours(24)
        );
        assert_eq!(
            row.deadlines[1].due_at,
            awareness + ChronoDuration::hours(72)
        );
        assert_eq!(row.deadlines[2].due_at, due);
        assert_eq!(row.deadlines[2].state, ClockDeadlineState::Due);
        assert_eq!(
            row.corrective_measure.as_ref().unwrap().evidence,
            anchor.evidence
        );
        let overdue = compute(
            &[],
            &reports,
            std::slice::from_ref(&anchor),
            due + ChronoDuration::seconds(1),
        );
        assert_eq!(
            overdue.items[0].deadlines[2].state,
            ClockDeadlineState::Overdue
        );
        for (at, state) in [
            (due, ClockDeadlineState::Met),
            (due + ChronoDuration::seconds(1), ClockDeadlineState::Late),
        ] {
            let submission = attestation(item.clone(), ClockDeadlineKind::FinalReport, at);
            let clock = compute(
                &[],
                &reports,
                &[anchor.clone(), submission],
                due + ChronoDuration::days(100),
            );
            assert_eq!(clock.items[0].deadlines[2].state, state);
        }
    }

    #[test]
    fn measure_expiry_does_not_erase_history_but_withdrawal_does_and_duplicates_choose_earliest() {
        let awareness = at("2026-09-01T09:00:00Z");
        let available = awareness + ChronoDuration::days(3);
        let findings = [finding("demo", "CVE-2026-1234", awareness)];
        let item = ClockItemRef::Finding {
            scope: "demo".into(),
            vulnerability: "CVE-2026-1234".into(),
        };
        let mut earlier = measure(item.clone(), available, available);
        earlier.expires_at = available + ChronoDuration::seconds(1);
        let later = measure(
            item,
            available + ChronoDuration::days(2),
            available + ChronoDuration::days(2),
        );
        let now = available + ChronoDuration::days(30);
        let clock = compute(&findings, &[], &[later, earlier.clone()], now);
        assert_eq!(
            clock.items[0].deadlines[2].due_at,
            available + ChronoDuration::days(14)
        );
        earlier.withdrawn = Some(Withdrawal {
            at: now,
            by: "owner".into(),
            reason: Some("incorrect evidence".into()),
        });
        let clock = compute(&findings, &[], &[earlier], now);
        assert_eq!(clock.items[0].deadlines.len(), 2);
        assert!(clock.items[0].corrective_measure.is_none());
    }

    #[test]
    fn old_attestations_and_clock_items_without_measure_fields_still_deserialize() {
        let now = at("2026-09-01T09:00:00Z");
        let item = ClockItemRef::Report {
            item: "root".into(),
        };
        let mut json =
            serde_json::to_value(attestation(item, ClockDeadlineKind::Notification, now)).unwrap();
        json.as_object_mut().unwrap().remove("corrective");
        assert!(serde_json::from_value::<Attestation>(json)
            .unwrap()
            .corrective
            .is_none());
        let clock = compute(&[], &[report("root", "demo", now, None)], &[], now);
        let mut json = serde_json::to_value(&clock.items[0]).unwrap();
        json.as_object_mut().unwrap().remove("corrective_measure");
        assert!(serde_json::from_value::<ClockItem>(json)
            .unwrap()
            .corrective_measure
            .is_none());
    }
}
