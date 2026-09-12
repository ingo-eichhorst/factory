//! The one fact the site plan needs that nothing else already serves: how big
//! a scope is on disk, and how that size is spread across its top-level
//! entries. `scope_views` answers what runs where; `occupancy` answers what
//! happened when; this answers how large to draw the hall, and what to
//! treemap onto its floor.
//!
//! Everything else the plan draws -- halls placed and ordered, agents, queued
//! runs -- comes from `/api/agents` and the tasks already in front of the
//! page. Only the footprint needs a walk of the filesystem, which is why it
//! is its own request rather than a field `scope_views` would make every
//! caller of `/api/agents` pay for.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use factory_core::error::Result;
use factory_core::protocol::{ScopeArea, ScopeFootprint, SiteFootprint};

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

/// The area a file lying loose in a scope's root is filed under. A scope
/// root usually holds a few of these -- a `Cargo.toml`, a `README.md` -- and
/// they are one tile on the floor, not one per file; this names that tile
/// for what it is rather than inventing a directory that does not exist.
const ROOT_FILES_AREA: &str = "(files)";

/// What one bounded walk of a scope found: its total size, the same bytes
/// broken down by which top-level entry they came from, and whether the walk
/// ran out of `ENTRY_CAP` before it finished the tree.
struct ScopeWalk {
    total: u64,
    areas: HashMap<String, u64>,
    truncated: bool,
}

/// `None` when `root` is not a directory the daemon can read, or exists but
/// cannot be listed -- gone, or a permission it does not have. That is drawn
/// as "size not recorded", not as zero: zero is a real answer for an empty
/// scope, and this is not that.
///
/// One walk, not two: every top-level entry of `root` seeds its own area on
/// the stack, and everything found underneath it is filed there too, so the
/// per-area breakdown falls out of the same traversal that already summed
/// the total rather than a second pass over the tree.
fn walk_scope(root: &Path) -> Option<ScopeWalk> {
    if !root.is_dir() {
        return None;
    }
    let Ok(root_entries) = fs::read_dir(root) else {
        return None;
    };

    let mut total = 0u64;
    let mut areas: HashMap<String, u64> = HashMap::new();
    let mut visited = 0usize;
    let mut truncated = false;
    // Each stack entry carries the top-level area name its bytes belong to,
    // established once when it is first seen directly under `root` and then
    // carried unchanged all the way down.
    let mut stack: Vec<(PathBuf, String)> = Vec::new();

    for entry in root_entries.flatten() {
        visited += 1;
        if visited > ENTRY_CAP {
            truncated = true;
            break;
        }
        if SKIP.iter().any(|s| entry.file_name() == *s) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            let area = entry.file_name().to_string_lossy().into_owned();
            stack.push((entry.path(), area));
        } else {
            total += meta.len();
            *areas.entry(ROOT_FILES_AREA.to_string()).or_insert(0) += meta.len();
        }
    }

    'walk: while !truncated {
        let Some((dir, area)) = stack.pop() else {
            break;
        };
        // An unreadable subdirectory loses only itself; the root already
        // answered, above, for whether the scope as a whole can be read.
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > ENTRY_CAP {
                truncated = true;
                break 'walk;
            }
            if SKIP.iter().any(|s| entry.file_name() == *s) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push((entry.path(), area.clone()));
            } else {
                total += meta.len();
                *areas.entry(area.clone()).or_insert(0) += meta.len();
            }
        }
    }

    Some(ScopeWalk {
        total,
        areas,
        truncated,
    })
}

impl Engine {
    /// Every scope's size on disk, and the top-level breakdown behind it.
    /// Sequential and one `spawn_blocking` per scope: a handful of bounded
    /// walks, not a hot path -- the site view asks for this once when it
    /// opens, not on every poll.
    pub async fn site_footprint(self: &Arc<Self>) -> Result<SiteFootprint> {
        let mut scopes = Vec::new();
        for name in self.factory.scope_names() {
            let path = self.factory.scope_path(&name)?;
            let walk = tokio::task::spawn_blocking(move || walk_scope(&path))
                .await
                .unwrap_or(None);
            let (size_bytes, areas, truncated) = match walk {
                Some(w) => {
                    // A tile with nothing in it -- a top-level directory that
                    // is entirely `SKIP`, say -- would be a meaningless shape
                    // on the floor, so it is left off rather than drawn at
                    // zero width.
                    let mut areas: Vec<ScopeArea> = w
                        .areas
                        .into_iter()
                        .filter(|(_, size_bytes)| *size_bytes > 0)
                        .map(|(name, size_bytes)| ScopeArea { name, size_bytes })
                        .collect();
                    areas.sort_by(|a, b| {
                        b.size_bytes.cmp(&a.size_bytes).then_with(|| a.name.cmp(&b.name))
                    });
                    (Some(w.total), areas, w.truncated)
                }
                None => (None, Vec::new(), false),
            };
            scopes.push(ScopeFootprint {
                name,
                size_bytes,
                areas,
                truncated,
            });
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

    /// `walk_scope`'s areas, sorted the way `site_footprint` serves them --
    /// largest first, zero-byte tiles dropped -- so a test can assert on the
    /// shape the UI actually receives rather than the raw accumulator.
    fn sorted_areas(walk: &ScopeWalk) -> Vec<(String, u64)> {
        let mut areas: Vec<(String, u64)> = walk
            .areas
            .iter()
            .filter(|(_, &bytes)| bytes > 0)
            .map(|(name, &bytes)| (name.clone(), bytes))
            .collect();
        areas.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        areas
    }

    #[test]
    fn sums_file_bytes_under_the_root() {
        let s = Scratch::new("sum");
        fs::write(s.path().join("a.rs"), b"0123456789").unwrap();
        fs::create_dir(s.path().join("sub")).unwrap();
        fs::write(s.path().join("sub/b.rs"), b"01234").unwrap();
        let walk = walk_scope(s.path()).unwrap();
        assert_eq!(walk.total, 15);
        assert!(!walk.truncated);
    }

    #[test]
    fn skips_git_and_dependency_directories() {
        let s = Scratch::new("skip");
        fs::write(s.path().join("real.rs"), b"0123456789").unwrap();
        fs::create_dir(s.path().join("node_modules")).unwrap();
        fs::write(s.path().join("node_modules/huge.js"), vec![0u8; 1000]).unwrap();
        fs::create_dir(s.path().join(".git")).unwrap();
        fs::write(s.path().join(".git/pack"), vec![0u8; 1000]).unwrap();
        let walk = walk_scope(s.path()).unwrap();
        assert_eq!(walk.total, 10);
    }

    #[test]
    fn a_missing_directory_has_no_footprint() {
        let s = Scratch::new("missing");
        assert!(walk_scope(&s.path().join("nope")).is_none());
    }

    #[test]
    fn an_empty_directory_is_a_real_zero_not_unknown() {
        let s = Scratch::new("empty");
        let walk = walk_scope(s.path()).unwrap();
        assert_eq!(walk.total, 0);
        assert!(walk.areas.is_empty());
        assert!(!walk.truncated);
    }

    /// A root the daemon has no permission to list is the same "cannot read
    /// this scope" fact as a missing directory -- `None`, not a real zero --
    /// even though `Path::is_dir()` on it still succeeds, because stat-ing a
    /// path only needs execute on its *parents*, not on the path itself.
    #[test]
    #[cfg(unix)]
    fn an_unreadable_root_has_no_footprint() {
        use std::os::unix::fs::PermissionsExt;
        let s = Scratch::new("unreadable");
        fs::set_permissions(s.path(), fs::Permissions::from_mode(0o000)).unwrap();
        let result = walk_scope(s.path());
        // Restore before Scratch's Drop tries to remove the directory.
        fs::set_permissions(s.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_none());
    }

    /// Several top-level directories, plus files loose in the root, build one
    /// area per directory and one area for the loose files -- largest first,
    /// and the total is exactly their sum, because they are the same walk
    /// broken down rather than two different answers about the same tree.
    #[test]
    fn several_top_level_entries_become_areas_largest_first() {
        let s = Scratch::new("areas");
        fs::write(s.path().join("readme.md"), b"hello!").unwrap(); // 6 bytes, loose
        fs::create_dir(s.path().join("big")).unwrap();
        fs::write(s.path().join("big/data.bin"), vec![0u8; 20]).unwrap(); // 20 bytes
        fs::create_dir(s.path().join("small")).unwrap();
        fs::write(s.path().join("small/note.txt"), b"1234").unwrap(); // 4 bytes

        let walk = walk_scope(s.path()).unwrap();
        assert_eq!(walk.total, 30);
        assert!(!walk.truncated);
        assert_eq!(
            sorted_areas(&walk),
            vec![
                ("big".to_string(), 20),
                (ROOT_FILES_AREA.to_string(), 6),
                ("small".to_string(), 4),
            ]
        );
        let area_sum: u64 = walk.areas.values().sum();
        assert_eq!(area_sum, walk.total);
    }

    /// A top-level directory that is entirely `SKIP` underneath -- dependency
    /// output and nothing else -- sums to zero, and a zero-byte tile is not a
    /// shape a treemap can draw, so it is left out of `areas` rather than
    /// drawn at zero width.
    #[test]
    fn a_top_level_entry_that_is_entirely_skipped_has_no_area() {
        let s = Scratch::new("all-skipped");
        fs::create_dir(s.path().join("code")).unwrap();
        fs::write(s.path().join("code/main.rs"), b"0123456789").unwrap(); // 10 bytes
        fs::create_dir(s.path().join("onlydeps")).unwrap();
        fs::create_dir(s.path().join("onlydeps/node_modules")).unwrap();
        fs::write(s.path().join("onlydeps/node_modules/huge.js"), vec![0u8; 1000]).unwrap();

        let walk = walk_scope(s.path()).unwrap();
        assert_eq!(walk.total, 10);
        assert_eq!(sorted_areas(&walk), vec![("code".to_string(), 10)]);
    }

    /// Past `ENTRY_CAP` the walk stops rather than running unbounded, and
    /// says so: `truncated` is the fact the site plan needs to draw the
    /// floor as a lower bound instead of a measurement.
    #[test]
    fn a_walk_past_the_cap_is_marked_truncated() {
        // The files live under a subdirectory, not loose in the root, so this
        // proves the fact the floor actually keys off: a truncated walk can
        // still carry a real, non-empty `areas` -- the site plan has to draw
        // that partial floor, not just know the walk was cut short.
        let s = Scratch::new("cap");
        fs::create_dir(s.path().join("big")).unwrap();
        for i in 0..(ENTRY_CAP + 500) {
            fs::write(s.path().join("big").join(format!("f{i}")), b"x").unwrap();
        }
        let walk = walk_scope(s.path()).unwrap();
        assert!(walk.truncated);
        // The root's own entry for `big` costs one visit before the walk
        // descends into it, so the cap lands one short of a round `ENTRY_CAP`
        // -- a lower bound, not a measurement, which is the fact under test.
        let expected = (ENTRY_CAP - 1) as u64;
        assert_eq!(walk.total, expected);
        assert_eq!(sorted_areas(&walk), vec![("big".to_string(), expected)]);
    }
}
