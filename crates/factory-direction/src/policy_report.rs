//! L6 policy report data, independent of the transport/router envelope.
use serde::{Deserialize, Serialize};

/// One framework over a report's rows, summarized for the L6 tab -- title,
/// kind and how many (scope, control) pairs it has. A control's own detail already lives in whichever
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
/// own flattened copy of every `n/a` across the rows it returns, so none is
/// silent (ADR 0004's Statement of Applicability).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotApplicableEntry {
    pub control: crate::policy::ControlRef,
    /// The scope whose own policy declared it -- the only scope it holds in.
    pub scope: String,
    pub rationale: String,
}

/// Where one framework has a policy over the subtree a report was asked
/// about (#390): every scope that owns one, and every scope in the subtree
/// that has none although another scope there does. A scope without a
/// policy is listed, never counted as compliant -- visibility instead of
/// inheritance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameworkApplicability {
    pub framework: String,
    /// Every scope in the subtree with its own policy for the framework.
    pub covers: Vec<String>,
    /// Every scope in the subtree without one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_policy: Vec<String>,
}

impl FrameworkApplicability {
    /// In words, the one wording `factory policy`, the export and the L6
    /// tab's port share.
    pub fn describe(&self) -> String {
        let mut line = format!("policy at {}", self.covers.join(", "));
        if !self.no_policy.is_empty() {
            line.push_str(&format!("; no policy at {}", self.no_policy.join(", ")));
        }
        line
    }
}

/// One scope's own policy status board: every control its own policies
/// commit it to, its own rollup (its controls alone -- see
/// `PolicyReport::rollup` for the subtree's), and every template control
/// it left unselected. One of `Request::Policy`'s rows, labelled with the
/// scope that owns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopePolicy {
    pub scope: String,
    pub statuses: Vec<crate::policy::ControlStatus>,
    pub rollup: Vec<crate::policy::FrameworkRollup>,
    /// Template controls this scope's policies neither picked nor marked
    /// `n/a` (#390): listed, never counted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_selected: Vec<crate::policy::NotSelected>,
    /// A control's `framework/id` to the id of the non-terminal task in this
    /// scope labelled `policy=<framework>/<id>` -- the one
    /// `Request::PolicyRemediate` would refuse a second call naming -- so a
    /// reader can show that task instead of offering to create another
    /// (`#98`, the same field `ScopeQuality` carries).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub open_tasks: std::collections::BTreeMap<String, String>,
}

/// One template, summarized (#390): what `factory policy templates` lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSummary {
    pub template: String,
    pub title: String,
    pub kind: crate::policy::Kind,
    pub controls: usize,
    /// Every scope with a policy that picks from it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used_by: Vec<String>,
}

/// Whether a scope picked one template control (#390).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    Selected,
    NotApplicable,
    NotSelected,
}

/// One template control and what one scope did with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateControlSelection {
    pub id: String,
    pub title: String,
    pub selection: Selection,
    /// The `n/a` rationale, when `selection` is `not_applicable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

/// One scope's selection from a template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateScopeSelection {
    pub scope: String,
    /// The framework of the scope's policy picking from the template --
    /// `None` when the scope has none, and then nothing is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    pub controls: Vec<TemplateControlSelection>,
}

/// `factory policy template show`'s answer: the template, and per scope
/// asked about -- the one given, or every scope picking from it -- each
/// control's selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateView {
    pub template: crate::policy::Template,
    pub scopes: Vec<TemplateScopeSelection>,
}

/// The L6 Policy tab's whole answer (#390): the asked scope's own policies
/// and every descendant's, one row per owning scope, a rollup aggregated
/// over that whole subtree, and everything that would otherwise be silent
/// -- `n/a` declarations, unselected template controls, scopes with no
/// policy for a framework another scope uses, and every finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyReport {
    /// `None` when the whole instance was asked about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// One row per scope in the subtree that owns a policy. A scope with
    /// none has no row; `applicability` names it instead.
    pub rows: Vec<ScopePolicy>,
    /// Per framework, every (scope, control) pair over the rows above: the
    /// subtree is compliant for a framework only when every scope with a
    /// policy for it is.
    pub rollup: Vec<crate::policy::FrameworkRollup>,
    pub not_applicable: Vec<NotApplicableEntry>,
    /// Every framework some scope in the subtree has a policy for, with the
    /// scopes that do and the scopes that do not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applicability: Vec<FrameworkApplicability>,
    /// Template and policy load findings over the subtree (and instance-wide
    /// template findings), legacy `policies:` blocks, and every row's own
    /// evidence findings, deduplicated.
    pub findings: Vec<crate::policy::Finding>,
    /// Required workflow controls as they would be injected now. This is
    /// advisory preview data; immutable run snapshots remain authoritative.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_enforcement: Vec<WorkflowEnforcement>,
    /// Ordering and functionary gaps from the same workflow lint pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_findings: Vec<WorkflowEnforcementFinding>,
    /// One per framework over the rows: its title and kind (the first
    /// owning policy's) and how many (scope, control) pairs it has.
    pub catalogues: Vec<CatalogueSummary>,
    /// Every template on disk -- what a scope may pick its controls from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<TemplateSummary>,
}

pub use factory_kernel::{WorkflowEnforcement, WorkflowEnforcementFinding};

/// One control's full detail in the scope that owns it: its data as that
/// scope's policy states it, its status there, and its whole attestation
/// history at that scope -- including withdrawn and expired ones.
/// `Request::PolicyControl`'s answer.
// `#278`: no `Eq` -- `checks: Vec<Check>` can carry a `Check::Metric`,
// whose `above`/`below` are `Option<f64>`, and `f64` has no `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyControlDetail {
    pub control: crate::policy::ControlRef,
    pub title: String,
    pub kind: crate::policy::Kind,
    /// Exactly as the catalogue wrote it -- see `policy::Applied::evidence`.
    pub checks: Vec<crate::policy::Check>,
    pub maps_to: Vec<crate::policy::ControlRef>,
    /// The effective freshness window -- `policy::Applied::max_age`.
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
    /// Every attestation ever recorded for this control at this scope, most
    /// recent first -- withdrawn and expired ones included, since this is
    /// the audit trail, not just what currently holds.
    pub attestations: Vec<crate::policy::Attestation>,
}
