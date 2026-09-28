//! A policy report, packaged for an auditor: `factory policy export` and
//! `GET /api/policy/export` (`#83`). Not part of `policy.rs` itself --
//! [`PolicyExport`] wraps [`crate::protocol::PolicyReport`], and `policy.rs`
//! never imports `protocol` (the dependency runs the other way: `protocol.rs`
//! is the wire layer built *from* the domain modules, `policy.rs` among
//! them). This module sits beside `protocol`, not inside `policy`, for
//! exactly that reason -- it is free to depend on both.
//!
//! [`export_markdown`] is pure, the same discipline `policy.rs` holds
//! itself to: `Engine::policy_export` (`factory-daemon`) is the one caller,
//! and it already has everything this needs from a [`crate::protocol::PolicyReport`]
//! it built and the attestations store it already reads for other requests.
//! `format=json` is `serde_json::to_string_pretty(&export)` at the call
//! site -- one data model behind both formats, so neither can say something
//! the other does not.

use crate::policy::{Attestation, ControlRef};
use crate::protocol::PolicyReport;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Everything `export_markdown`/`serde_json` render. `report` is exactly
/// what `Request::Policy` itself returns -- every row's own statuses,
/// `refs`, the subtree rollup, `n/a` declarations, findings and catalogues
/// -- so an export can never drift from what the L6 tab shows for the same
/// scope. `attestations` is the one thing `PolicyReport` does not carry: a
/// control's whole attestation history (withdrawn and expired included, an
/// audit trail's business), not just the id `ControlStatus.refs` points at
/// for whichever one currently decides a status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyExport {
    /// The instance's own name (`Config.instance.name`).
    pub instance: String,
    /// `None` when the whole instance was exported -- `report.scope`'s own
    /// meaning, carried here too since `export_filename` needs it without
    /// reaching into `report`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub produced_at: DateTime<Utc>,
    pub report: PolicyReport,
    /// Every attestation recorded at a row's own scope or an ancestor of it
    /// -- keyed by `ScopePolicy.scope`, the same "scope or ancestor" rule
    /// `Engine::policy_report`'s own `Evidence` assembly already applies per
    /// scope (`factory-daemon/src/policies/mod.rs`), just kept here rather
    /// than folded into a status and discarded. A control's *whole* history
    /// at that scope, the same as `PolicyControlDetail.attestations` --
    /// `export_markdown` filters each scope's list down to one control by
    /// `Attestation::control` as it renders.
    #[serde(default)]
    pub attestations: BTreeMap<String, Vec<Attestation>>,
}

/// `policy-<scope>-<date>.<format>`, or `policy-instance-<date>.<format>`
/// for the whole instance -- the literal word `instance`, not
/// `PolicyExport.instance` (an instance's own name may hold spaces or
/// slashes, which a file name should not have to survive). `produced_at`'s
/// own date, not `Utc::now()` read again -- a caller that renders both
/// `md` and `json` from one `PolicyExport` gets the same date on both.
pub fn export_filename(export: &PolicyExport, format: &str) -> String {
    let scope_part = export.scope.as_deref().unwrap_or("instance");
    format!("policy-{scope_part}-{}.{format}", export.produced_at.format("%Y-%m-%d"))
}

/// One attestation's own line -- evidence pointer, who, when, expiry, and
/// (append-only, so it is never dropped from the history) a withdrawal.
fn attestation_line(a: &Attestation) -> String {
    let withdrawn = a
        .withdrawn
        .as_ref()
        .map(|w| {
            format!(
                ", withdrawn {} by {}{}",
                w.at.to_rfc3339(),
                w.by,
                w.reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default()
            )
        })
        .unwrap_or_default();
    format!(
        "attestation `{}` -- evidence {}, by {}, attested {}, expires {}{withdrawn}",
        a.id,
        a.evidence,
        a.attested_by,
        a.attested_at.to_rfc3339(),
        a.expires_at.to_rfc3339(),
    )
}

/// One control's full markdown entry -- status, reasons, refs, and its own
/// attestation history -- written under whichever scope heading is
/// currently open. `atts` is already this control's slice of that scope's
/// own attestations (see the caller in [`export_markdown`]).
fn control_section(out: &mut String, control: &ControlRef, title: &str, status: &crate::policy::Status, refs: &[crate::policy::EvidenceRef], atts: &[&Attestation]) {
    out.push_str(&format!("\n#### {control} -- {title}\n\n"));
    out.push_str(&format!("- status: {}\n", status.kind().as_str()));
    for reason in status.reasons() {
        out.push_str(&format!("- reason: {reason}\n"));
    }
    if !refs.is_empty() {
        let joined = refs
            .iter()
            .map(|r| format!("{}:{}", r.kind.as_str(), r.id))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("- refs: {joined}\n"));
    }
    for a in atts {
        out.push_str(&format!("- {}\n", attestation_line(a)));
    }
}

/// The whole export as markdown: an instance/scope/produced-at header and
/// the "compliant means evidence complete" sentence ADR 0004 asks every
/// view of this to carry, then per framework its rollup and, scope by
/// scope, every control that framework applies to there, then every `n/a`
/// declaration with its rationale and declaring scope, then every finding.
/// No page text anywhere in it -- every pointer is an id
/// (`EvidenceRef`/`Attestation.evidence`), never a knowledge page's own
/// content, exactly the issue's own "no page text, only pointers" rule.
pub fn export_markdown(export: &PolicyExport) -> String {
    let report = &export.report;
    let mut out = String::new();

    out.push_str("# Policy export\n\n");
    out.push_str(&format!("- Instance: {}\n", export.instance));
    out.push_str(&format!(
        "- Scope: {}\n",
        export.scope.as_deref().unwrap_or("whole instance")
    ));
    out.push_str(&format!("- Produced at: {}\n", export.produced_at.to_rfc3339()));
    out.push_str(
        "\n\"Compliant\" means the evidence is complete for every regulation and standard that \
         applies here -- never that anything is certified. A best-practice control is shown but \
         never counted.\n",
    );

    let titles: BTreeMap<&str, &str> = report
        .catalogues
        .iter()
        .map(|c| (c.framework.as_str(), c.title.as_str()))
        .collect();

    for r in &report.rollup {
        let title = titles.get(r.framework.as_str()).copied().unwrap_or(r.framework.as_str());
        out.push_str(&format!("\n## {title} ({})\n\n", r.framework));
        out.push_str(&format!(
            "{} satisfied, {} attested, {} stale, {} open, {} n/a -- {}\n",
            r.counts.satisfied,
            r.counts.attested,
            r.counts.stale,
            r.counts.open,
            r.counts.not_applicable,
            if r.compliant { "compliant" } else { "not compliant" },
        ));
        let bp = &r.best_practice;
        if bp.satisfied + bp.attested + bp.stale + bp.open + bp.not_applicable > 0 {
            out.push_str(&format!(
                "best practice (shown, never counted): {} satisfied, {} attested, {} stale, {} open, {} n/a\n",
                bp.satisfied, bp.attested, bp.stale, bp.open, bp.not_applicable,
            ));
        }

        for row in &report.rows {
            let controls: Vec<&crate::policy::ControlStatus> =
                row.statuses.iter().filter(|s| s.control.framework == r.framework).collect();
            if controls.is_empty() {
                continue;
            }
            out.push_str(&format!("\n### {}\n", row.scope));
            let scope_atts = export.attestations.get(&row.scope);
            for c in controls {
                let atts: Vec<&Attestation> = scope_atts
                    .map(|v| v.iter().filter(|a| a.control == c.control).collect())
                    .unwrap_or_default();
                control_section(&mut out, &c.control, &c.title, &c.status, &c.refs, &atts);
            }
        }
    }

    if !report.not_applicable.is_empty() {
        out.push_str("\n## Not applicable\n\n");
        for na in &report.not_applicable {
            out.push_str(&format!("- {} at {}: {}\n", na.control, na.scope, na.rationale));
        }
    }

    if !report.findings.is_empty() {
        out.push_str("\n## Findings\n\n");
        for f in &report.findings {
            out.push_str(&format!("- {:?} -- {} -- {}\n", f.kind, f.subject, f.detail));
        }
    }

    out.trim_end().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{
        Attestation, ControlRef, ControlStatus, EvidenceRef, EvidenceRefKind, Finding, FindingKind, FrameworkRollup,
        Kind, Status, StatusCounts,
    };
    use crate::protocol::{CatalogueSummary, NotApplicableEntry, ScopePolicy};
    use chrono::TimeZone;

    fn t(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
    }

    fn sample() -> PolicyExport {
        let control = ControlRef::new("cra", "annex-i-2-1");
        let status = ControlStatus {
            control: control.clone(),
            title: "Identify and document components".to_string(),
            kind: Kind::Regulation,
            refs: vec![EvidenceRef { kind: EvidenceRefKind::Task, id: "task-1".to_string() }],
            status: Status::Open { reasons: vec!["knowledge: tag `control/cra/annex-i-2-1` not found".to_string()] },
        };
        let attestation = Attestation {
            id: "att-1".to_string(),
            control: control.clone(),
            scope: "demo".to_string(),
            evidence: "https://example.com/sbom".to_string(),
            note: None,
            attested_by: "owner".to_string(),
            attested_at: t(2026, 1, 1),
            expires_at: t(2026, 6, 1),
            withdrawn: None,
            clock: None,
        };
        let report = PolicyReport {
            scope: Some("demo".to_string()),
            rows: vec![ScopePolicy {
                scope: "demo".to_string(),
                statuses: vec![status],
                rollup: vec![],
                open_tasks: Default::default(),
            }],
            rollup: vec![FrameworkRollup {
                framework: "cra".to_string(),
                counts: StatusCounts { open: 1, ..Default::default() },
                best_practice: StatusCounts::default(),
                compliant: false,
            }],
            not_applicable: vec![NotApplicableEntry {
                control: ControlRef::new("cra", "annex-i-2-2"),
                scope: "demo".to_string(),
                rationale: "we ship no hardware".to_string(),
            }],
            findings: vec![Finding {
                kind: FindingKind::EmptyRationale,
                subject: "demo".to_string(),
                detail: "cra/annex-i-2-3 marked not applicable with no rationale".to_string(),
            }],
            catalogues: vec![CatalogueSummary {
                framework: "cra".to_string(),
                title: "Cyber Resilience Act".to_string(),
                kind: Kind::Regulation,
                controls: 3,
            }],
        };
        let mut attestations = BTreeMap::new();
        attestations.insert("demo".to_string(), vec![attestation]);
        PolicyExport {
            instance: "acme".to_string(),
            scope: Some("demo".to_string()),
            produced_at: t(2026, 6, 15),
            report,
            attestations,
        }
    }

    #[test]
    fn export_filename_names_the_scope_and_the_produced_date() {
        let export = sample();
        assert_eq!(export_filename(&export, "md"), "policy-demo-2026-06-15.md");
        assert_eq!(export_filename(&export, "json"), "policy-demo-2026-06-15.json");

        let mut whole_instance = sample();
        whole_instance.scope = None;
        assert_eq!(export_filename(&whole_instance, "md"), "policy-instance-2026-06-15.md");
    }

    #[test]
    fn export_markdown_carries_header_rollup_control_attestation_na_and_findings() {
        let text = export_markdown(&sample());

        assert!(text.contains("Instance: acme"), "{text}");
        assert!(text.contains("Scope: demo"), "{text}");
        assert!(text.contains("Produced at: 2026-06-15"), "{text}");
        assert!(text.contains("means the evidence is complete"), "{text}");

        // The rollup, keyed by the catalogue's own title.
        assert!(text.contains("Cyber Resilience Act (cra)"), "{text}");
        assert!(text.contains("1 open"), "{text}");
        assert!(text.contains("not compliant"), "{text}");

        // The control itself, nested under its scope, with status, reason and refs.
        assert!(text.contains("### demo"), "{text}");
        assert!(text.contains("cra/annex-i-2-1"), "{text}");
        assert!(text.contains("status: open"), "{text}");
        assert!(text.contains("knowledge: tag `control/cra/annex-i-2-1` not found"), "{text}");
        assert!(text.contains("refs: task:task-1"), "{text}");

        // Its attestation, in full -- pointer, who, when, expiry.
        assert!(text.contains("attestation `att-1`"), "{text}");
        assert!(text.contains("evidence https://example.com/sbom"), "{text}");
        assert!(text.contains("by owner"), "{text}");

        // Not applicable, with rationale and the declaring scope.
        assert!(text.contains("## Not applicable"), "{text}");
        assert!(text.contains("cra/annex-i-2-2 at demo: we ship no hardware"), "{text}");

        // Findings.
        assert!(text.contains("## Findings"), "{text}");
        assert!(text.contains("EmptyRationale"), "{text}");
    }

    #[test]
    fn export_markdown_shows_a_withdrawn_attestation_rather_than_dropping_it() {
        let mut export = sample();
        let atts = export.attestations.get_mut("demo").unwrap();
        atts[0].withdrawn = Some(crate::policy::Withdrawal {
            at: t(2026, 3, 1),
            by: "owner".to_string(),
            reason: Some("superseded".to_string()),
        });
        let text = export_markdown(&export);
        assert!(text.contains("withdrawn 2026-03-01"), "{text}");
        assert!(text.contains("superseded"), "{text}");
    }

    #[test]
    fn json_and_markdown_render_from_the_same_data() {
        let export = sample();
        let json = serde_json::to_string(&export).unwrap();
        let back: PolicyExport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.instance, export.instance);
        assert_eq!(export_markdown(&back), export_markdown(&export));
    }
}
