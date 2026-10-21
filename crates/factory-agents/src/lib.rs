//! L3 agent domain and runtime adapter seam.
//!
//! Standing-session state, role resolution, harness health and the cumulative
//! runtime usage contract are owned here without process tasks or the facade.
//! Prompt/reporting contexts consume an explicit dispatch snapshot, never the
//! process task record. Live services and the strict command ladder remain
//! outside this domain/adapter ownership migration. A scope's memory and
//! skills (`#339`) are L3's too: files in the scope, read and written here.
pub mod adapter;
pub mod agent;
pub mod assignment;
pub mod dispatch;
pub mod harness;
pub mod memory;
pub mod role;
pub mod role_chain;
pub mod roster;
pub mod selection;
pub mod skills;
pub mod runtime;
pub mod store;
pub mod usage;
