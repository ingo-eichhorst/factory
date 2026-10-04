//! L5 assurance ownership: the sole plan compiler and check evaluator,
//! Quality, conformance and metric vocabulary. Benchmarks and live services
//! are still being migrated separately.
//!
//! Policy declarations must be adapted to plain command inputs, not imported
//! from an upper level through a compatibility facade:
//! ```compile_fail
//! use factory_core::policy::Applied;
//! ```
pub mod budget;
pub mod checks;
pub mod conformance;
pub mod control_plan;
pub mod metrics;
pub mod quality;
