//! Plain image-build observations shared by readiness, Doctor and Inbox.
//! A failure is not proof that an older, smoke-tested image cannot run.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageBuildFailure {
    pub expected_key: String,
    pub reason: String,
    pub since: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxImageFailure {
    pub scope: String,
    pub agent: String,
    pub image: String,
    pub failure: ImageBuildFailure,
}
