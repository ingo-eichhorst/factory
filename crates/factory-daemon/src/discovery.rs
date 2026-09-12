//! Every directory under the instance root becomes a scope, so the rail can
//! select a directory nobody wrote into the config -- `projects`, on the
//! instance that builds this repo, which only ever held other directories
//! and so was furniture in the tree rather than a place work could be given
//! to.
//!
//! Bounded three ways, on purpose: `SKIP` and `ENTRY_CAP` are `site.rs`'s own
//! (a walk has to stay out of dependency trees and answer in bounded time
//! for the same reason regardless of what it is walking for), and
//! `DEPTH_LIMIT` is new -- without it, "every folder" means every folder
//! inside `node_modules` too, the moment a walk ever gets that deep, and the
//! rail becomes unusable on the first real project anyone points it at.
//!
//! What this does not do: give a discovered directory an agent, a runtime
//! override, or a foreman. Those stay exactly what `scopes:` says. A
//! discovered `Scope` runs on defaults until an entry there overlays onto
//! it -- see `Scope::discovered` and `apply` below.

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use factory_core::config::{scope_identity, Factory, Scope};

use crate::site::{ENTRY_CAP, SKIP};

/// How many path segments below the instance root a directory can be and
/// still become a scope. Four is deep enough for the shape a real monorepo
/// actually has -- `projects/<repo>/crates/<one>` is already four deep -- and
/// shallow enough that a tree with no `SKIP` entry of its own (a language
/// this instance has never heard of, say) still cannot turn into an unbounded
/// number of rows nobody asked for.
pub const DEPTH_LIMIT: usize = 4;

/// Every directory at or under `root`, root included, breadth-first -- so a
/// shallow directory is always found before anything nested in it, which is
/// what lets `apply` below match the shallower one first on a path collision.
/// Bounded by `SKIP` (kept out of dependency trees), `entry_cap` (answers in
/// bounded time), and `DEPTH_LIMIT` (a deep tree stays a small number of
/// rows). `entry_cap` is a parameter rather than always `site::ENTRY_CAP` so
/// a test can set it low without creating tens of thousands of files, the way
/// `site.rs`'s own cap test has to.
pub fn walk(root: &Path, entry_cap: usize) -> Vec<PathBuf> {
    let mut found = vec![root.to_path_buf()];
    let mut visited = 0usize;
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));

    while let Some((dir, depth)) = queue.pop_front() {
        if depth >= DEPTH_LIMIT {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > entry_cap {
                return found;
            }
            if SKIP.iter().any(|s| entry.file_name() == *s) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_dir() {
                continue;
            }
            let path = entry.path();
            found.push(path.clone());
            queue.push_back((path, depth + 1));
        }
    }
    found
}

/// `path`, resolved against `root` the way `Factory::scope_path` resolves a
/// scope's own path -- absolute paths kept as they are, everything else
/// joined onto the root.
fn absolute(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// Best-effort canonical form, for comparing two paths that might reach the
/// same directory through a symlink -- `/tmp` resolving to `/private/tmp` on
/// macOS is the case that bites if only one side of a comparison is
/// resolved. Falls back to the path as given when it cannot be resolved (it
/// does not exist yet, or the daemon cannot read one of its parents), which
/// keeps a scope pointed at a not-yet-created directory from vanishing from
/// the merge entirely.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Turn every directory under the instance root into a scope, then let each
/// `scopes:` entry make its own directory special. Declared entries keep
/// their place and go first, in the order they were written -- `create`'s
/// no-scope default is the first scope, and the site plan lays halls out in
/// list order, and neither should move just because a big tree added
/// thousands of undeclared ones after them.
///
/// An existing config is not edited for this: every scope it already names
/// keeps meaning the same directory, just under the identity that directory
/// gets from `scope_identity` now rather than the bare name that used to be
/// the whole of it. That rename is exactly what `Factory::scope`'s bare-name
/// fallback exists to paper over for anything already written down under the
/// old one.
pub fn apply(factory: &mut Factory) {
    let root = factory.root.clone();
    let root_canon = canonical(&root);

    let declared = std::mem::take(&mut factory.config.scopes);
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut resolved: Vec<Scope> = Vec::with_capacity(declared.len());
    for mut scope in declared {
        let abs = canonical(&absolute(&root, &scope.path));
        claimed.insert(abs.clone());
        scope.name = scope_identity(&root_canon, &abs, &scope.name);
        scope.declared = true;
        resolved.push(scope);
    }

    let mut discovered: Vec<Scope> = walk(&root_canon, ENTRY_CAP)
        .into_iter()
        .filter(|abs| !claimed.contains(abs))
        .map(|abs| {
            let rel = abs.strip_prefix(&root_canon).unwrap_or(&abs);
            let path = if rel.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                rel.to_path_buf()
            };
            let fallback = abs
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| factory.config.instance.name.clone());
            let name = scope_identity(&root_canon, &abs, &fallback);
            Scope::discovered(name, path)
        })
        .collect();
    discovered.sort_by(|a, b| a.path.cmp(&b.path));

    resolved.extend(discovered);
    factory.config.scopes = resolved;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "factory-discovery-test-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> PathBuf {
            canonical(&self.0)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn rel(root: &Path, found: &[PathBuf]) -> Vec<String> {
        let mut out: Vec<String> = found
            .iter()
            .map(|p| {
                let r = p.strip_prefix(root).unwrap();
                if r.as_os_str().is_empty() {
                    ".".to_string()
                } else {
                    r.to_string_lossy().replace('\\', "/")
                }
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn the_walk_finds_the_root_and_every_directory_under_it() {
        let s = Scratch::new("basic");
        fs::create_dir_all(s.path().join("projects/demo/src/inner")).unwrap();
        fs::write(s.path().join("projects/demo/readme.md"), b"hi").unwrap();
        let found = walk(&s.path(), ENTRY_CAP);
        assert_eq!(
            rel(&s.path(), &found),
            vec![
                ".",
                "projects",
                "projects/demo",
                "projects/demo/src",
                "projects/demo/src/inner",
            ]
        );
    }

    #[test]
    fn the_walk_skips_what_site_rs_skips() {
        let s = Scratch::new("skip");
        fs::create_dir_all(s.path().join("node_modules/left-pad")).unwrap();
        fs::create_dir_all(s.path().join("real")).unwrap();
        let found = rel(&s.path(), &walk(&s.path(), ENTRY_CAP));
        assert!(!found.iter().any(|p| p.contains("node_modules")), "{found:?}");
        assert!(found.contains(&"real".to_string()));
    }

    #[test]
    fn the_walk_never_surfaces_factorys_own_directory() {
        // "Nothing Factory owns is written inside a scope" (AGENTS.md) is not
        // only a rule about writes -- a scope discovery handed out for
        // `.factory` itself would be a place a task could be given to run an
        // agent straight at the daemon's own database.
        let s = Scratch::new("dotfactory");
        fs::create_dir_all(s.path().join(".factory/plugins")).unwrap();
        fs::create_dir_all(s.path().join("real")).unwrap();
        let found = rel(&s.path(), &walk(&s.path(), ENTRY_CAP));
        assert!(!found.iter().any(|p| p.contains(".factory")), "{found:?}");
        assert!(found.contains(&"real".to_string()));
    }

    #[test]
    fn the_walk_stops_at_the_depth_limit() {
        let s = Scratch::new("depth");
        // Root (0) / a (1) / b (2) / c (3) / d (4) / e (5). `d` is in bounds;
        // `e`, one level past the limit, is not.
        fs::create_dir_all(s.path().join("a/b/c/d/e")).unwrap();
        let found = rel(&s.path(), &walk(&s.path(), ENTRY_CAP));
        assert!(found.contains(&"a/b/c/d".to_string()), "{found:?}");
        assert!(!found.contains(&"a/b/c/d/e".to_string()), "{found:?}");
    }

    #[test]
    fn the_walk_respects_a_low_entry_cap() {
        let s = Scratch::new("cap");
        for i in 0..20 {
            fs::create_dir(s.path().join(format!("d{i}"))).unwrap();
        }
        // A cap far below the real count still returns *something* rather
        // than nothing -- a lower bound, not a crash -- and never exceeds it.
        let found = walk(&s.path(), 5);
        assert!(found.len() > 1, "the root plus at least a few directories");
        assert!(found.len() <= 1 + 5, "{found:?}");
    }

    #[test]
    fn a_declared_scope_overlays_onto_the_directory_discovery_already_found() {
        let s = Scratch::new("overlay");
        fs::create_dir_all(s.path().join("projects/demo")).unwrap();
        let mut factory = Factory {
            root: s.path(),
            config: bare_config(vec![Scope {
                name: "demo".into(),
                path: PathBuf::from("projects/demo"),
                agent: Some(factory_core::config::AgentRef::Name("pi".into())),
                agents: vec![],
                runtime: None,
                git: None,
                declared: true,
            }]),
        };
        apply(&mut factory);

        // One scope for `projects/demo`, not two -- the declared entry and
        // the directory discovery found are the same fact.
        let demo: Vec<_> = factory
            .config
            .scopes
            .iter()
            .filter(|sc| sc.path == PathBuf::from("projects/demo"))
            .collect();
        assert_eq!(demo.len(), 1, "{:?}", factory.config.scopes);
        assert_eq!(demo[0].name, "projects/demo", "identity is the path, not the declared name");
        assert!(demo[0].declared);
        assert_eq!(demo[0].agent_adapter(), Some("pi"), "the overlay's own settings survive the merge");

        // `projects` itself is real too, discovered and undeclared.
        let projects = factory
            .config
            .scopes
            .iter()
            .find(|sc| sc.path == PathBuf::from("projects"))
            .expect("projects is discovered");
        assert!(!projects.declared);
    }

    #[test]
    fn declared_scopes_stay_first_and_in_their_written_order() {
        let s = Scratch::new("order");
        fs::create_dir_all(s.path().join("projects/a")).unwrap();
        fs::create_dir_all(s.path().join("projects/b")).unwrap();
        let mut factory = Factory {
            root: s.path(),
            config: bare_config(vec![
                // `b` written before `a` -- alphabetically the wrong way
                // round, which is the point: this proves order survives from
                // the config rather than falling out of sorting the paths.
                Scope::discovered("b-declared".into(), PathBuf::from("projects/b")),
                Scope::discovered("a-declared".into(), PathBuf::from("projects/a")),
            ]),
        };
        // The two above were built with `Scope::discovered` only to skip
        // writing out every field; mark them declared as `scopes:` parsing
        // would.
        for s in &mut factory.config.scopes {
            s.declared = true;
        }
        apply(&mut factory);

        // Identity is recomputed from the path for both -- `b-declared` does
        // not survive -- but *which* path comes first still reflects the
        // order they were written in, not alphabetical path order.
        let paths: Vec<String> = factory
            .config
            .scopes
            .iter()
            .take(2)
            .map(|s| s.path.display().to_string())
            .collect();
        assert_eq!(paths, vec!["projects/b", "projects/a"], "{:?}", factory.config.scopes);
        assert!(factory.config.scopes[0].declared && factory.config.scopes[1].declared);
    }

    fn bare_config(scopes: Vec<Scope>) -> factory_core::config::Config {
        factory_core::config::Config {
            version: 1,
            instance: factory_core::config::Instance { id: "i".into(), name: "n".into() },
            daemon: Default::default(),
            scopes,
            roles: Default::default(),
            plugins_dir: None,
        }
    }
}
