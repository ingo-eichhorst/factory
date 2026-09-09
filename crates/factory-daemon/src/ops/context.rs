//! `context.show` (design §2.5, §7).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::envelope::ErrorBody;
use crate::errors;
use crate::handler::FactoryHandler;
use crate::server::HandlerOutcome;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    agent_name: String,
    #[serde(default)]
    task_prompt: Option<String>,
}

pub(crate) fn show(h: &FactoryHandler, scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: Payload = errors::parse_payload(&payload)?;

    let config = crate::handler::config::load(h.instance_root())?;
    let agent = crate::handler::config::find_agent(&config, scope_id, &payload.agent_name)?;

    let store = h.lock_store();
    let scope_contexts = compile_scope_chain(&store, scope_id)?;
    drop(store);

    let compiled = factory_context::compile(&scope_contexts, agent, payload.task_prompt.as_deref())
        .map_err(errors::context_error)?;

    let sources: Vec<Value> = compiled
        .sources
        .iter()
        .map(|s| {
            json!({
                "label": s.label,
                "path": s.path.as_ref().map(|p| p.display().to_string()),
                "bytes": s.bytes,
            })
        })
        .collect();

    h.success(json!({ "text": compiled.text, "sources": sources }), false)
}

/// The root-to-leaf [`factory_context::ScopeContext`] chain ending at
/// `scope_id`, built from the registered `scopes` table (see
/// `handler::scope_chain`'s module docs).
pub(crate) fn compile_scope_chain(
    store: &factory_store::Store,
    scope_id: uuid::Uuid,
) -> Result<Vec<factory_context::ScopeContext>, ErrorBody> {
    let chain = crate::handler::scope_chain::root_to_leaf(store, scope_id)?;
    chain
        .into_iter()
        .map(|row| {
            let canonical_path = row.canonical_path.ok_or_else(|| {
                errors::err(
                    "unavailable.scope_path_missing",
                    format!(
                        "scope `{}` has no currently-resolvable canonical path; \
                         run `scope.reconcile` to see the drift",
                        row.name
                    ),
                )
            })?;
            Ok(factory_context::ScopeContext {
                scope_name: row.name,
                context_file: std::path::PathBuf::from(canonical_path).join("AGENTS.md"),
            })
        })
        .collect()
}
