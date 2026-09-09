//! Startup: reconcile the restored database before serving.
//!
//! ADR 0014's own open item: "a restarted daemon must treat 'sessions to
//! re-adopt' as normal rather than as a cold start." [`build_handler`] is
//! where that happens — [`factory_recovery::restore::reconcile`] runs before
//! the [`FactoryHandler`] this crate hands back is ever given to
//! [`crate::server::Daemon::serve`], so this crate's own `Handler` can never
//! observe a lease-holding session or a possibly-delivered task in the state
//! a restart, not a fresh start, would have left them in.

use factory_adapter::Adapter;

use crate::envelope::ErrorBody;
use crate::errors;
use crate::handler::FactoryHandler;

/// Open the store beneath `instance_root`, run
/// [`factory_recovery::restore::reconcile`], and build a [`FactoryHandler`]
/// over the result.
///
/// # Errors
///
/// Whatever `factory_store::Store::open` or `factory_recovery::restore::reconcile`
/// themselves return, as `internal.*`.
pub fn build_handler(
    instance_root: impl AsRef<std::path::Path>,
    adapter: impl Adapter + Send + Sync + 'static,
) -> Result<FactoryHandler, ErrorBody> {
    let instance_root = instance_root.as_ref();
    let mut store = factory_store::Store::open(instance_root)
        .map_err(|e| errors::err("internal.store_error", e.to_string()))?;

    factory_recovery::restore::reconcile(&mut store).map_err(errors::recovery_error)?;

    Ok(FactoryHandler::new(store, adapter, instance_root))
}
