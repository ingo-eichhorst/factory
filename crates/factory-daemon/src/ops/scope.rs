//! `scope.add`, `scope.reconcile`, `scope.list` (design §7; ADR 0016).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::errors;
use crate::handler::FactoryHandler;
use crate::server::HandlerOutcome;

/// Always refuses — see `handler::config`'s module docs for why this crate
/// cannot safely write `.factory/config.yaml`, and `lib.rs`'s payload table
/// for the operator remedy this message names.
pub(crate) fn add(_h: &FactoryHandler, _scope_id: uuid::Uuid, _payload: Value) -> HandlerOutcome {
    Err(errors::err(
        "internal.scope_add_unsupported",
        "factory-daemon has no YAML writer for `.factory/config.yaml`, and ADR 0009 requires \
         that unknown top-level keys be preserved, which a hand-rolled writer cannot safely \
         guarantee. Edit `.factory/config.yaml` directly to add the scope, then call \
         `scope.reconcile` with `apply: true` to project it.",
    ))
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct ReconcilePayload {
    apply: bool,
}

pub(crate) fn reconcile(
    h: &FactoryHandler,
    _scope_id: uuid::Uuid,
    payload: Value,
) -> HandlerOutcome {
    let payload: ReconcilePayload = errors::parse_payload(&payload)?;

    let config = crate::handler::config::load(h.instance_root())?;
    let scopes =
        factory_registry::resolve(&config, h.instance_root()).map_err(errors::registry_error)?;

    let mut store = h.lock_store();
    let report = factory_registry::reconcile(&store, &scopes).map_err(errors::registry_error)?;
    let clean = report.is_clean();
    let drift: Vec<Value> = report.items.iter().map(drift_to_json).collect();

    let mut applied_json = Vec::new();
    let mut mutated = false;
    if payload.apply {
        let applied = factory_registry::apply(&mut store, &scopes, &report)
            .map_err(errors::registry_error)?;
        mutated = !applied.is_empty();
        applied_json = applied.iter().map(drift_to_json).collect();
    }

    h.success(
        json!({ "clean": clean, "drift": drift, "applied": applied_json }),
        mutated,
    )
}

pub(crate) fn list(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {}
    let _: Payload = errors::parse_payload(&payload)?;

    let store = h.lock_store();
    let mut stmt = store
        .connection()
        .prepare(
            "SELECT id, name, declared_path, canonical_path, git, parent_id \
             FROM scopes ORDER BY name, id",
        )
        .map_err(store_err)?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let declared_path: String = row.get(2)?;
            let canonical_path: Option<String> = row.get(3)?;
            let git: Option<String> = row.get(4)?;
            let parent_id: Option<String> = row.get(5)?;
            Ok(json!({
                "id": id,
                "name": name,
                "declared_path": declared_path,
                "canonical_path": canonical_path,
                "git": git,
                "parent_id": parent_id,
            }))
        })
        .map_err(store_err)?;

    let mut scopes = Vec::new();
    for row in rows {
        scopes.push(row.map_err(store_err)?);
    }

    h.success(json!({ "scopes": scopes }), false)
}

fn store_err(e: impl std::fmt::Display) -> crate::envelope::ErrorBody {
    errors::err("internal.store_error", e.to_string())
}

fn drift_to_json(d: &factory_registry::Drift) -> Value {
    use factory_registry::Drift as D;
    match d {
        D::DeclaredNotProjected { id, name } => json!({
            "kind": "declared_not_projected", "id": id.to_string(), "name": name,
        }),
        D::ProjectedNotDeclared { id, name } => json!({
            "kind": "projected_not_declared", "id": id.to_string(), "name": name,
        }),
        D::PathChangedSameIdentity { id, name, from, to } => json!({
            "kind": "path_changed_same_identity", "id": id.to_string(), "name": name,
            "from": from.display().to_string(), "to": to.display().to_string(),
        }),
        D::PathChangedDifferentIdentity { id, name, from, to } => json!({
            "kind": "path_changed_different_identity", "id": id.to_string(), "name": name,
            "from": from.display().to_string(), "to": to.display().to_string(),
        }),
        D::MissingPath { id, name, path } => json!({
            "kind": "missing_path", "id": id.to_string(), "name": name,
            "path": path.display().to_string(),
        }),
        D::UnreadableContext { id, name, path } => json!({
            "kind": "unreadable_context", "id": id.to_string(), "name": name,
            "path": path.display().to_string(),
        }),
        D::NameChanged { id, from, to } => json!({
            "kind": "name_changed", "id": id.to_string(), "from": from, "to": to,
        }),
        D::GitChanged { id, name, from, to } => json!({
            "kind": "git_changed", "id": id.to_string(), "name": name, "from": from, "to": to,
        }),
        D::DeclaredPathChanged { id, name, from, to } => json!({
            "kind": "declared_path_changed", "id": id.to_string(), "name": name,
            "from": from.display().to_string(), "to": to.display().to_string(),
        }),
        D::ParentChanged { id, name, from, to } => json!({
            "kind": "parent_changed", "id": id.to_string(), "name": name,
            "from": from.map(|u| u.to_string()), "to": to.map(|u| u.to_string()),
        }),
    }
}
