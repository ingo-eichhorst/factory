//! What each hall on the site plan is, and what it is doing.
//!
//! Two answers, from two different places, deliberately served together so the
//! page draws both from one moment:
//!
//! * **how big the scope is** -- a bounded walk of its directory, counting
//!   files, bytes, source files and the directories holding them, plus the
//!   per-top-level-entry breakdown the floor treemaps. This is the only thing
//!   here that touches the filesystem, which is why the walk is cached: the
//!   page now asks for this on every run and agent event, and a repository
//!   does not change size between two of them.
//! * **what Factory is doing in it** -- runs in flight, tasks queued, agents
//!   standing up, read out of the daemon's own records. Never cached: it is
//!   the reason the page asks again.
//!
//! Both are then run through `factory_core::building`, which is where the
//! mapping from those numbers to a shape and a set of lit cues lives and where
//! it is tested. Nothing in this file decides what a hall looks like; it
//! gathers what the mapping needs and remembers the answer it gave last time,
//! which is what makes the steps sticky rather than flickering on a boundary.
//!
//! `scope_views` still answers what runs where and `occupancy` what happened
//! when. This stays its own request rather than a field on either, because a
//! filesystem walk is not something every caller of `/api/agents` should pay
//! for.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use factory_core::building::{appearance, Activity, RepoMetrics};
use factory_core::config::FACTORY_DIR;
use factory_core::error::Result;
use factory_core::protocol::{ScopeArea, ScopeFootprint, SiteFootprint};
use factory_core::run::RunStatus;
use factory_core::task::TaskStatus;

use crate::engine::Engine;

/// Directories that are not the codebase: dependencies, build output, version
/// control. Walking into them would measure `node_modules`, not the project.
/// Shared with `discovery.rs`, which walks the same tree for a different
/// reason and must stay out of the same directories for the same reason --
/// `FACTORY_DIR` is inspected as a scope marker by discovery but never walked:
/// configuration and instance state beneath it are not child scopes.
pub(crate) const SKIP: &[&str] = &[
    FACTORY_DIR,
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
/// `discovery.rs` reuses this rather than inventing its own: the reason a
/// walk needs a cap does not change with what the walk is for.
pub(crate) const ENTRY_CAP: usize = 40_000;

/// How long a walk's numbers stand before the directory is counted again. The
/// page asks for this view every time a run or an agent changes, which is
/// often; a repository's size changes when somebody commits, which is not. A
/// scope that grew is a hall that grows within five minutes, and nothing pays
/// for a filesystem walk per websocket event.
const WALK_TTL: Duration = Duration::from_secs(300);

/// Extensions counted as source. Shown in the details panel, never scored --
/// the size mapping uses files, bytes and directories, all three of which are
/// counts rather than judgements. This list is the judgement, and it is kept
/// out of the geometry for exactly that reason.
const SOURCE_EXTS: &[&str] = &[
    "rs", "js", "mjs", "ts", "tsx", "jsx", "vue", "svelte", "py", "go", "java", "kt", "kts",
    "rb", "c", "h", "cc", "cpp", "hpp", "cs", "swift", "m", "mm", "php", "scala", "clj", "ex",
    "exs", "erl", "hs", "lua", "pl", "r", "dart", "zig", "sh", "bash", "zsh", "fish", "sql",
    "html", "css", "scss", "sass", "less",
];

fn is_source(name: &std::ffi::OsStr) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| SOURCE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// The area a file lying loose in a scope's root is filed under. A scope
/// root usually holds a few of these -- a `Cargo.toml`, a `README.md` -- and
/// they are one tile on the floor, not one per file; this names that tile
/// for what it is rather than inventing a directory that does not exist.
const ROOT_FILES_AREA: &str = "(files)";

/// What one bounded walk of a scope found: its total size, the same bytes
/// broken down by which top-level entry they came from, the counts the size
/// mapping reads, and whether the walk ran out of `ENTRY_CAP` before it
/// finished the tree.
///
/// The counts fall out of the traversal that was already summing bytes -- one
/// pass, no second walk, and nothing is opened or read. Counting lines would
/// mean reading every file in the repository on a timer, which is a different
/// order of cost for a number that says the same thing as its bytes.
struct ScopeWalk {
    total: u64,
    areas: HashMap<String, u64>,
    truncated: bool,
    files: u64,
    source_files: u64,
    /// Directories holding at least one counted file -- the modules. A
    /// directory of nothing but skipped dependencies is not one.
    directories: u64,
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
    let mut files = 0u64;
    let mut source_files = 0u64;
    // A directory counts once, when its first file is found: counting every
    // directory entered would count the ones holding nothing but skipped
    // dependencies, and a module with no files in it is not a module.
    let mut directories = 0u64;
    let mut here_has_files = false;
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
            files += 1;
            if is_source(&entry.file_name()) {
                source_files += 1;
            }
            here_has_files = true;
            *areas.entry(ROOT_FILES_AREA.to_string()).or_insert(0) += meta.len();
        }
    }
    // The root itself is a module when it holds files of its own.
    if here_has_files {
        directories += 1;
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
        here_has_files = false;
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
                files += 1;
                if is_source(&entry.file_name()) {
                    source_files += 1;
                }
                here_has_files = true;
                *areas.entry(area.clone()).or_insert(0) += meta.len();
            }
        }
        if here_has_files {
            directories += 1;
        }
    }

    Some(ScopeWalk {
        total,
        areas,
        truncated,
        files,
        source_files,
        directories,
    })
}

/// One walk's answers, kept for `WALK_TTL`. `size_bytes` is `None` exactly
/// when the directory could not be read -- gone, or a permission the daemon
/// does not have -- which is a different fact from a scope walked and found
/// empty, and the two have to stay tellable apart all the way to the page.
#[derive(Debug, Clone)]
pub(crate) struct Measured {
    size_bytes: Option<u64>,
    areas: Vec<ScopeArea>,
    truncated: bool,
    metrics: RepoMetrics,
}

impl From<Option<ScopeWalk>> for Measured {
    fn from(walk: Option<ScopeWalk>) -> Self {
        let Some(walk) = walk else {
            return Self {
                size_bytes: None,
                areas: Vec::new(),
                truncated: false,
                // Not zeroes: `known` false is what keeps an unreadable scope
                // from being drawn as a small one.
                metrics: RepoMetrics::default(),
            };
        };
        // A tile with nothing in it -- a top-level directory that is entirely
        // `SKIP`, say -- would be a meaningless shape on the floor, so it is
        // left off rather than drawn at zero width.
        let mut areas: Vec<ScopeArea> = walk
            .areas
            .into_iter()
            .filter(|(_, size_bytes)| *size_bytes > 0)
            .map(|(name, size_bytes)| ScopeArea { name, size_bytes })
            .collect();
        areas.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes).then_with(|| a.name.cmp(&b.name)));
        Self {
            size_bytes: Some(walk.total),
            areas,
            truncated: walk.truncated,
            metrics: RepoMetrics {
                known: true,
                files: walk.files,
                source_files: walk.source_files,
                bytes: walk.total,
                directories: walk.directories,
                truncated: walk.truncated,
            },
        }
    }
}

impl Engine {
    /// Every scope's hall: how big it is, what is happening in it, and what
    /// both of those make it look like.
    ///
    /// The walk is cached per scope for `WALK_TTL`; the activity is read fresh
    /// every time, because that is what the page came back for. The tier and
    /// the activity level each scope was last drawn at are remembered here and
    /// handed to the mapping, which is what stops a metric sitting on a
    /// threshold from rebuilding the hall on every poll -- the same shape of
    /// memory as `seen_status`, and lost on restart for the same reason: after
    /// a restart the first answer is genuinely new.
    ///
    /// Configured scopes only. Ordinary directories are not halls, so the
    /// filesystem walk is paid exactly once per scope a local config opted in.
    pub async fn site_footprint(self: &Arc<Self>) -> Result<SiteFootprint> {
        let activity = self.scope_activity().await?;
        let factory = self.factory_snapshot();
        let mut scopes = Vec::new();
        for name in factory.scope_names() {
            let measured = self.measure_scope(&name).await?;
            let activity = activity.get(&name).copied().unwrap_or_default();
            let previous = self.site_memory.lock().unwrap().get(&name).copied();
            let (shape, cues) = appearance(&measured.metrics, &activity, previous);
            self.site_memory
                .lock()
                .unwrap()
                .insert(name.clone(), (shape.tier, cues.level));
            scopes.push(ScopeFootprint {
                name,
                size_bytes: measured.size_bytes,
                areas: measured.areas,
                truncated: measured.truncated,
                metrics: measured.metrics,
                shape,
                activity,
                cues,
            });
        }
        // A scope that stopped being declared must not keep a vote on what its
        // neighbours look like, or the memory grows for the life of the daemon.
        let live: std::collections::HashSet<String> =
            scopes.iter().map(|s| s.name.clone()).collect();
        self.site_memory.lock().unwrap().retain(|k, _| live.contains(k));
        Ok(SiteFootprint { scopes })
    }

    /// One scope's size, walked at most every `WALK_TTL`. One
    /// `spawn_blocking` per walk: a bounded traversal, off the reactor.
    async fn measure_scope(self: &Arc<Self>, name: &str) -> Result<Measured> {
        if let Some((at, measured)) = self.site_walks.lock().unwrap().get(name) {
            if at.elapsed() < WALK_TTL {
                return Ok(measured.clone());
            }
        }
        let path = self.factory_snapshot().scope_path(name)?;
        let walk = tokio::task::spawn_blocking(move || walk_scope(&path))
            .await
            .unwrap_or(None);
        let measured = Measured::from(walk);
        self.site_walks
            .lock()
            .unwrap()
            .insert(name.to_string(), (Instant::now(), measured.clone()));
        Ok(measured)
    }

    /// What Factory is doing in each scope, from its own records and nothing
    /// else. Assembled the way `occupancy` assembles its layers -- one list of
    /// tasks, one of runs in flight, one of standing agents -- rather than a
    /// lookup per run, which is what `scope_views` pays to answer a different
    /// question.
    async fn scope_activity(self: &Arc<Self>) -> Result<BTreeMap<String, Activity>> {
        let factory = self.factory_snapshot();
        let mut out: BTreeMap<String, Activity> = BTreeMap::new();
        for scope in &factory.config.scopes {
            // Capacity is what the scope *declares*, never how many agents
            // happen to be up: a hall whose only agent is down is a hall with
            // work it cannot start, and dividing by the agents present would
            // hide exactly that.
            out.insert(
                scope.name.clone(),
                Activity {
                    declared_agents: scope
                        .agents_with(&factory.config.daemon.foreman)
                        .len() as u32,
                    ..Default::default()
                },
            );
        }

        let tasks = self.store.list(&Default::default()).await?;
        let scope_of: HashMap<&str, &str> = tasks
            .iter()
            .map(|t| (t.id.as_str(), t.scope.as_str()))
            .collect();

        for run in self.store.active_runs().await? {
            let Some(scope) = scope_of.get(run.task_id.as_str()) else {
                continue; // the task is gone; the run has no hall to stand in
            };
            let Some(activity) = out.get_mut(*scope) else {
                continue;
            };
            activity.active_runs += 1;
            if run.status == RunStatus::Blocked {
                activity.blocked_runs += 1;
            }
        }

        for task in &tasks {
            // The same tasks the plan already draws at the door: assigned to
            // this scope, waiting for a trigger, no run started.
            if task.status == TaskStatus::Pending {
                if let Some(activity) = out.get_mut(&task.scope) {
                    activity.queued_tasks += 1;
                }
            }
        }

        for agent in self.store.agents().await? {
            let Some(activity) = out.get_mut(&agent.scope) else {
                continue;
            };
            if agent.state.is_live() {
                activity.live_agents += 1;
            } else if agent.declared && agent.error.is_some() {
                activity.failed_agents += 1;
            }
        }

        Ok(out)
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
        fs::create_dir(s.path().join(".factory")).unwrap();
        fs::write(s.path().join(".factory/factory.db"), vec![0u8; 1000]).unwrap();
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

    // ------------------------------------------------------- what was counted

    /// The counts the size mapping reads come out of the same single pass
    /// that already summed the bytes: files, the source-extension subset of
    /// them, and the directories actually holding files.
    #[test]
    fn one_pass_counts_files_source_files_and_the_directories_holding_them() {
        let s = Scratch::new("counts");
        fs::write(s.path().join("README.md"), b"docs").unwrap();
        fs::write(s.path().join("main.rs"), b"fn main() {}").unwrap();
        fs::create_dir(s.path().join("src")).unwrap();
        fs::write(s.path().join("src/lib.rs"), b"pub fn x() {}").unwrap();
        fs::write(s.path().join("src/notes.txt"), b"not source").unwrap();
        // A directory holding nothing but another directory is not a module:
        // nothing is filed in it.
        fs::create_dir_all(s.path().join("empty/deeper")).unwrap();

        let walk = walk_scope(s.path()).unwrap();
        assert_eq!(walk.files, 4);
        assert_eq!(walk.source_files, 2, "README.md and notes.txt are not source");
        assert_eq!(walk.directories, 2, "the root and src hold files; empty/ and deeper/ do not");
    }

    /// A directory of nothing but skipped dependencies is not a module, and
    /// its files are not this repository's files -- the counts have to honour
    /// `SKIP` exactly as the byte total already does.
    #[test]
    fn skipped_directories_count_for_nothing() {
        let s = Scratch::new("skip-counts");
        fs::write(s.path().join("main.rs"), b"fn main() {}").unwrap();
        fs::create_dir(s.path().join("node_modules")).unwrap();
        fs::write(s.path().join("node_modules/a.js"), b"x").unwrap();
        fs::create_dir(s.path().join("target")).unwrap();
        fs::write(s.path().join("target/big.bin"), vec![0u8; 100]).unwrap();

        let walk = walk_scope(s.path()).unwrap();
        assert_eq!(walk.files, 1);
        assert_eq!(walk.source_files, 1);
        assert_eq!(walk.directories, 1);
    }

    /// An empty scope counts to zero, which is a measurement. An unreadable
    /// one has no counts at all, which is not -- and `known` is the field that
    /// keeps a fresh scope from being drawn the same way as a missing one.
    #[test]
    fn an_empty_scope_is_measured_and_a_missing_one_is_not() {
        let s = Scratch::new("empty-vs-missing");
        let empty = Measured::from(walk_scope(s.path()));
        assert_eq!(empty.size_bytes, Some(0));
        assert!(empty.metrics.known);
        assert_eq!(empty.metrics.files, 0);

        let missing = Measured::from(walk_scope(&s.path().join("nope")));
        assert_eq!(missing.size_bytes, None);
        assert!(!missing.metrics.known);
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

    // ------------------------------------------------ the hall over the wire

    use factory_core::building::{ActivityLevel, Beacon, Tier};
    use factory_core::config::{Config, DaemonConfig, Instance};
    use factory_core::run::{NewRun, RunPatch, RunStatus, Trigger};
    use factory_core::task::NewTask;
    use factory_plugins::registry::Registry;
    use factory_plugins::SqliteStore;

    /// One scope pointed at `path`, one store in memory, every built-in
    /// adapter registered. Enough to answer `/api/site`, which reads the store
    /// and the filesystem and never starts anything.
    fn site_engine(path: &Path) -> Arc<Engine> {
        let config = Config {
            version: 1,
            instance: Instance { id: "i".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: None,
            scopes: vec![serde_yaml_ng::from_str(&format!(
                "name: demo\npath: {}\n",
                path.display()
            ))
            .unwrap()],
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        Arc::new(Engine::new(
            factory_core::config::Factory { root: path.to_path_buf(), config },
            Registry::with_builtins(),
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            vec![],
        ))
    }

    async fn hall(engine: &Arc<Engine>) -> ScopeFootprint {
        engine.site_footprint().await.unwrap().scopes.pop().unwrap()
    }

    /// A queued task, and then a run in flight, change what the hall is doing
    /// and nothing about what it is. This is the whole contract between the
    /// two signals, asserted end to end rather than on the mapping alone.
    #[tokio::test]
    async fn work_arriving_lights_the_hall_and_never_rebuilds_it() {
        let s = Scratch::new("engine-activity");
        fs::write(s.path().join("main.rs"), b"fn main() {}").unwrap();
        let engine = site_engine(s.path());

        let idle = hall(&engine).await;
        assert_eq!(idle.cues.beacon, Beacon::Off);
        assert_eq!(idle.cues.lit_floors, 0, "nothing running and nobody in");
        assert_eq!(idle.activity.queued_tasks, 0);

        let task = engine
            .create(NewTask {
                title: "something to do".into(),
                instructions: "true".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        let queued = hall(&engine).await;
        assert_eq!(queued.activity.queued_tasks, 1, "a pending task is work at the door");
        assert_eq!(queued.cues.beacon, Beacon::Waiting);
        assert!(queued.cues.lit_floors >= 1, "work waiting turns a light on");
        assert_eq!(queued.shape, idle.shape, "and does not touch the building");

        let run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: task.runtime.clone(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(&run.id, &RunPatch { status: Some(RunStatus::Running), ..Default::default() })
            .await
            .unwrap();

        let working = hall(&engine).await;
        assert_eq!(working.activity.active_runs, 1);
        assert_eq!(working.cues.beacon, Beacon::Working);
        assert!(working.cues.pulse_ms > 0, "a run is the one thing that moves");
        assert!(working.cues.lit_floors >= queued.cues.lit_floors);
        assert_eq!(working.shape, idle.shape, "still the same building");

        // And a run that stops to ask for a person is the hall's loudest fact.
        engine
            .store
            .update_run(&run.id, &RunPatch { status: Some(RunStatus::Blocked), ..Default::default() })
            .await
            .unwrap();
        let blocked = hall(&engine).await;
        assert_eq!(blocked.cues.beacon, Beacon::Blocked);
        assert_eq!(blocked.cues.pulse_ms, 0, "nothing is moving until somebody comes");
        assert_eq!(blocked.shape, idle.shape);

        // Finished: the lights come down on the same poll, with no margin to
        // wait out, and the hall is the size it always was.
        engine
            .store
            .update_run(&run.id, &RunPatch { status: Some(RunStatus::Done), ..Default::default() })
            .await
            .unwrap();
        engine
            .store
            .update(&task.id, &factory_core::task::TaskPatch {
                status: Some(TaskStatus::Done),
                ..Default::default()
            })
            .await
            .unwrap();
        let after = hall(&engine).await;
        assert_eq!(after.cues.level, ActivityLevel::Idle);
        assert_eq!(after.cues.beacon, Beacon::Off);
        assert_eq!(after.shape, idle.shape);
    }

    /// The walk is cached for `WALK_TTL`, so the page can ask on every event
    /// without a filesystem walk per websocket message -- and the hall it gets
    /// back is the same hall, not a flicker between two sizes.
    #[tokio::test]
    async fn the_size_is_walked_once_and_the_activity_every_time() {
        let s = Scratch::new("engine-cache");
        fs::write(s.path().join("main.rs"), b"fn main() {}").unwrap();
        let engine = site_engine(s.path());

        let first = hall(&engine).await;
        for i in 0..40 {
            fs::write(s.path().join(format!("added{i}.rs")), vec![b'x'; 4096]).unwrap();
        }
        let second = hall(&engine).await;

        assert_eq!(second.size_bytes, first.size_bytes, "within the TTL, the same walk answers");
        assert_eq!(second.metrics, first.metrics);
        assert_eq!(second.shape, first.shape);

        // Force the walk to be stale, and the hall grows.
        engine.site_walks.lock().unwrap().clear();
        let third = hall(&engine).await;
        assert!(third.metrics.files > first.metrics.files, "a re-walk sees the new files");
        assert!(third.shape.score > first.shape.score);
    }

    /// The tier a hall was last drawn at is remembered between calls, which is
    /// what the dead band needs to work at all -- and a scope that stops being
    /// declared does not keep a place in that memory for the life of the
    /// daemon.
    #[tokio::test]
    async fn a_hall_remembers_what_it_was_between_polls() {
        let s = Scratch::new("engine-memory");
        fs::write(s.path().join("main.rs"), b"fn main() {}").unwrap();
        let engine = site_engine(s.path());

        let first = hall(&engine).await;
        assert_eq!(
            engine.site_memory.lock().unwrap().get("demo").copied(),
            Some((first.shape.tier, first.cues.level))
        );
        let second = hall(&engine).await;
        assert_eq!(second.shape, first.shape, "the same facts draw the same hall");
        assert_eq!(second.cues, first.cues);

        engine.site_memory.lock().unwrap().insert("gone".into(), (Tier::Plant, ActivityLevel::Peak));
        hall(&engine).await;
        assert!(
            !engine.site_memory.lock().unwrap().contains_key("gone"),
            "a scope nobody declares any more keeps no memory"
        );
    }

    /// A scope pointed at nothing still gets a hall, and it is not the
    /// smallest one on the site: "could not be read" is not a measurement of
    /// zero, and drawing it as one would be a claim nobody made.
    #[tokio::test]
    async fn a_scope_pointed_at_nothing_is_drawn_as_unknown_not_as_empty() {
        let s = Scratch::new("engine-missing");
        let engine = site_engine(&s.path().join("not-here"));
        let missing = hall(&engine).await;
        assert_eq!(missing.size_bytes, None);
        assert_eq!(missing.shape.tier, Tier::Unknown);
        assert!(missing.shape.floors >= 1 && missing.shape.width_tenths > 0, "still drawable");

        let empty = Scratch::new("engine-empty");
        let on_empty = site_engine(empty.path());
        let plot = hall(&on_empty).await;
        assert_eq!(plot.shape.tier, Tier::Plot);
        assert!(
            plot.shape.width_tenths < missing.shape.width_tenths,
            "an empty scope is a small plot; an unreadable one is not drawn as smaller still"
        );
    }
}
