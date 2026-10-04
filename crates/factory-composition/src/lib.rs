//! Instance configuration and people-side page composition, outside the stack.
//! This package owns the real scope/config loader and view projections; it
//! does not supply a domain command port or an internal level identity.
//! Level crates must not import it. Live service and read isolation is still
//! being migrated separately; moving view code does not complete that work.
pub mod building;
pub mod config;
pub mod dashboard;
pub mod operations;
