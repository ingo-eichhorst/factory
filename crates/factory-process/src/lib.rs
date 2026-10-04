//! L4 owns standing intent, attempts, workflows, intake, execution gates and
//! the append-only run evidence/artifact provenance store.
//! Only L0 and the directly lower L3 contract may be imported here. Live
//! services and command routing remain separate migration work in #193.
//!
//! L4 cannot reach a facade to recover upward imports:
//! ```compile_fail
//! use factory_core::task::Task;
//! ```
pub mod assignment;
pub mod control_plan;
pub mod evidence_store;
pub mod facts;
pub mod intake;
pub mod measurements;
pub mod occupancy;
pub mod occupancy_history;
pub mod operations;
pub mod origin;
pub mod production;
mod process_metrics;
pub mod ready;
pub mod recovery_journal;
pub mod run;
pub mod store;
pub mod task;
pub mod usage;
pub mod window;
pub mod workflow;
pub mod workflow_store;
