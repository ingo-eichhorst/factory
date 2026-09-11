//! `memory.add`, `memory.list` (design §7, backlog §12; ADR 0022).
//!
//! Scope-local memory reuses the same five-step write ordering
//! `ops::knowledge::write` uses (ADR 0022 decision 11: one primitive, not
//! two near-identical ones) — see that module's own doc comment for why the
//! order is fixed. The one thing genuinely new here is decision 12: the
//! on-disk directory is named by the scope's own `name`, and `scopes.name`
//! carries no UNIQUE constraint, so two registered scopes can share one —
//! [`scope_name_and_collisions`] is where that is caught, for both `add` and
//! `list`, before either ever touches a directory.

use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::envelope::ErrorBody;
use crate::errors;
use crate::handler::FactoryHandler;
use crate::server::HandlerOutcome;

/// `<instance_root>/.factory/memory/` — every scope's memory lives directly
/// beneath this, one directory per scope name (design §4's tree).
fn memory_root(instance_root: &std::path::Path) -> std::path::PathBuf {
    instance_root.join(".factory").join("memory")
}

/// `scopes.name` for `scope_id`, and every scope id that shares that exact
/// name (`scope_id` itself included) — ADR 0022 decision 12's own check.
///
/// A caller with more than one row back cannot safely resolve a memory
/// directory: `.factory/memory/<name>/` would hold two scopes' entries
/// merged into one pile with no way to tell them apart afterwards. Neither
/// `add` nor `list` writes or reads a single file until this has run.
///
/// # Errors
///
/// `not_found.scope` if `scope_id` names no row at all — a raw JSON-RPC
/// caller can send any id, and the registry is what this crate defers to for
/// "does this scope exist," the same stance `handler::config::find_agent`
/// takes.
fn scope_name_and_collisions(
    store: &factory_store::Store,
    scope_id: uuid::Uuid,
) -> Result<(String, Vec<uuid::Uuid>), ErrorBody> {
    let mut own = store
        .connection()
        .prepare("SELECT name FROM scopes WHERE id = ?1")
        .map_err(errors::store_error)?;
    let mut rows = own
        .query_map([scope_id.to_string()], |row| row.get::<_, String>(0))
        .map_err(errors::store_error)?;
    let name = match rows.next() {
        Some(name) => name.map_err(errors::store_error)?,
        None => {
            return Err(errors::err(
                "not_found.scope",
                format!("scope `{scope_id}` is not registered"),
            ));
        }
    };
    drop(rows);
    drop(own);

    let mut stmt = store
        .connection()
        .prepare("SELECT id FROM scopes WHERE name = ?1 ORDER BY id")
        .map_err(errors::store_error)?;
    let rows = stmt
        .query_map([&name], |row| row.get::<_, String>(0))
        .map_err(errors::store_error)?;

    let mut ids = Vec::new();
    for row in rows {
        let id = row.map_err(errors::store_error)?;
        ids.push(
            uuid::Uuid::parse_str(&id)
                .unwrap_or_else(|e| panic!("scopes.id is a UUID; read {id:?}: {e}")),
        );
    }
    Ok((name, ids))
}

/// The refusal decision 12 asks for, naming which scopes collided so an
/// operator can actually fix it (rename one of them in `config.yaml`).
///
/// `message` already spells out `name` and every colliding id in prose —
/// that is what a person reads. `details` carries the same two facts
/// structured (`scope_name`, `scope_ids`) for a machine reader that would
/// rather not parse the sentence — this crate's own `ErrorBody` keeps that
/// on the wire regardless of how any particular client renders it. What
/// changed for station 12's drill is `factory-cli`'s `rpc::send`, which used
/// to also echo `details` back as a raw JSON parenthetical appended to the
/// human-facing line; it no longer does, so this function does not need to
/// withhold `details` to keep the printed text clean.
fn ambiguous_scope_name(name: &str, collisions: &[uuid::Uuid]) -> ErrorBody {
    let ids: Vec<String> = collisions.iter().map(ToString::to_string).collect();
    errors::err_with_details(
        "conflict.ambiguous_scope_name",
        format!(
            "scope name `{name}` is registered for {n} scopes ({ids}); a memory directory named \
             by scope would merge their entries into one unrecoverable pile\n  help: rename one \
             of the colliding scopes in `.factory/config.yaml` so each has a unique name",
            n = collisions.len(),
            ids = ids.join(", "),
        ),
        json!({ "scope_name": name, "scope_ids": ids }),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddPayload {
    /// Minted by the CLI, mirroring every other caller-supplied id in this
    /// crate's payload table (`task.send`'s `task_id`, `agent.start`'s
    /// `session_id`).
    #[serde(with = "crate::serde_uuid::required")]
    entry_id: uuid::Uuid,
    /// The entry's text, read from standard input by the CLI. Refused when
    /// empty or whitespace-only by
    /// [`factory_knowledge::memory::stage_entry`] itself.
    entry: String,
}

/// `memory add --scope <scope>`. `scope_id` is the envelope's own field —
/// the target scope, exactly the stance `agent.start` already takes toward
/// it — never a payload field, so a raw JSON-RPC caller cannot claim to
/// write into a scope's memory while addressing the request to a different
/// one.
pub(crate) fn add(h: &FactoryHandler, scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: AddPayload = errors::parse_payload(&payload)?;

    let mut store = h.lock_store();
    let (scope_name, collisions) = scope_name_and_collisions(&store, scope_id)?;
    if collisions.len() > 1 {
        return Err(ambiguous_scope_name(&scope_name, &collisions));
    }

    // Second resolution, not `to_rfc3339`'s default microseconds — see
    // `ops::knowledge::write`'s own comment on its `updated` stamp; the
    // reasoning is identical, and this is the other of the two fields
    // ADR 0022 decision 13's stamping stance covers.
    let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let root = memory_root(h.instance_root());

    // Step 1: stage.
    let staged = factory_knowledge::memory::stage_entry(
        &root,
        &scope_name,
        &payload.entry,
        payload.entry_id,
        &created_at,
    )
    .map_err(errors::knowledge_error)?;
    let relative_path = staged
        .path()
        .strip_prefix(h.instance_root())
        .unwrap_or(staged.path())
        .display()
        .to_string();

    // Step 2: open the transaction.
    let tx = store.transaction().map_err(errors::store_error)?;

    // Step 3: the provenance row. `scope_id` is required here —
    // `durable_writes`'s own CHECK pairs it with `kind = 'memory'`.
    factory_store::durable::append(
        &tx,
        factory_store::durable::WriteKind::Memory,
        &payload.entry_id.to_string(),
        &relative_path,
        Some(&scope_id.to_string()),
        None,
        None,
    )
    .map_err(errors::store_error)?;

    // Step 4: the rename.
    let final_path = staged.commit().map_err(errors::knowledge_error)?;

    // Step 5: only now does the row become durable.
    tx.commit().map_err(errors::store_error)?;

    h.success(
        json!({
            "id": payload.entry_id.to_string(),
            "scope": scope_name,
            "path": final_path
                .strip_prefix(h.instance_root())
                .unwrap_or(&final_path)
                .display()
                .to_string(),
            "created_at": created_at,
        }),
        true,
    )
}

pub(crate) fn list(h: &FactoryHandler, scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {}
    let _: Payload = errors::parse_payload(&payload)?;

    let store = h.lock_store();
    let (scope_name, collisions) = scope_name_and_collisions(&store, scope_id)?;
    if collisions.len() > 1 {
        return Err(ambiguous_scope_name(&scope_name, &collisions));
    }

    let root = memory_root(h.instance_root());
    let entries = factory_knowledge::memory::list_entries(&root, &scope_name)
        .map_err(errors::knowledge_error)?;

    let entries_json: Vec<Value> = entries
        .iter()
        .map(|e| {
            json!({
                "id": e.id.to_string(),
                "created_at": e.created_at,
                "text": e.text,
            })
        })
        .collect();

    h.success(
        json!({ "scope": scope_name, "entries": entries_json }),
        false,
    )
}
