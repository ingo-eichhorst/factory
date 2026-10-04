//! Harness health (`#131`): whether the program an agent adapter starts
//! actually starts, checked before a task is handed to it.
//!
//! The adapter only *declares* its probe ([`HealthProbe`], from
//! `Agent::health_probe`); the daemon runs it, with a timeout, so a probe that
//! hangs or a program that never answers can never stall or take down the
//! daemon -- and a plugin needs no new wire call to have none.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The command that shows a harness starts at all, `<harness> --version` for
/// every built-in one. The first element is the program: a bare name is
/// looked up on the daemon's `PATH` the way `execvp` would, a path containing
/// a `/` is used as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthProbe {
    pub command: Vec<String>,
}

impl HealthProbe {
    /// `<program> --version`.
    pub fn version(program: impl Into<String>) -> Self {
        Self {
            command: vec![program.into(), "--version".into()],
        }
    }

    pub fn program(&self) -> &str {
        self.command.first().map(String::as_str).unwrap_or_default()
    }
}

/// Where a harness's last probe left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessState {
    /// It answered its probe, exit status zero, in time.
    Healthy,
    /// It did not answer in time, exited non-zero, or could not be found.
    /// Tasks for it are blocked before a run starts, never failed.
    Unhealthy,
    /// Nothing has asked for it since the daemon started, so it has not been
    /// probed. Nothing is ever probed just to fill a page in.
    Unprobed,
}

impl HarnessState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Unhealthy => "unhealthy",
            Self::Unprobed => "unprobed",
        }
    }
}

/// One harness binary, as `factory infra` and the L1 page show it, and as the
/// Operations "needs a human" list reads it when it is unhealthy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessRow {
    /// The harness, as the adapter names it to the runtime (`codex`,
    /// `claude`, ...).
    pub harness: String,
    /// The binary the probe resolved to, or the bare program name when it
    /// was not found on the daemon's `PATH`.
    pub binary: String,
    pub state: HarnessState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
    /// When the current unhealthy stretch began -- the first failed probe
    /// since it last answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unhealthy_since: Option<DateTime<Utc>>,
    /// What it answered (its first line), when healthy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Why it is unhealthy, naming the binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The command a person runs to repair it, when unhealthy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<String>,
    /// Tasks blocked on it before a run started, waiting for it to answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<HeldTask>,
    /// What the opt-in automatic repair last did, when it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_repair: Option<String>,
}

/// A task held on an unhealthy harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldTask {
    pub task_id: String,
    pub scope: String,
    pub title: String,
}

/// The journal kind a task gets when it is blocked on its harness before a
/// run started. Also what the daemon searches for to release those tasks once
/// the harness answers again -- the one record of the hold, so it survives a
/// restart without a table of its own.
pub const HELD_ENTRY: &str = "harness_unhealthy";
/// The journal kind a held task gets when its harness answers again and it is
/// dispatched.
pub const RELEASED_ENTRY: &str = "harness_recovered";

/// The command that repairs a harness, as a blocked reason names it. The
/// script lives in the Factory repository (`scripts/repair-harness`); an
/// instance that set `daemon.harness_health.repair_script` gets that path.
pub fn repair_command(repair_script: Option<&str>, harness: &str) -> String {
    let script = repair_script.unwrap_or("scripts/repair-harness");
    format!("{script} {harness}")
}

/// The reason a blocked task carries: which binary, what happened, and how
/// to fix it.
pub fn blocked_reason(harness: &str, problem: &str, repair: &str) -> String {
    format!("{harness} does not start: {problem}. A person can repair it with `{repair}`")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reason_names_the_binary_and_the_repair_command() {
        let repair = repair_command(None, "codex");
        let why = blocked_reason(
            "codex",
            "`/opt/homebrew/bin/codex --version` did not answer in 10s",
            &repair,
        );
        assert!(why.contains("/opt/homebrew/bin/codex --version"));
        assert!(why.contains("`scripts/repair-harness codex`"));
    }

    #[test]
    fn a_configured_script_is_named_by_its_path() {
        assert_eq!(
            repair_command(Some("/opt/factory/repair-harness"), "claude"),
            "/opt/factory/repair-harness claude"
        );
    }

    #[test]
    fn a_row_round_trips_and_omits_what_it_does_not_have() {
        let row = HarnessRow {
            harness: "codex".into(),
            binary: "/opt/homebrew/bin/codex".into(),
            state: HarnessState::Unprobed,
            checked_at: None,
            unhealthy_since: None,
            version: None,
            reason: None,
            repair: None,
            held: Vec::new(),
            auto_repair: None,
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["state"], "unprobed");
        assert!(json.get("held").is_none());
        let back: HarnessRow = serde_json::from_value(json).unwrap();
        assert_eq!(back, row);
    }
}
