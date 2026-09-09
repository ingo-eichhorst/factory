//! Check 5: database sessions against actual Herdr panes.
//!
//! The pane is the join key, never the working directory: two sessions —
//! even of different agents — can share a workspace path (this crate's task
//! brief measured four live Claude sessions sharing one on this machine), so
//! a working-directory join would attribute any one of them to any other.
//! `sessions.herdr_pane_id` (added by migration 5) is that join key on the
//! database side; `herdr pane list` — one of the two read-only Herdr
//! commands this crate is permitted to run — is the source of truth for
//! which panes currently exist.
//!
//! This crate depends on `factory-adapter` (declared in `Cargo.toml`) but
//! deliberately does not import anything from it here:
//! `factory_adapter::HerdrAccess` has no pane-listing method today, and
//! another station is actively editing that trait and `Adapter`
//! concurrently. [`LivePanes`] is this crate's own, narrower trait — a
//! signature change in `factory-adapter` cannot ripple into this file, and a
//! future `factory_adapter::HerdrAccess::pane_list` becomes a one-line
//! change to [`SystemHerdr`]'s implementation, not a rewrite of this check.
//!
//! `factory_session::Session` does not expose `herdr_pane_id` — nothing
//! writes that column yet: `factory-store`'s migration 5 backfills it to
//! `NULL` for every pre-existing row, and no adapter records it today. A
//! production run of this check will find no session to compare until a
//! future station's adapter starts recording panes; that is a real,
//! honestly-reported limit of what this check can find right now, not a
//! defect in it. It is read directly by SQL here, mirroring
//! `factory_registry::read_stored_scopes`'s own precedent for reading a
//! column another crate's public API does not surface.

use crate::{CheckName, Finding};

/// `sessions.herdr_pane_id` does not exist before migration 5
/// (`factory_store`'s `schema::V5_SCHEMA`). A released migration is never
/// edited, so this fact cannot drift the way
/// [`factory_store::latest_schema_version`] can.
pub(crate) const MIN_SCHEMA: i64 = 5;

/// The narrowest fact this crate needs from Herdr: which panes currently
/// exist.
pub trait LivePanes {
    /// The pane ids Herdr currently reports as live.
    ///
    /// # Errors
    ///
    /// A query failure — Herdr not running, the binary missing, unreadable
    /// output — as a message. Not "no panes exist": that is `Ok(vec![])`.
    fn live_pane_ids(&self) -> Result<Vec<String>, String>;
}

/// Runs the real `herdr pane list` and extracts every `pane_id`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemHerdr;

impl LivePanes for SystemHerdr {
    fn live_pane_ids(&self) -> Result<Vec<String>, String> {
        let output = std::process::Command::new("herdr")
            .args(["pane", "list"])
            .output()
            .map_err(|source| format!("could not run `herdr pane list`: {source}"))?;
        if !output.status.success() {
            return Err(format!(
                "`herdr pane list` exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(extract_pane_ids(&String::from_utf8_lossy(&output.stdout)))
    }
}

/// Every `pane_id` in `herdr pane list`'s `result.panes` array.
///
/// Navigates to that array rather than searching the whole document for a
/// key name. The difference is not pedantry: a scan for `"pane_id":"`
/// anywhere in the text also matches the same key nested under some future
/// object that is not a pane, and misses it entirely if Herdr ever emits a
/// space after the colon. Both would fail silently, and this check exists to
/// notice drift rather than to add some.
///
/// Malformed JSON yields no pane ids. That is the honest answer — doctor
/// reports "Herdr said nothing usable", which check 5 already handles — and
/// it is why this returns a `Vec` rather than a `Result` the caller would
/// have to invent a finding for.
pub(crate) fn extract_pane_ids(json: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    value["result"]["panes"]
        .as_array()
        .map(|panes| {
            panes
                .iter()
                .filter_map(|pane| pane["pane_id"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One `(session id, state, herdr_pane_id)` row for a session that has a
/// recorded pane, read directly because `factory_session::Session` does not
/// carry `herdr_pane_id` (see the module docs).
struct SessionPane {
    id: uuid::Uuid,
    state: factory_session::SessionState,
    pane_id: String,
}

fn sessions_with_panes(
    store: &factory_store::Store,
) -> Result<Vec<SessionPane>, factory_store::StoreError> {
    let mut stmt = store
        .connection()
        .prepare("SELECT id, state, herdr_pane_id FROM sessions WHERE herdr_pane_id IS NOT NULL")?;
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let state: String = row.get(1)?;
        let pane_id: String = row.get(2)?;
        Ok((id, state, pane_id))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (id, state, pane_id) = row?;
        out.push(SessionPane {
            id: uuid::Uuid::parse_str(&id)
                .unwrap_or_else(|e| panic!("sessions.id is a UUID; read {id:?}: {e}")),
            state: factory_session::SessionState::from_db_str(&state),
            pane_id,
        });
    }
    Ok(out)
}

pub(crate) fn check(
    store: &factory_store::Store,
    schema_version: i64,
    herdr: &dyn LivePanes,
    findings: &mut Vec<Finding>,
) -> Result<(), factory_store::StoreError> {
    if schema_version < MIN_SCHEMA {
        findings.push(Finding::CheckSkipped {
            check: CheckName::SessionPaneAudit,
            reason: format!(
                "database schema is {schema_version}; `sessions.herdr_pane_id` was \
                 introduced in schema {MIN_SCHEMA}"
            ),
        });
        return Ok(());
    }

    let sessions = sessions_with_panes(store)?;
    if sessions.is_empty() {
        // Nothing recorded a pane at all (today, always true in
        // production — see the module docs), so there is nothing to
        // correlate; querying Herdr would answer a question nobody asked.
        return Ok(());
    }

    let live_panes = match herdr.live_pane_ids() {
        Ok(panes) => panes,
        Err(err) => {
            findings.push(Finding::HerdrPaneQueryFailed(err));
            return Ok(());
        }
    };

    for session in sessions {
        let pane_is_live = live_panes.contains(&session.pane_id);
        if session.state.holds_lease() && !pane_is_live {
            findings.push(Finding::SessionPaneGone {
                session_id: session.id,
                pane_id: session.pane_id,
            });
        } else if !session.state.holds_lease() && pane_is_live {
            findings.push(Finding::PaneStillAliveForDeadSession {
                session_id: session.id,
                pane_id: session.pane_id,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::extract_pane_ids;

    #[test]
    fn extracts_every_pane_id_from_a_real_captured_pane_list_response() {
        // A trimmed, real shape: two panes, one with the extra agent fields
        // a working session carries, one without.
        let json = r#"{"id":"cli:pane:list","result":{"panes":[
            {"agent_status":"unknown","pane_id":"w2:p1","workspace_id":"w2"},
            {"agent":"claude","pane_id":"wE:p4","workspace_id":"wE"}
        ],"type":"pane_list"}}"#;

        assert_eq!(extract_pane_ids(json), vec!["w2:p1", "wE:p4"]);
    }

    #[test]
    fn an_empty_pane_list_extracts_nothing() {
        let json = r#"{"id":"cli:pane:list","result":{"panes":[],"type":"pane_list"}}"#;
        assert_eq!(extract_pane_ids(json), Vec::<String>::new());
    }

    /// Adversarial case named in this module's own doc comment: a terminal
    /// title that contains the literal text `"pane_id":"` must not produce a
    /// phantom id, because JSON escapes the embedded quotes and the
    /// scanner's needle then does not match.
    #[test]
    fn a_terminal_title_containing_the_needle_text_produces_no_phantom_id() {
        let json = r#"{"result":{"panes":[
            {"pane_id":"wE:p2","terminal_title":"look: \"pane_id\":\"nope\" not real"}
        ]}}"#;

        assert_eq!(extract_pane_ids(json), vec!["wE:p2"]);
    }

    #[test]
    fn a_truncated_response_with_a_dangling_key_extracts_nothing_and_does_not_panic() {
        let json = r#"{"result":{"panes":[{"pane_id":"#;
        assert_eq!(extract_pane_ids(json), Vec::<String>::new());
    }
}
