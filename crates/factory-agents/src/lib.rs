//! L3 agent domain and runtime adapter seam.
//!
//! Standing-session state, role resolution, harness health and the cumulative
//! runtime usage contract are owned here without process tasks or the facade.
//! Prompt/reporting contexts consume an explicit dispatch snapshot, never the
//! process task record. Live services and the strict command ladder remain
//! outside this domain/adapter ownership migration.
pub mod adapter;
pub mod agent;
pub mod assignment;
pub mod harness;
pub mod role;
pub mod role_chain;
pub mod roster;
pub mod selection;
pub mod runtime;
pub mod usage;
