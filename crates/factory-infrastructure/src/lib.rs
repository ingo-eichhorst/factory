//! L1 infrastructure domain: backup, running environments and renewal decisions.
//!
//! Domain behaviour and backup/environment/expiry storage belong here, not in
//! the compatibility facade. Probes and full live service isolation are pending.
//! No dependency on core, plugins, the router, or a higher level is permitted.
pub mod backup;
pub mod backup_facts;
pub mod backup_store;
pub mod environment_facts;
pub mod environment_store;
pub mod environments;
pub mod expiry_store;
pub mod host;
pub mod interfaces;
#[cfg(test)]
mod provider_tests;
pub mod renewal_declarations;
pub mod renewals;
pub mod settings_facts;
