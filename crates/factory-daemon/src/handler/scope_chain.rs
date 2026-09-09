//! Reading one `scopes` row, and walking the chain of them root-to-leaf for
//! [`factory_context::compile`] (design §2.5: company context, ancestor scope
//! contexts, current scope context, agent definition, task prompt — in that
//! order).
//!
//! Reads the *registered* `scopes` table, not a fresh
//! `factory_registry::resolve` over the configuration: backlog §3's own
//! acceptance criterion is "[t]he registry supplies **canonical** paths to
//! the context compiler," and the registry's canonical answer for what is
//! currently registered is the projected table (`factory_registry::apply`'s
//! output), not a recomputation over the configuration this crate would
//! otherwise have to reload on every `agent.start` and `context.show`.

use crate::envelope::ErrorBody;
use crate::errors;

fn store_err(e: impl std::fmt::Display) -> ErrorBody {
    errors::err("internal.store_error", e.to_string())
}

pub(crate) struct ScopeRow {
    pub name: String,
    pub canonical_path: Option<String>,
    pub parent_id: Option<uuid::Uuid>,
}

/// One `scopes` row by id.
///
/// # Errors
///
/// `not_found.scope` if `scope_id` names no registered row.
pub(crate) fn row(
    store: &factory_store::Store,
    scope_id: uuid::Uuid,
) -> Result<ScopeRow, ErrorBody> {
    let mut stmt = store
        .connection()
        .prepare("SELECT name, canonical_path, parent_id FROM scopes WHERE id = ?1")
        .map_err(store_err)?;
    let mut rows = stmt
        .query_map([scope_id.to_string()], |r| {
            let name: String = r.get(0)?;
            let canonical_path: Option<String> = r.get(1)?;
            let parent_id: Option<String> = r.get(2)?;
            Ok((name, canonical_path, parent_id))
        })
        .map_err(store_err)?;

    let Some(found) = rows.next() else {
        return Err(errors::err(
            "not_found.scope",
            format!("scope `{scope_id}` is not registered"),
        ));
    };
    let (name, canonical_path, parent_id) = found.map_err(store_err)?;
    let parent_id = parent_id
        .map(|p| {
            uuid::Uuid::parse_str(&p).map_err(|e| {
                errors::err(
                    "internal.store_error",
                    format!("scopes.parent_id is a UUID; read {p:?}: {e}"),
                )
            })
        })
        .transpose()?;

    Ok(ScopeRow {
        name,
        canonical_path,
        parent_id,
    })
}

/// The scope chain root-to-leaf, ending at `scope_id`, per
/// [`factory_context::compile`]'s required order.
///
/// # Errors
///
/// `not_found.scope` if any scope in the chain (including `scope_id` itself)
/// is not registered. `internal.cyclic_parentage` if walking `parent_id`
/// revisits a scope already seen — see `factory_registry::RegistryError::CyclicParentage`'s
/// own doc comment for why the schema alone cannot prevent a hand-edited
/// cycle.
pub(crate) fn root_to_leaf(
    store: &factory_store::Store,
    scope_id: uuid::Uuid,
) -> Result<Vec<ScopeRow>, ErrorBody> {
    let mut chain = Vec::new();
    let mut current = Some(scope_id);
    let mut visited = std::collections::HashSet::new();

    while let Some(id) = current {
        if !visited.insert(id) {
            return Err(errors::err(
                "internal.cyclic_parentage",
                format!(
                    "scope `{id}`'s parent_id cycles back to a scope already seen while walking its ancestry"
                ),
            ));
        }
        let found = row(store, id)?;
        current = found.parent_id;
        chain.push(found);
    }
    chain.reverse();
    Ok(chain)
}
