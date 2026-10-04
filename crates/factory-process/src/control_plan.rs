//! L4 generic execution plan and verification; no policy catalogue/compiler.
use chrono::{DateTime, Utc};
pub use factory_kernel::StepKind;
use serde::{Deserialize, Serialize};

/// The category a task or workflow that names none is planned as -- so that
/// leaving `category` out is never a way around the plan. A catalogue
/// catches uncategorised work with `applies_to: [default]`.
pub const DEFAULT_CATEGORY: &str = "default";

/// In `applies_to`, every category, `default` included.
pub const ANY_CATEGORY: &str = "*";

/// Who a gate's attestation names as having run it. The daemon, never the
/// agent whose work is being judged -- see [`judge`].
pub const GATE_ACTOR: &str = "factory-daemon";

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

impl ControlPlan {
    /// The steps the daemon injects and judges.
    pub fn enforced(&self) -> impl Iterator<Item = &PlanStep> {
        self.steps.iter().filter(|s| s.enforced)
    }
}

// ============================================================ attestation

pub use factory_kernel::RequiredStep;

pub use factory_kernel::AttestationVerdict;

pub use factory_kernel::StepAttestation;

/// Whether a run's evidence is complete, and if not, why -- in words a
/// person reads in the Inbox.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
}

impl Verification {
    /// One sentence for the run's block reason.
    pub fn reason(&self) -> String {
        let mut parts = Vec::new();
        if !self.failed.is_empty() {
            parts.push(format!("failed: {}", self.failed.join("; ")));
        }
        if !self.missing.is_empty() {
            parts.push(format!("no evidence for: {}", self.missing.join("; ")));
        }
        format!("verification did not pass -- {}", parts.join(" -- "))
    }
}

/// The `done` gate: every enforced required step has an attestation at or
/// after `since` (this verification round), produced by someone other than
/// `executing_agent`, and the newest such one for each step passed.
pub fn judge(
    required: &[RequiredStep],
    attestations: &[StepAttestation],
    executing_agent: &str,
    since: DateTime<Utc>,
) -> Verification {
    let mut out = Verification {
        passed: true,
        ..Default::default()
    };
    for step in required.iter().filter(|s| s.kind.enforced()) {
        let by = if step.required_by.is_empty() {
            String::new()
        } else {
            format!(" (required by {})", step.required_by.join(", "))
        };
        let newest = attestations
            .iter()
            .filter(|a| {
                a.step == step.step
                    && (step.kind != StepKind::Gate || a.at >= since)
                    && a.actor != executing_agent
                    && step.actor.as_deref().is_none_or(|actor| actor == a.actor)
            })
            .max_by_key(|a| a.at);
        match newest {
            None => {
                out.passed = false;
                let why = if step.kind == StepKind::Gate && step.command.is_none() {
                    " -- no gate command is declared for it"
                } else if step.kind == StepKind::Review && step.actor.is_none() {
                    " -- no independent agent is declared in this scope"
                } else {
                    ""
                };
                out.missing.push(format!("{}{by}{why}", step.step));
            }
            Some(a) if a.verdict == AttestationVerdict::Fail => {
                out.passed = false;
                let code = match a.exit_code {
                    Some(code) => format!("exit {code}"),
                    None => "did not finish".to_string(),
                };
                out.failed.push(format!("{} {code}{by}", step.step));
            }
            Some(_) => {}
        }
    }
    out
}
