//! L2 environment domain: sandbox plans, secrets and declared dependencies.
//!
//! Configuration validation and planning do not reach into whole-instance
//! configuration, adapter traits, process tasks, or the router.
pub mod declarations;
pub mod dependencies;
pub mod expiry_store;
pub mod openshell;
pub mod secrets;
