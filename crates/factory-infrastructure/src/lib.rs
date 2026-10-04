//! L1 infrastructure domain: backup, running environments and renewal decisions.
//!
//! Pure domain behaviour belongs here, not in the compatibility facade.
//! No dependency on core, plugins, the router, or a higher level is permitted.
pub mod backup;
pub mod environments;
pub mod renewals;
