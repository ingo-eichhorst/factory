//! The one fact the site plan needs that nothing else already serves: how big
//! a scope is on disk. `scope_views` answers what runs where; `occupancy`
//! answers what happened when; this answers how large to draw the hall.
//!
//! Everything else the plan draws -- halls placed and ordered, agents, queued
//! runs -- comes from `/api/agents` and the tasks already in front of the
//! page. Only the footprint needs a walk of the filesystem, which is why it
//! is its own request rather than a field `scope_views` would make every
//! caller of `/api/agents` pay for.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use factory_core::error::Result;
use factory_core::protocol::{ScopeFootprint, SiteFootprint};

use crate::engine::Engine;

/// Directories that are not the codebase: dependencies, build output, version
/// control. Walking into them would measure `node_modules`, not the project.
const SKIP: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".cache",
    "vendor",
    ".terraform",
];

/// A bound on how many entries one walk visits, so a scope pointed at
/// something enormous still answers in bounded time. What this returns once
/// the cap is hit is a lower bound, not a measurement -- fine for sizing a
/// hall relative to its neighbours, which is the only thing it is used for.
const ENTRY_CAP: usize = 40_000;

/// `None` when `root` is not a directory the daemon can read -- gone, or a
/// permission it does not have. That is drawn as "size not recorded", not as
/// zero: zero is a real answer for an empty scope, and this is not that.
fn dir_size_bounded(root: &Path) -> Option<u64> {
    if !root.is_dir() {
        return None;
    }
    let mut total = 0u64;
    let mut visited = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > ENTRY_CAP {
                return Some(total);
            }
            if SKIP.iter().any(|s| entry.file_name() == *s) {
                continue;
            }
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => stack.push(entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    Some(total)
}

impl Engine {
    /// Every scope's size on disk. Sequential and one `spawn_blocking` per
    /// scope: a handful of bounded walks, not a hot path -- the site view
    /// asks for this once when it opens, not on every poll.
    pub async fn site_footprint(self: &Arc<Self>) -> Result<SiteFootprint> {
        let mut scopes = Vec::new();
        for name in self.factory.scope_names() {
            let path = self.factory.scope_path(&name)?;
            let size_bytes = tokio::task::spawn_blocking(move || dir_size_bounded(&path))
                .await
                .unwrap_or(None);
            scopes.push(ScopeFootprint { name, size_bytes });
        }
        Ok(SiteFootprint { scopes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch directory per test, removed when the test drops it --
    /// tests run concurrently, so a shared fixture would race on its own files.
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "factory-site-test-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sums_file_bytes_under_the_root() {
        let s = Scratch::new("sum");
        fs::write(s.path().join("a.rs"), b"0123456789").unwrap();
        fs::create_dir(s.path().join("sub")).unwrap();
        fs::write(s.path().join("sub/b.rs"), b"01234").unwrap();
        assert_eq!(dir_size_bounded(s.path()), Some(15));
    }

    #[test]
    fn skips_git_and_dependency_directories() {
        let s = Scratch::new("skip");
        fs::write(s.path().join("real.rs"), b"0123456789").unwrap();
        fs::create_dir(s.path().join("node_modules")).unwrap();
        fs::write(s.path().join("node_modules/huge.js"), vec![0u8; 1000]).unwrap();
        fs::create_dir(s.path().join(".git")).unwrap();
        fs::write(s.path().join(".git/pack"), vec![0u8; 1000]).unwrap();
        assert_eq!(dir_size_bounded(s.path()), Some(10));
    }

    #[test]
    fn a_missing_directory_has_no_footprint() {
        let s = Scratch::new("missing");
        assert_eq!(dir_size_bounded(&s.path().join("nope")), None);
    }

    #[test]
    fn an_empty_directory_is_a_real_zero_not_unknown() {
        let s = Scratch::new("empty");
        assert_eq!(dir_size_bounded(s.path()), Some(0));
    }
}
