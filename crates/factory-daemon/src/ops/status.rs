//! `daemon.status` (design §7's `factory status`).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::errors;
use crate::handler::FactoryHandler;
use crate::server::HandlerOutcome;

pub(crate) fn daemon_status(
    h: &FactoryHandler,
    _scope_id: uuid::Uuid,
    payload: Value,
) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {}
    let _: Payload = errors::parse_payload(&payload)?;

    let store = h.lock_store();
    let schema_version = store
        .schema_version()
        .map_err(|e| errors::err("internal.store_error", e.to_string()))?;
    let db_path = store.path().display().to_string();

    h.success(
        json!({
            "schema_version": schema_version,
            "database_path": db_path,
            "socket_path": crate::socket_path(h.instance_root()).display().to_string(),
            "lock_path": crate::lock_path(h.instance_root()).display().to_string(),
        }),
        false,
    )
}
