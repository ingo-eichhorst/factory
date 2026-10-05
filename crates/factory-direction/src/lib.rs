//! L6 authored direction: policy declarations, goals, scenarios and budgets.
//! Domain behavior, check-in and policy receipt storage live here, not in the facade.
//! Evaluation stays in L5; facts and shared receipts remain plain L0 data.
//! Live service isolation and adjacent command ports are still being migrated.
//!
//! L6 cannot import the outside compatibility hub or skip L5 for commands:
//! ```compile_fail
//! use factory_core::config::Factory;
//! ```
//! ```compile_fail
//! use factory_process::task::NewTask;
//! ```

pub mod budget;
pub mod budget_service;
pub mod goals;
pub mod goals_service;
pub mod goals_view;
pub mod goals_store;
pub mod policy;
pub mod policy_export;
pub mod policy_intent;
pub mod policy_service;
pub mod policy_report;
pub mod policy_store;
pub mod reporting_clock;
pub mod renewal_store;
pub mod scenario;
pub mod scenarios_view;
pub mod scenarios_service;
pub mod remediation;
