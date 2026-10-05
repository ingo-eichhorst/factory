//! L5 assurance ownership: the sole plan compiler and check evaluator,
//! Quality, conformance, metrics, benchmarks/datasets, the benchmark store and
//! timer, and knowledge behavior/provider seam. Full live service isolation
//! and the remaining command ladder are still being migrated separately.
//!
//! Policy declarations must be adapted to plain command inputs, not imported
//! from an upper level through a compatibility facade:
//! ```compile_fail
//! use factory_core::policy::Applied;
//! ```
pub mod bench;
pub mod bench_store;
pub mod bench_timer;
pub mod benchmark;
pub mod budget;
pub mod checks;
pub mod check_evaluation;
pub mod conformance;
pub mod control_plan;
pub mod dataset;
pub mod evidence;
pub mod evaluation_rollup;
pub mod facts;
pub mod knowledge;
pub mod knowledge_provider;
pub mod metrics;
pub mod metrics_service;
pub mod metric_values;
pub mod signposts;
pub mod quality;
pub mod quality_inputs;
pub mod remediation;
