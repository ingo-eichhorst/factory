//! The L0 kernel: pure vocabulary shared by code at every level, from
//! `factory-core`'s own lower layers up to the daemon (#193, phase 1).
//!
//! Nothing here may depend on any other `factory-*` crate -- that is the
//! whole point of an L0 nothing else can accidentally reach back up
//! through, and `tests/no_factory_dependency.rs` guards it. What lives here
//! is picked the same way: a type or function that several layers already
//! needed the same copy of, moved down to where duplicating it is no longer
//! possible.
//!
//! - [`Duration`] -- a freshness window (`30d`, `12h`, `2w`), formerly
//!   `factory_core::policy::Duration`. `policy.rs` keeps re-exporting it, so
//!   the wire format does not change; everything below L6 now names this
//!   path instead.
//! - [`nearest_rank`] and [`percentile`] -- the one percentile rule every
//!   layer that forecasts or reports agrees on, formerly
//!   `factory_core::scenario::nearest_rank` and `operations::percentile`'s
//!   own inlined sort-and-index.
//! - [`Level`], `L1`..`L6` and [`Fact`] (#193, phase 2) -- a marker per
//!   level and the trait that names a fact's producer. [`facts`]'s own doc
//!   comment explains which fact types moved here outright
//!   ([`DaemonConfigFact`], [`BackupFact`], [`VerifySummary`],
//!   [`SecretsPresence`]) and which stayed in `factory-core` with a local
//!   `impl Fact` because they carry a producing level's own status
//!   vocabulary; [`FACT_CATALOGUE`] lists all eleven.

mod duration;
pub mod facts;
mod stats;

pub use duration::Duration;
pub use facts::{
    BackupFact, DaemonConfigFact, Fact, FactCatalogueEntry, Level, SecretsPresence, VerifySummary, FACT_CATALOGUE,
    KNOWN_DAEMON_FACTS, KNOWN_SECRETS_LOCATIONS, L1, L2, L3, L4, L5, L6,
};
pub use stats::{nearest_rank, percentile};
