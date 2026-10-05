//! L4 generic execution plan and verification; no policy catalogue/compiler.
use chrono::{DateTime, Utc};
pub use factory_kernel::StepKind;
use serde::{Deserialize, Serialize};

pub use factory_kernel::{ControlPlan, PlanStep, Waiver, DEFAULT_CATEGORY, ANY_CATEGORY, effective_category, is_name, check_category};

/// Who a gate's attestation names as having run it. The daemon, never the
/// agent whose work is being judged -- see [`judge`].
pub const GATE_ACTOR: &str = "factory-daemon";

/// L4 view over the canonical plain plan; compilation stays in L5.
pub trait ControlPlanExt {
    fn enforced(&self) -> impl Iterator<Item = &PlanStep>;
}
impl ControlPlanExt for ControlPlan {
    /// The steps the daemon injects and judges.
    fn enforced(&self) -> impl Iterator<Item = &PlanStep> {
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
