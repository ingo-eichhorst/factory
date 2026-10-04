//! L5 assurance ownership: a single compiler for declaration requirements.
//! The evaluator, benchmarks, live providers and services are not moved yet.
//!
//! Policy declarations must be adapted to plain command inputs, not imported
//! from an upper level through a compatibility facade:
//! ```compile_fail
//! use factory_core::policy::Applied;
//! ```
pub mod control_plan;
