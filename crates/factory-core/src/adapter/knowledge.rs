//! Adapter seam 5: how the knowledge vault is searched. Read side only --
//! nothing is ever written through a provider. `knowledge::import`/`add`/
//! `write_bytes` stay the one way into the vault, with their sourcing and
//! secret-path checks, and tell the provider afterwards what they wrote.
//! See `.specs/adr/0003-knowledge-search.md` in the company repository.
//!
//! Two promises every provider keeps, whatever it does inside:
//!
//! - **A hit is a page id, never page text.** The agent opens the page
//!   itself. That keeps the knowledge module's own rule -- no body text and
//!   no document bytes on the wire -- true for search as well.
//! - **The vault is the source of truth.** A provider that keeps an index of
//!   its own keeps a derived one: deleting it and rebuilding from the vault
//!   loses nothing. It never indexes anything outside the vault, and never
//!   follows a source path into `data/secrets/`.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How many hits a search returns when the caller does not say.
pub const DEFAULT_SEARCH_LIMIT: usize = 10;

/// How many pages a run of a task with `knowledge_hints` on is handed. Few
/// on purpose: they are a starting point the agent reads, not a reading
/// list, and every one of them costs the agent a file open.
pub const KNOWLEDGE_HINTS_LIMIT: usize = 5;

/// The most hits one search may return, whatever the caller asks for. The
/// daemon applies it before a provider sees the query, so a provider can
/// trust `limit` rather than capping it again.
pub const MAX_SEARCH_LIMIT: usize = 50;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeQuery {
    /// Free text: something typed at `factory knowledge search`, or a task's
    /// title and instructions.
    #[serde(default)]
    pub text: String,
    /// Pages must carry every one of these tags. Empty filters nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The scope asking, for a provider that weighs a page's `area` against
    /// it. `None` when nobody in particular is asking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Already capped at `MAX_SEARCH_LIMIT` by the time a provider sees it.
    pub limit: usize,
}

pub use factory_kernel::{KnowledgeHit, KnowledgeHints};

#[async_trait::async_trait]
pub trait KnowledgeProvider: Send + Sync {
    fn name(&self) -> &str;

    /// One line for `factory adapters`.
    fn description(&self) -> String {
        format!("{} knowledge provider", self.name())
    }

    /// Hits for `query`, best first, at most `query.limit` of them. `root` is
    /// the instance root, the same path `knowledge::index` takes -- the
    /// vault is `<root>/.factory/knowledge/`. A vault that does not exist is
    /// no hits, not an error: an instance with nothing added yet is an empty
    /// state. May block on the filesystem only inside `spawn_blocking`.
    async fn search(&self, root: &Path, query: &KnowledgeQuery) -> Result<Vec<KnowledgeHit>>;

    /// The vault just had these files written into it: vault-relative paths
    /// with their extension, pages and documents alike, exactly as
    /// `knowledge::WriteResult::copied` names them. A provider that reads
    /// the vault on every search has nothing to do. Called after the write
    /// has already succeeded, so a failure here is logged and never undoes
    /// or fails the write.
    async fn changed(&self, root: &Path, files: &[String]) -> Result<()> {
        let _ = (root, files);
        Ok(())
    }
}
