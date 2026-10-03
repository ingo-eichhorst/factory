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

mod duration;
mod stats;

pub use duration::Duration;
pub use stats::{nearest_rank, percentile};
