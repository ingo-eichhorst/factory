//! Harness health (`#131`): whether the program an agent adapter starts
//! actually starts, checked before a task is handed to it.
//!
//! The adapter only *declares* its probe ([`HealthProbe`], from
//! `Agent::health_probe`); the daemon runs it, with a timeout, so a probe that
//! hangs or a program that never answers can never stall or take down the
//! daemon -- and a plugin needs no new wire call to have none.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The Codex model fact (`#340`) is L1's; its plain schema is L0's, named here
/// beside the harness rows it is shown with.
pub use factory_kernel::{CodexAgentModel, CodexModelSource, CodexModelsFact, CodexServedModel};

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

/// Why a codex agent is refused before dispatch (`#340`): the backend does not
/// serve the model it would ask for to the installed client. `None` unless that
/// is known -- an unknown list, model or verdict refuses nothing. `row` is the
/// agent's own row; a bare `codex` adapter with no declaration has none and asks
/// for the host default.
pub fn unserved_model_reason(
    fact: &CodexModelsFact,
    row: Option<&CodexAgentModel>,
) -> Option<String> {
    let (model, source) = match row {
        Some(row) if row.served == Some(false) => (row.model.as_deref()?, row.source),
        Some(_) => return None,
        None if fact.default_served == Some(false) => (fact.default_model.as_deref()?, CodexModelSource::ConfigDefault),
        None => return None,
    };
    let from = match source {
        CodexModelSource::Declared => "declared in the agent's args",
        CodexModelSource::Profile => "from its codex profile",
        CodexModelSource::ConfigDefault => "`model` in the host's codex config.toml",
        CodexModelSource::CodexDefault => "codex's own default",
    };
    let version = fact.client_version.as_deref().unwrap_or("the installed client");
    let served: Vec<&str> = fact.served.iter().filter(|m| !m.hidden).map(|m| m.slug.as_str()).collect();
    Some(format!(
        "model {model} ({from}) is not served to codex {version} with this host's ChatGPT login; served: {}. \
         Declare a served model with `--model <name>` in the agent's args, set `model` in the codex config.toml, \
         or upgrade codex. The run was not started",
        served.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unserved_model_names_itself_where_it_came_from_and_what_is_served() {
        let row = |served: Option<bool>| CodexAgentModel {
            scope: "factory".into(),
            agent: "codex".into(),
            model: Some("gpt-6.1-sol".into()),
            source: CodexModelSource::ConfigDefault,
            served,
            note: None,
        };
        let fact = CodexModelsFact {
            client_version: Some("0.157.0".into()),
            served: vec![
                CodexServedModel { slug: "gpt-6-astra".into(), hidden: false },
                CodexServedModel { slug: "gpt-6-sol".into(), hidden: false },
                CodexServedModel { slug: "gpt-reserve".into(), hidden: true },
            ],
            default_model: Some("gpt-6.1-sol".into()),
            default_served: Some(false),
            ..Default::default()
        };
        let why = unserved_model_reason(&fact, Some(&row(Some(false)))).unwrap();
        assert!(why.contains("gpt-6.1-sol") && why.contains("config.toml") && why.contains("0.157.0"), "{why}");
        assert!(why.contains("served: gpt-6-astra, gpt-6-sol."), "{why}");
        assert!(!why.contains("gpt-reserve"), "hidden models are not offered: {why}");
        assert_eq!(unserved_model_reason(&fact, Some(&row(Some(true)))), None);
        assert_eq!(unserved_model_reason(&fact, Some(&row(None))), None, "unknown refuses nothing");
        assert!(unserved_model_reason(&fact, None).unwrap().contains("gpt-6.1-sol"), "the bare adapter asks for the default");
        let unknown = CodexModelsFact { default_served: None, ..fact };
        assert_eq!(unserved_model_reason(&unknown, None), None);
    }

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
