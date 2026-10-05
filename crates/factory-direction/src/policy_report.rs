//! L6 policy report data, independent of the transport/router envelope.
use serde::{Deserialize, Serialize};

/// One catalogue, summarized for the L6 tab -- title, kind and how many
/// controls it defines. A control's own detail already lives in whichever
/// `ScopePolicy.statuses` names it, so this is only what a catalogue picker
/// needs, never the controls themselves twice over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueSummary {
    pub framework: String,
    pub title: String,
    pub kind: crate::policy::Kind,
    pub controls: usize,
}

/// Which scope declared a control not applicable, and why -- `Request::Policy`'s
/// own flattened copy of every `n/a` across the rows it returns, deduplicated,
/// so a caller that only asked about a leaf scope still sees the rationale an
/// ancestor wrote down, and none is silent (ADR 0004's Statement of
/// Applicability).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotApplicableEntry {
    pub control: crate::policy::ControlRef,
    /// The scope that declared it -- not necessarily the scope a row is
    /// about, since a declaration is inherited by everything below it.
    pub scope: String,
    pub rationale: String,
}

/// One scope's own policy status board: every control applicable there, and
/// this scope's own rollup (its controls alone, not the subtree's -- see
/// `PolicyReport::rollup` for that). One of `Request::Policy`'s rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopePolicy {
    pub scope: String,
    pub statuses: Vec<crate::policy::ControlStatus>,
    pub rollup: Vec<crate::policy::FrameworkRollup>,
    /// A control's `framework/id` to the id of the non-terminal task in this
    /// scope labelled `policy=<framework>/<id>` -- the one
    /// `Request::PolicyRemediate` would refuse a second call naming -- so a
    /// reader can show that task instead of offering to create another
    /// (`#98`, the same field `ScopeQuality` carries).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub open_tasks: std::collections::BTreeMap<String, String>,
}

/// The L6 Policy tab's whole answer: every applicable control's status for
/// the asked scope and every scope below it, a rollup over that whole
/// subtree, and everything that would otherwise be silent -- `n/a`
/// declarations, catalogue and applicability findings, and the catalogues
/// themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyReport {
    /// `None` when the whole instance was asked about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// One row per scope that has at least one applicable control. A scope
    /// whose whole chain applies no framework is omitted -- there is
    /// nothing to show, and no silent "0 controls" row to explain.
    pub rows: Vec<ScopePolicy>,
    /// Per framework, over every row above: a control counts compliant only
    /// when it is compliant in every scope it applies to, not merely one --
    /// see `policy::worst_across_scopes`, which this is built from.
    pub rollup: Vec<crate::policy::FrameworkRollup>,
    pub not_applicable: Vec<NotApplicableEntry>,
    /// Catalogue parse findings (`policy::load_all`) and every row's own
    /// applicability findings (`policy::applicable`), deduplicated -- the
    /// same finding reached by walking two different scopes' chains is
    /// reported once.
    pub findings: Vec<crate::policy::Finding>,
    /// Required workflow controls as they would be injected now. This is
    /// advisory preview data; immutable run snapshots remain authoritative.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_enforcement: Vec<WorkflowEnforcement>,
    /// Ordering and functionary gaps from the same workflow lint pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_findings: Vec<WorkflowEnforcementFinding>,
    pub catalogues: Vec<CatalogueSummary>,
}

pub use factory_kernel::{WorkflowEnforcement, WorkflowEnforcementFinding};

/// One control's full detail: its catalogue data as it applies at the scope
/// asked about, its status there, and its whole attestation history --
/// including withdrawn and expired ones -- for that scope and its ancestors.
/// `Request::PolicyControl`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyControlDetail {
    pub control: crate::policy::ControlRef,
    pub title: String,
    pub kind: crate::policy::Kind,
    /// Exactly as the catalogue wrote it -- see `policy::Applied::evidence`.
    pub checks: Vec<crate::policy::Check>,
    pub maps_to: Vec<crate::policy::ControlRef>,
    /// The effective freshness window after folding in every scope's own
    /// tightening -- `policy::Applied::max_age`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<crate::policy::Duration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_applicable: Option<crate::policy::AppliedNotApplicable>,
    /// The catalogue's own `remediation:` text, if it wrote one --
    /// `policy::Applied::remediation`, carried through so both the L6 tab's
    /// control-detail modal and `Engine::policy_remediate` (`#83`) read it
    /// off this one struct rather than a second lookup against the
    /// catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    /// Machine-readable pointers alongside `status`'s reasons -- see
    /// `policy::EvidenceRef`. `ControlStatus` already carries these;
    /// `policy_control` (`factory-daemon`) would otherwise drop them
    /// building this from it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<crate::policy::EvidenceRef>,
    pub status: crate::policy::Status,
    /// The id of the non-terminal task in this scope labelled
    /// `policy=<framework>/<id>` for this control, if one is open -- the
    /// same lookup as `ScopePolicy::open_tasks` (`#98`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_task: Option<String>,
    /// Every attestation ever recorded for this control at this scope or an
    /// ancestor of it, most recent first -- withdrawn and expired ones
    /// included, since this is the audit trail, not just what currently
    /// holds.
    pub attestations: Vec<crate::policy::Attestation>,
}
