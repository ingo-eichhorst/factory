//! Adapters. The ones that ship in the box, the host that runs the ones that
//! do not, and the registry that makes the daemon unable to tell them apart.

pub mod builtin;
pub mod host;
pub mod manifest;
pub mod proxy;
pub mod registry;

pub use builtin::{HarnessAgent, HerdrRuntime, KeywordKnowledge, ShellAgent, SqliteStore};
pub use manifest::{PluginManifest, discover, load};
pub use registry::Registry;
