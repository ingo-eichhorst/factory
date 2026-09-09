//! Loading the instance configuration, and why this crate never writes it.
//!
//! `factory_config` (off limits to this crate — see the top-level task
//! brief) exposes exactly two entry points, [`factory_config::load`] and
//! [`factory_config::parse`], and both only read. ADR 0016 decision 1
//! requires `factory scope add` to append an entry to `.factory/config.yaml`
//! "because a human asked it to," but this crate has no dependency capable of
//! writing YAML back out safely: `factory-daemon`'s `Cargo.toml` carries
//! `serde` and `serde_json`, never `serde-saphyr` (the library
//! `factory_config` itself uses to parse — ADR 0009), and this crate's brief
//! forbids `cargo add`.
//!
//! A hand-rolled text append is not a safe substitute. ADR 0009's schema
//! policy requires that "unknown top-level keys are preserved and ignored" —
//! the live `assistant` scope's own file carries a `runtime:` block owned by
//! a different tool — and a serializer this crate improvised (or a
//! string-splice into the raw file text) has no way to honour that guarantee
//! without re-implementing a YAML-preserving editor, which is exactly the
//! kind of rule this crate exists to wire to, not invent.
//!
//! `scope.add` therefore returns `internal.scope_add_unsupported`
//! unconditionally (see `ops::scope::add`) rather than attempting a partial
//! implementation that silently risks an operator's configuration file. This
//! is reported, not solved, per the task brief's own instruction for exactly
//! this situation.
//!
//! Every other operation that needs the configuration calls [`load`] below,
//! which is the one place this module joins [`crate::config_path`] to
//! [`factory_config::load`].

use crate::envelope::ErrorBody;
use crate::errors;

/// Load and validate the instance configuration beneath `instance_root`.
pub(crate) fn load(
    instance_root: &std::path::Path,
) -> Result<factory_config::InstanceConfig, ErrorBody> {
    factory_config::load(crate::config_path(instance_root)).map_err(errors::config_error)
}

/// The agent named `agent_name` declared for `scope_id`, or a `not_found.*`
/// error naming what is missing.
///
/// `factory_task::assign`'s own module docs record that `tasks` carries no
/// agent-name column, so nothing durable resolves "which agent is this task
/// for" on its own; every operation that needs one asks its caller for
/// `agent_name` and resolves it against the configuration here.
pub(crate) fn find_agent<'a>(
    config: &'a factory_config::InstanceConfig,
    scope_id: uuid::Uuid,
    agent_name: &str,
) -> Result<&'a factory_config::Agent, ErrorBody> {
    let scope_entry = config
        .scopes
        .iter()
        .find(|s| s.id == scope_id)
        .ok_or_else(|| {
            errors::err(
                "not_found.scope",
                format!("scope `{scope_id}` is not registered"),
            )
        })?;

    scope_entry
        .agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            errors::err(
                "not_found.agent",
                format!("scope `{scope_id}` has no agent named `{agent_name}`"),
            )
        })
}
