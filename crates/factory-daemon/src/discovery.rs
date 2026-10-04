//! Scope discovery is marker-based: a directory becomes a scope only when its
//! own `.factory/config.yaml` contains a `scope:` block. The file carries the
//! stable identity and runtime configuration; its containing directory is the
//! working path.
//!
//! The walk is bounded three ways. `SKIP` and `ENTRY_CAP` are shared with the
//! site walk so dependencies and build products cannot dominate startup, and
//! `DEPTH_LIMIT` prevents an unfamiliar tree from becoming unbounded. A
//! `.factory` directory is inspected as a marker but never traversed: the
//! instance root's runtime state and a scope's configuration are not scopes
//! beneath that scope.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use factory_core::config::{
    refuse_misplaced_scope_dashboard, refuse_misplaced_scope_infrastructure, refuse_misplaced_scope_policies,
    refuse_misplaced_scope_quality, refuse_misplaced_scope_roles, refuse_misplaced_scope_secrets, Factory, Scope, CONFIG_FILE, FACTORY_DIR,
};
use factory_core::error::{FactoryError, Result};
use serde::Deserialize;

use crate::site::{ENTRY_CAP, SKIP};

/// How many path segments below the instance root discovery will inspect.
pub const DEPTH_LIMIT: usize = 4;

/// A `.factory/config.yaml` may belong to another tool that shares the
/// directory. Only a document with Factory's `scope:` key is a scope marker.
/// Malformed YAML is still returned so `read_scope` can report the file rather
/// than silently hiding a broken scope declaration.
fn is_scope_config(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return true;
    };
    match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) {
        Ok(serde_yaml_ng::Value::Mapping(values)) => {
            values.contains_key(serde_yaml_ng::Value::String("scope".into()))
        }
        Err(_) => true,
        _ => false,
    }
}

/// Directories at or below `root` that contain a Factory config marker. The
/// result includes `root` when it has one, is breadth-first, and is bounded by
/// the same exclusions and entry cap as the site walk.
pub fn walk(root: &Path, entry_cap: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut visited = 0usize;
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));

    while let Some((dir, depth)) = queue.pop_front() {
        let config = dir.join(FACTORY_DIR).join(CONFIG_FILE);
        if config.is_file() && is_scope_config(&config) {
            found.push(dir.clone());
        }
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
            if meta.is_dir() {
                queue.push_back((entry.path(), depth + 1));
            }
        }
    }
    found
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn relative_path(root: &Path, dir: &Path) -> PathBuf {
    let rel = dir.strip_prefix(root).unwrap_or(dir);
    if rel.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        rel.to_path_buf()
    }
}

#[derive(Deserialize)]
struct ScopeFile {
    scope: Scope,
}

fn read_scope(path: &Path) -> Result<Scope> {
    let text = fs::read_to_string(path).map_err(|e| {
        FactoryError::Other(anyhow::anyhow!(
            "reading scope config {}: {e}",
            path.display()
        ))
    })?;
    let parsing = |e: serde_yaml_ng::Error| {
        FactoryError::Other(anyhow::anyhow!(
            "parsing scope config {}: {e}",
            path.display()
        ))
    };
    let document: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).map_err(parsing)?;
    // Only the `scope:` block is read, so a `roles:` beside it would vanish
    // without a word. Refuse it here, with the file named, before the daemon
    // starts on a role set somebody believes is different.
    refuse_misplaced_scope_roles(&document, path)?;
    // Same failure mode, for a top-level `policies:` block.
    refuse_misplaced_scope_policies(&document, path)?;
    // And for a top-level `quality:` block. Deliberately not also in
    // `configuration.rs`'s `read_document`, which reads the instance root's
    // own file too -- where a top-level `quality:` is exactly right.
    refuse_misplaced_scope_quality(&document, path)?;
    // And for a top-level `dashboard:` block (`#159`) -- the same mistake,
    // for the dashboard layout instead. Deliberately not also in
    // `configuration.rs`'s `read_document`, which mirrors quality's own
    // reasoning there: that reader also reads the instance root's own file,
    // where a top-level `dashboard:` is exactly right.
    refuse_misplaced_scope_dashboard(&document, path)?;
    // And for `infrastructure:`, which only the instance root's file reads.
    // Only nested files come through here -- the root's own config is parsed
    // whole by `Factory::load` -- so this never refuses the one place the
    // block belongs.
    refuse_misplaced_scope_infrastructure(&document, path)?;
    factory_core::config::refuse_misplaced_scope_renewals(&document, path)?;
    // And `secrets:` (#244), for the same reason.
    refuse_misplaced_scope_secrets(&document, path)?;
    let file: ScopeFile = serde_yaml_ng::from_value(document).map_err(parsing)?;
    Ok(file.scope)
}

fn validate_identity(scope: &Scope, path: &Path) -> Result<()> {
    if scope.id.trim().is_empty() {
        return Err(FactoryError::BadRequest(format!(
            "scope config {} has no scope.id",
            path.display()
        )));
    }
    if scope.name.trim().is_empty() {
        return Err(FactoryError::BadRequest(format!(
            "scope config {} has no scope.name",
            path.display()
        )));
    }
    Ok(())
}

/// Replace the runtime scope list with the configs found in scope directories.
/// The root scope has already been parsed as part of `Factory::load`; nested
/// files are parsed here. Duplicate names or stable IDs are refused with both
/// source files named, before any engine or standing agent starts.
pub fn apply(factory: &mut Factory) -> Result<()> {
    let root = canonical(&factory.root);
    let mut scopes = Vec::new();
    let mut ids: HashMap<String, PathBuf> = HashMap::new();
    let mut names: HashMap<String, PathBuf> = HashMap::new();

    for dir in walk(&root, ENTRY_CAP) {
        let config_path = dir.join(FACTORY_DIR).join(CONFIG_FILE);
        let mut scope = if dir == root {
            let Some(scope) = factory.config.scope.clone() else {
                // An older instance config may not opt the root itself into
                // being a scope. It still anchors discovery for nested ones.
                continue;
            };
            scope
        } else {
            read_scope(&config_path)?
        };

        validate_identity(&scope, &config_path)?;
        if let Some(first) = ids.insert(scope.id.clone(), config_path.clone()) {
            return Err(FactoryError::BadRequest(format!(
                "duplicate scope id {:?} in {} and {}",
                scope.id,
                first.display(),
                config_path.display()
            )));
        }
        if let Some(first) = names.insert(scope.name.clone(), config_path.clone()) {
            return Err(FactoryError::BadRequest(format!(
                "duplicate scope name {:?} in {} and {}",
                scope.name,
                first.display(),
                config_path.display()
            )));
        }

        scope.path = relative_path(&root, &dir);
        scopes.push(scope);
    }

    scopes.sort_by(|a, b| a.path.cmp(&b.path));
    factory.config.scopes = scopes;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::config::{Config, DaemonConfig, Instance};

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
        fn write_scope(&self, rel: &str, yaml: &str) -> PathBuf {
            let dir = self.path().join(rel).join(FACTORY_DIR);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join(CONFIG_FILE);
            fs::write(&path, yaml).unwrap();
            path
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn factory(root: &Path, root_scope: Option<Scope>) -> Factory {
        let mut legacy = configured("legacy-id", "legacy-central-entry");
        legacy.path = PathBuf::from("ordinary");
        Factory {
            root: root.to_path_buf(),
            config: Config {
                version: 1,
                instance: Instance {
                    id: "instance-id".into(),
                    name: "instance".into(),
                },
                daemon: DaemonConfig::default(),
                scope: root_scope,
                scopes: vec![legacy],
                roles: Default::default(),
                dashboard: Default::default(),
                policies: Default::default(),
                quality: Default::default(),
                infrastructure: Default::default(),
                secrets: Vec::new(),
                plugins_dir: None,
                renewals: Vec::new(),
                renewals_notify: None,
            },
        }
    }

    fn configured(id: &str, name: &str) -> Scope {
        serde_yaml_ng::from_str(&format!("id: {id}\nname: {name}\n")).unwrap()
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
    fn the_walk_returns_only_directories_with_factory_config() {
        let s = Scratch::new("markers");
        fs::create_dir_all(s.path().join("ordinary/child")).unwrap();
        s.write_scope("projects/demo", "scope: { id: demo-id, name: demo }");
        s.write_scope("projects/other", "scope: { id: other-id, name: other }");
        s.write_scope("runtime-only", "runtime: { version: 1 }");

        assert_eq!(
            rel(&s.path(), &walk(&s.path(), ENTRY_CAP)),
            vec!["projects/demo", "projects/other"]
        );
    }

    #[test]
    fn the_walk_skips_dependency_and_factory_internals() {
        let s = Scratch::new("skip");
        s.write_scope(
            "node_modules/left-pad",
            "scope: { id: hidden, name: hidden }",
        );
        s.write_scope(
            ".factory/worktrees/run",
            "scope: { id: state, name: state }",
        );
        s.write_scope("real", "scope: { id: real, name: real }");

        let found = rel(&s.path(), &walk(&s.path(), ENTRY_CAP));
        assert_eq!(found, vec!["real"]);
    }

    #[test]
    fn the_walk_stops_at_the_depth_limit_and_entry_cap() {
        let s = Scratch::new("bounds");
        s.write_scope("a/b/c/d", "scope: { id: in, name: in }");
        s.write_scope("a/b/c/d/e", "scope: { id: out, name: out }");
        for i in 0..20 {
            fs::create_dir_all(s.path().join(format!("d{i}"))).unwrap();
        }

        let found = rel(&s.path(), &walk(&s.path(), ENTRY_CAP));
        assert!(found.contains(&"a/b/c/d".to_string()), "{found:?}");
        assert!(!found.contains(&"a/b/c/d/e".to_string()), "{found:?}");
        assert!(
            walk(&s.path(), 5).len() <= 1,
            "only marked dirs can be returned"
        );
    }

    #[test]
    fn apply_loads_the_complete_local_scope_config_and_ignores_central_scopes() {
        let s = Scratch::new("config");
        s.write_scope(
            "projects/demo",
            "version: 1\nscope:\n  id: scope-1\n  name: demo\n  agent: pi\n  runtime: tmux\n  git: main\n  task_store: github\n  agents:\n    - name: reviewer\n      harness: codex\n      lifetime: permanent\n",
        );
        fs::create_dir_all(s.path().join("ordinary")).unwrap();
        let mut f = factory(&s.path(), None);

        apply(&mut f).unwrap();

        assert_eq!(f.config.scopes.len(), 1);
        let scope = &f.config.scopes[0];
        assert_eq!(scope.id, "scope-1");
        assert_eq!(scope.name, "demo");
        assert_eq!(scope.path, PathBuf::from("projects/demo"));
        assert_eq!(scope.agent_adapter(), Some("pi"));
        assert_eq!(scope.runtime.as_deref(), Some("tmux"));
        assert_eq!(scope.git.as_deref(), Some("main"));
        assert_eq!(scope.task_store.as_deref(), Some("github"));
        assert_eq!(scope.declared_agents()[0].name(), "reviewer");
    }

    #[test]
    fn the_instance_root_can_also_be_a_scope() {
        let s = Scratch::new("root");
        s.write_scope(
            "",
            "instance: { id: i, name: instance }\nscope: { id: root-id, name: root }",
        );
        let mut f = factory(&s.path(), Some(configured("root-id", "root")));

        apply(&mut f).unwrap();

        assert_eq!(f.config.scopes.len(), 1);
        assert_eq!(f.config.scopes[0].id, "root-id");
        assert_eq!(f.config.scopes[0].path, PathBuf::from("."));
    }

    #[test]
    fn malformed_nested_scope_config_names_the_file() {
        let s = Scratch::new("malformed");
        let path = s.write_scope("bad", "scope: [not, a, map]");
        let mut f = factory(&s.path(), None);

        let error = apply(&mut f).unwrap_err().to_string();
        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    #[test]
    fn a_roles_block_beside_the_scope_block_is_refused_with_the_file_named() {
        let s = Scratch::new("misplaced-roles");
        let path = s.write_scope(
            "projects",
            "scope: { id: p-id, name: projects }\nroles:\n  reviewer:\n    grants: [task.report]\n",
        );
        let mut f = factory(&s.path(), None);

        let error = apply(&mut f).unwrap_err().to_string();
        assert!(error.contains("scope.roles"), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    #[test]
    fn scope_roles_are_read_from_a_nested_scope_file() {
        let s = Scratch::new("scope-roles");
        s.write_scope(
            "projects",
            "scope:\n  id: p-id\n  name: projects\n  roles:\n    reviewer:\n      grants: [task.report]\n      reach: own\n",
        );
        let mut f = factory(&s.path(), None);

        apply(&mut f).unwrap();

        assert!(f.config.scopes[0].roles.contains_key("reviewer"));
    }

    #[test]
    fn a_policies_block_beside_the_scope_block_is_refused_with_the_file_named() {
        let s = Scratch::new("misplaced-policies");
        let path = s.write_scope(
            "projects",
            "scope: { id: p-id, name: projects }\npolicies:\n  frameworks: [cra]\n",
        );
        let mut f = factory(&s.path(), None);

        let error = apply(&mut f).unwrap_err().to_string();
        assert!(error.contains("scope.policies"), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    #[test]
    fn scope_policies_are_read_from_a_nested_scope_file() {
        let s = Scratch::new("scope-policies");
        s.write_scope(
            "projects",
            "scope:\n  id: p-id\n  name: projects\n  policies:\n    frameworks: [iso27001]\n",
        );
        let mut f = factory(&s.path(), None);

        apply(&mut f).unwrap();

        assert_eq!(f.config.scopes[0].policies.frameworks, vec!["iso27001".to_string()]);
    }

    #[test]
    fn a_dashboard_block_beside_the_scope_block_is_refused_with_the_file_named() {
        let s = Scratch::new("misplaced-dashboard");
        let path = s.write_scope(
            "projects",
            "scope: { id: p-id, name: projects }\ndashboard:\n  tiles: []\n",
        );
        let mut f = factory(&s.path(), None);

        let error = apply(&mut f).unwrap_err().to_string();
        assert!(error.contains("scope.dashboard"), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    #[test]
    fn scope_dashboard_is_read_from_a_nested_scope_file() {
        let s = Scratch::new("scope-dashboard");
        s.write_scope(
            "projects",
            "scope:\n  id: p-id\n  name: projects\n  dashboard:\n    tiles:\n      - { metric: throughput_week, size: s }\n",
        );
        let mut f = factory(&s.path(), None);

        apply(&mut f).unwrap();

        let dashboard = f.config.scopes[0].dashboard.as_ref().expect("scope.dashboard read");
        assert_eq!(dashboard.tiles.len(), 1);
    }

    #[test]
    fn missing_identity_is_refused_with_the_file_named() {
        let s = Scratch::new("identity");
        let path = s.write_scope("bad", "scope: { name: nameless-id }");
        let mut f = factory(&s.path(), None);

        let error = apply(&mut f).unwrap_err().to_string();
        assert!(error.contains("scope.id"), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    #[test]
    fn duplicate_ids_and_names_are_refused_with_both_files_named() {
        let s = Scratch::new("duplicates");
        let a = s.write_scope("a", "scope: { id: same, name: a }");
        let b = s.write_scope("b", "scope: { id: same, name: b }");
        let mut f = factory(&s.path(), None);
        let error = apply(&mut f).unwrap_err().to_string();
        assert!(error.contains("duplicate scope id"), "{error}");
        assert!(error.contains(&a.display().to_string()), "{error}");
        assert!(error.contains(&b.display().to_string()), "{error}");

        fs::write(&b, "scope: { id: other, name: a }").unwrap();
        let error = apply(&mut f).unwrap_err().to_string();
        assert!(error.contains("duplicate scope name"), "{error}");
        assert!(error.contains(&a.display().to_string()), "{error}");
        assert!(error.contains(&b.display().to_string()), "{error}");
    }

    #[test]
    fn two_equal_leaf_directories_can_use_distinct_configured_names() {
        let s = Scratch::new("names");
        s.write_scope("one/src", "scope: { id: one-id, name: one-source }");
        s.write_scope("two/src", "scope: { id: two-id, name: two-source }");
        let mut f = factory(&s.path(), None);

        apply(&mut f).unwrap();

        assert_eq!(f.scope_names(), vec!["one-source", "two-source"]);
    }
}
