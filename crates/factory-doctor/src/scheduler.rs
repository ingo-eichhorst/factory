//! Check 6: whether the scheduler job is loaded, and when it last fired.
//!
//! Station 11 (backlog §11, the shared task audit and cron dispatcher) has
//! not shipped, so there is no Factory-built cron dispatcher to check for
//! yet. But "the label Factory will use" is a decision that already exists,
//! not one to invent here: `docs/task-store-migration-plan.md` keeps
//! `com.business-factory.scheduler` as the label when
//! `launchd/com.business-factory.scheduler.plist` moves from the company
//! root into `projects/factory/launchd/` — and, checked directly
//! (`grep -rn "com\.business-factory" --include="*.rs" --include="*.md"
//! --include="*.toml"` across this repository, plus the one file under
//! `/Users/factory/business-factory/launchd/` on this machine), that is the
//! *only* `com.business-factory.*` label recorded anywhere.
//!
//! **What this machine reports today is "loaded," not "not installed."**
//! `launchctl list com.business-factory.scheduler` finds a job on this
//! machine: the pre-station-11 Python prototype
//! (`scripts/factory_tasks.py dispatch --deliver`), loaded under this same
//! label per the migration plan above, and not yet replaced. This check
//! reports whatever `launchctl` actually says, honestly, rather than
//! asserting a particular answer — once station 11 either takes over this
//! label or is given a different one, this check's true/false answer
//! changes with it, with no edit needed here.
//!
//! `last_fired` is always `None`. Neither `launchctl list` nor
//! `launchctl print` expose a last-fired timestamp — `LastExitStatus` in
//! `launchctl list`'s own output is an exit *code* from the job's last run,
//! not a time, and is deliberately not used as a stand-in for one. That is
//! the honest answer the backlog itself asks for: "today nothing would
//! reveal that a cron schedule has not fired for days," and inventing a
//! timestamp here would quietly defeat that same check.

use crate::Finding;

/// The only `com.business-factory.*` scheduler label this repository
/// documents — see the module docs for how that was established.
pub const SCHEDULER_LABEL: &str = "com.business-factory.scheduler";

/// The narrowest fact this crate needs from launchd: whether a label is
/// loaded, and its raw `launchctl list` output when it is.
pub trait LaunchdAccess {
    /// `Ok(Some(raw))` when `label` is loaded (`launchctl list <label>`
    /// exits successfully, with `raw` its stdout); `Ok(None)` when launchd
    /// reports no such service; `Err` for any other failure (the `launchctl`
    /// binary missing, an unreadable process failure).
    fn list(&self, label: &str) -> Result<Option<String>, String>;
}

/// Runs the real `launchctl list <label>`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemLaunchd;

impl LaunchdAccess for SystemLaunchd {
    fn list(&self, label: &str) -> Result<Option<String>, String> {
        let output = std::process::Command::new("launchctl")
            .args(["list", label])
            .output()
            .map_err(|source| format!("could not run `launchctl list {label}`: {source}"))?;
        if output.status.success() {
            Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
        } else {
            // Measured directly on 2026-09-09: an unknown label exits
            // non-zero (113 on this machine) with "Could not find service
            // ... in domain for port" on stderr. `launchctl`'s own exit
            // codes are not documented as stable API, so this treats any
            // non-zero exit as "not loaded" rather than pattern-matching a
            // specific number.
            Ok(None)
        }
    }
}

/// Check 6's report, always present in [`crate::DoctorReport`] regardless of
/// whether anything is wrong — mirroring [`crate::BackupsSummary`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerStatus {
    pub label: String,
    pub loaded: bool,
    /// Always `None` today — see the module docs for why that is the
    /// honest answer, not a gap.
    pub last_fired: Option<String>,
}

pub(crate) fn check(launchd: &dyn LaunchdAccess, findings: &mut Vec<Finding>) -> SchedulerStatus {
    let loaded = match launchd.list(SCHEDULER_LABEL) {
        Ok(Some(_raw)) => true,
        Ok(None) => {
            findings.push(Finding::SchedulerNotLoaded {
                label: SCHEDULER_LABEL.to_string(),
            });
            false
        }
        Err(error) => {
            findings.push(Finding::SchedulerQueryFailed {
                label: SCHEDULER_LABEL.to_string(),
                error,
            });
            false
        }
    };

    SchedulerStatus {
        label: SCHEDULER_LABEL.to_string(),
        loaded,
        last_fired: None,
    }
}
