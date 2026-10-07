//! L2 environment domain: sandbox plans, secrets and declared dependencies.
//!
//! Configuration validation and planning do not reach into whole-instance
//! configuration, adapter traits, process tasks, or the router.
pub mod credential_expiry;
pub mod credentials;
pub mod declarations;
pub mod dependencies;
pub mod dependency_inventory;
pub mod expiry_store;
pub mod openshell;
pub mod provision;
#[cfg(test)]
mod provider_tests;
pub mod sandbox;
pub mod sandbox_runtime;
pub mod secrets;
pub mod service_observations;
