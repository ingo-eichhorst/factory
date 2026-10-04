//! L3 agent domain and runtime adapter seam.
//!
//! Standing-session state, role resolution, harness health and the cumulative
//! runtime usage contract are owned here without process tasks or the facade.
//! Agent prompt/reporting contexts still await the L4 assignment-payload split.
pub mod agent;
pub mod harness;
pub mod role;
pub mod runtime;
pub mod usage;
