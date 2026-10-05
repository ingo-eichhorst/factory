//! Plain shared execution-plan vocabulary. L5 owns compilation; L4 owns
//! injection, verification and its view helper. No evaluation lives here.
use crate::StepKind;
use serde::{Deserialize, Serialize};

/// The category a task or workflow that names none is planned as -- so that
/// leaving `category` out is never a way around the plan. A catalogue
/// catches uncategorised work with `applies_to: [default]`.
pub const DEFAULT_CATEGORY: &str = "default";

/// In `applies_to`, every category, `default` included.
pub const ANY_CATEGORY: &str = "*";

/// The category a piece of work is planned as.
pub fn effective_category(category: Option<&str>) -> &str {
    match category.map(str::trim) {
        Some(c) if !c.is_empty() => c,
        _ => DEFAULT_CATEGORY,
    }
}

/// `[a-z0-9][a-z0-9_-]*` -- a category or a step name. Wider than
/// `dataset::is_slug` by the underscore, because the issue's own step names
/// (`security_scan`, `security-report`) use both.
pub fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Refuse a category that could never match an `applies_to` entry.
pub fn check_category(category: &str) -> Result<(), String> {
    if is_name(category) {
        Ok(())
    } else {
        Err(format!(
            "category {category:?} is not a name: lowercase letters, digits, '-' and '_', starting with a letter or digit"
        ))
    }
}

/// One step of a resolved plan -- every requirement for the same step name
/// and command folded into one, naming every control that asked for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Unique within the plan: the step name, or `<name>-2`, `-3`, ... when
    /// two controls require the same step name with different commands --
    /// add only, so both run.
    pub id: String,
    pub step: String,
    pub kind: StepKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The shortest any requirement asked for -- tighten only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// `<framework>/<control>` for a policy control, `quality/<attribute>`
    /// for a quality attribute.
    pub required_by: Vec<String>,
    /// Kept on the wire for v1 snapshots. New plans enforce every kind.
    pub enforced: bool,
}

/// A control marked `n/a` whose requirements would otherwise have applied --
/// the one way a step leaves a plan, listed like any other so it is never a
/// silent drop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiver {
    pub control: String,
    pub scope: String,
    pub rationale: String,
    pub steps: Vec<String>,
}

/// A scope's control plan for one category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlan {
    pub scope: String,
    pub category: String,
    /// In execution order: `before`/`after` respected, otherwise by id.
    pub steps: Vec<PlanStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waived: Vec<Waiver>,
    /// Anything the plan could not honour as written -- a gate with no
    /// command, an ordering cycle -- in words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<String>,
}

/// A freshly compiled L5 execution plan, read through the upward fact port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledPlanFact {
    pub plan: ControlPlan,
}
