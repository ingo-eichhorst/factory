//! The runtime-owned cumulative session usage contract (schema 1).
//! Parsing drops non-usage fields; run snapshots, deltas and allocation belong
//! to L4, not to this runtime observer. Missing counts remain unknown.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The contract version this Factory reads.
pub const USAGE_SCHEMA: u32 = 1;

/// Tokens by type. Each count is `None` when the observer could not see it,
/// never 0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    #[serde(default)]
    pub input: Option<u64>,
    #[serde(default)]
    pub output: Option<u64>,
    #[serde(default)]
    pub cache_read: Option<u64>,
    #[serde(default)]
    pub cache_write: Option<u64>,
}

impl TokenCounts {
    /// Every token of every type, or None if any count is unknown.
    pub fn total(&self) -> Option<u64> {
        [self.input, self.output, self.cache_read, self.cache_write]
            .iter()
            .try_fold(0u64, |sum, value| value.map(|value| sum + value))
    }
}

/// What the tokens cost, in API-equivalent US dollars, and the price table
/// that said so.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageCost {
    #[serde(default)]
    pub usd: Option<f64>,
    /// e.g. `litellm@2026-09-01`. Kept with every number it priced, because a
    /// price table changes and history must not be re-priced silently.
    #[serde(default)]
    pub pricing_source: Option<String>,
}

/// One subagent a harness session spawned. Listed apart from its parent,
/// which is read as *excluding* it by the process layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentUsage {
    pub session_id: String,
    #[serde(default)]
    pub tokens: TokenCounts,
    #[serde(default)]
    pub cost: UsageCost,
}

/// One subscription rate-limit window, as the provider reports it. Stored
/// with every snapshot and allocated across the runs active between readings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitWindow {
    #[serde(default)]
    pub window_minutes: Option<u32>,
    #[serde(default)]
    pub used_percent: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimit {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub plan_type: Option<String>,
    /// How sure the observer is that this window belongs to this session's
    /// account -- `confirmed`, or something weaker.
    #[serde(default)]
    pub attribution_quality: Option<String>,
    #[serde(default)]
    pub windows: Vec<RateLimitWindow>,
}

/// One harness session the runtime saw in a Factory session -- a Claude Code
/// or Codex conversation running in a herdr pane, say. Cumulative since that
/// harness session began.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessUsage {
    pub session_id: String,
    /// Which harness: `claude-code`, `codex`, ...
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tokens: TokenCounts,
    #[serde(default)]
    pub cost: UsageCost,
    #[serde(default)]
    pub elapsed_seconds: Option<f64>,
    #[serde(default)]
    pub active_seconds: Option<f64>,
    #[serde(default)]
    pub subagents: Vec<SubagentUsage>,
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
    /// Why a field is `null`, keyed by its dotted path (`tokens.cache_write`)
    /// -- the contract's "null with a reason". Optional on the wire: a
    /// `null` with no reason given is still unknown, just unexplained.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

/// The runtime's answer to "what has this session used?" -- the #117
/// contract, schema 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUsage {
    pub schema: u32,
    /// The runtime's own name for where it looked: herdr's pane id. Called
    /// `pane_id` on the wire, since that is the contract's word; kept under
    /// a runtime-neutral one here.
    #[serde(default, alias = "pane_id", skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sessions: Vec<HarnessUsage>,
}

impl SessionUsage {
    /// Read a runtime's answer. Refuses a schema this Factory does not know
    /// rather than guessing at what its fields mean; everything outside the
    /// typed fields above is dropped, which is what keeps transcript text a
    /// plugin might send from ever being stored.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(text.trim()).map_err(|e| format!("usage is not JSON: {e}"))?;
        match value.get("schema").and_then(serde_json::Value::as_u64) {
            Some(s) if s == u64::from(USAGE_SCHEMA) => {}
            Some(s) => {
                return Err(format!(
                    "usage schema {s} is not one this Factory reads (it reads {USAGE_SCHEMA})"
                ))
            }
            None => return Err("usage carries no schema version".into()),
        }
        serde_json::from_value(value)
            .map_err(|e| format!("usage does not match schema {USAGE_SCHEMA}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_parsing_drops_transcripts_and_preserves_unknowns_and_aliases() {
        let parsed = SessionUsage::parse(
            &json!({
                "schema": 1, "pane_id": "opaque", "transcript": "not usage",
                "sessions": [{
                    "session_id": "conversation", "message": "never stored",
                    "tokens": {"input": 10, "output": null},
                    "cost": {"usd": null, "pricing_source": "snapshotted"},
                    "unavailable": {"tokens.output": "observer could not see it"},
                    "rate_limit": {"provider": "provider", "windows": [{"window_minutes": 300}]}
                }]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(parsed.handle.as_deref(), Some("opaque"));
        let tokens = &parsed.sessions[0].tokens;
        assert_eq!(tokens.input, Some(10));
        assert_eq!(tokens.output, None);
        assert_eq!(tokens.cache_read, None);
        assert_eq!(tokens.total(), None);
        let wire = serde_json::to_value(parsed).unwrap();
        assert!(wire.get("transcript").is_none() && wire.get("pane_id").is_none());
        assert!(wire["sessions"][0].get("message").is_none());
        assert_eq!(wire["sessions"][0]["cost"]["pricing_source"], "snapshotted");
        assert!(wire["sessions"][0]["rate_limit"]["windows"][0]["used_percent"].is_null());
    }

    #[test]
    fn an_absent_or_unsupported_schema_is_refused_by_l3_itself() {
        for invalid in [
            "not JSON",
            "[]",
            "{}",
            "{\"schema\":2}",
            "{\"schema\":\"1\"}",
        ] {
            assert!(SessionUsage::parse(invalid).is_err(), "{invalid}");
        }
        let known = SessionUsage::parse("{\"schema\":1}").unwrap();
        assert_eq!(known.schema, USAGE_SCHEMA);
        assert!(known.sessions.is_empty());
    }

    #[test]
    fn token_totals_require_every_count_not_only_the_known_subset() {
        for mask in 0..16 {
            let count = |bit, value| (mask & (1 << bit) != 0).then_some(value);
            let tokens = TokenCounts {
                input: count(0, 2),
                output: count(1, 3),
                cache_read: count(2, 5),
                cache_write: count(3, 7),
            };
            assert_eq!(tokens.total(), (mask == 15).then_some(17));
        }
        assert_eq!(
            TokenCounts {
                input: Some(0),
                output: Some(0),
                cache_read: Some(0),
                cache_write: Some(0),
            }
            .total(),
            Some(0)
        );
    }
}
