//! The one fixture `tests/harness_crash.rs` and `tests/harness_change.rs`
//! need that `tests/common/mod.rs` does not already provide: a session
//! seeded directly into `starting`, `running`, or `disconnected` — a state
//! the caller chooses, rather than `common::seed_disconnected_session`'s
//! hardcoded `disconnected` (built for [`crate::reconnect`]'s own restart
//! classes, which only ever start from an already-`disconnected` row).
//! Everything else these two test files need — `open_store`, `uid`,
//! `seed_scope`, `seed_task`, the `session_state`/`task_status`/
//! `every_lease_released` readers, and `FakeAdapter` — comes from `mod
//! common;` directly.
//!
//! Named `harness_common`, not `common`: `tests/` is a flat namespace shared
//! by every test binary in this crate, and a second `mod common;` pointing
//! at a different path would silently replace whichever file was written
//! first — the identical collision `tests/restore_common/mod.rs`'s own doc
//! comment already explains for its own reason to exist as a separate file.

#![allow(dead_code)]

use std::path::Path;

use factory_store::Store;

/// Insert a session directly in `state`, with the matching open
/// `workspace_leases` row when `state` holds a lease — mirrors
/// `tests/common/mod.rs::seed_disconnected_session` exactly, except `state`
/// is the caller's choice rather than fixed to `disconnected`, and `id` is
/// supplied by the caller (via `common::uid`) rather than derived from a
/// seed here, so this file does not need its own copy of that helper.
pub fn seed_session(
    store: &mut Store,
    id: uuid::Uuid,
    scope_id: uuid::Uuid,
    agent_name: &str,
    workspace_path: &Path,
    state: &str,
    herdr_pane_id: Option<&str>,
) {
    let workspace_path = workspace_path.to_string_lossy().into_owned();
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO sessions \
         (id, scope_id, agent_name, workspace_path, state, herdr_pane_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            id.to_string(),
            scope_id.to_string(),
            agent_name,
            &workspace_path,
            state,
            herdr_pane_id,
        ),
    )
    .expect("insert session");
    if matches!(state, "starting" | "running" | "disconnected") {
        tx.execute(
            "INSERT INTO workspace_leases (session_id, canonical_workspace_path) VALUES (?1, ?2)",
            (id.to_string(), &workspace_path),
        )
        .expect("insert lease");
    }
    tx.commit().expect("commit");
}
