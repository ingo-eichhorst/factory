//! Scope identity and ancestry, independent of authored configuration.
//! Names resolve aliases; only whole path components establish parents.
use crate::{FactoryError, Result};
use std::path::{Component, Path, PathBuf};

pub trait ScopeIdentity {
    fn scope_name(&self) -> &str;
    fn scope_path(&self) -> &Path;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeNode {
    pub name: String,
    pub path: PathBuf,
}

impl ScopeIdentity for ScopeNode {
    fn scope_name(&self) -> &str {
        &self.name
    }
    fn scope_path(&self) -> &Path {
        &self.path
    }
}

/// Plain scope identities captured from the live configuration at a read.
/// No declarations, store access, fact gathering or status live here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeTree {
    pub scopes: Vec<ScopeNode>,
}

impl ScopeTree {
    pub fn scope(&self, name: &str) -> Result<&ScopeNode> {
        resolve_scope(&self.scopes, name)
    }
    pub fn canonical_scope_name(&self, name: &str) -> String {
        self.scope(name)
            .map(|s| s.name.clone())
            .unwrap_or_else(|_| name.to_string())
    }
    pub fn ancestors_of(&self, scope: &ScopeNode) -> Vec<&ScopeNode> {
        scope_ancestors(&self.scopes, scope)
    }
    pub fn subtree_scopes(
        &self,
        scope: Option<&str>,
    ) -> Result<(Option<&ScopeNode>, Vec<&ScopeNode>)> {
        scope_subtree(&self.scopes, scope)
    }
}

/// Configured names win, then exact legacy paths, then unambiguous leaves.
/// Preserve configuration order in ambiguous-name diagnostics.
pub fn resolve_scope<'a, S: ScopeIdentity>(scopes: &'a [S], name: &str) -> Result<&'a S> {
    if let Some(s) = scopes.iter().find(|s| s.scope_name() == name) {
        return Ok(s);
    }
    if let Some(s) = scopes.iter().find(|s| {
        s.scope_path()
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
            == name
    }) {
        return Ok(s);
    }
    if !name.contains('/') {
        let matches: Vec<&S> = scopes
            .iter()
            .filter(|s| {
                s.scope_name().rsplit('/').next().unwrap_or(s.scope_name()) == name
                    || s.scope_path()
                        .file_name()
                        .map(|p| p == name)
                        .unwrap_or(false)
            })
            .collect();
        match matches.len() {
            0 => {}
            1 => return Ok(matches[0]),
            _ => {
                let candidates: Vec<&str> = matches.iter().map(|s| s.scope_name()).collect();
                return Err(FactoryError::BadRequest(format!(
                    "{name:?} could mean any of: {} -- name one of these instead",
                    candidates.join(", ")
                )));
            }
        }
    }
    Err(FactoryError::NoSuchScope(name.to_string()))
}

pub fn scope_ancestors<'a, S: ScopeIdentity>(scopes: &'a [S], scope: &S) -> Vec<&'a S> {
    let own = path_segments(scope.scope_path());
    let mut above: Vec<(usize, &S)> = scopes
        .iter()
        .filter_map(|candidate| {
            let theirs = path_segments(candidate.scope_path());
            (theirs.len() < own.len() && own.starts_with(&theirs))
                .then_some((theirs.len(), candidate))
        })
        .collect();
    above.sort_by_key(|(depth, _)| *depth);
    above.into_iter().map(|(_, s)| s).collect()
}

pub fn scope_subtree<'a, S: ScopeIdentity>(
    scopes: &'a [S],
    scope: Option<&str>,
) -> Result<(Option<&'a S>, Vec<&'a S>)> {
    let asked = scope.map(|name| resolve_scope(scopes, name)).transpose()?;
    let children = match asked {
        Some(asked) => scopes
            .iter()
            .filter(|child| {
                child.scope_name() == asked.scope_name()
                    || scope_ancestors(scopes, *child)
                        .iter()
                        .any(|ancestor| ancestor.scope_path() == asked.scope_path())
            })
            .collect(),
        None => scopes.iter().collect(),
    };
    Ok((asked, children))
}

fn path_segments(path: &Path) -> Vec<String> {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tree(entries: &[(&str, &str)]) -> ScopeTree {
        ScopeTree {
            scopes: entries
                .iter()
                .map(|(name, path)| ScopeNode {
                    name: (*name).into(),
                    path: (*path).into(),
                })
                .collect(),
        }
    }
    #[test]
    fn resolution_preserves_name_path_leaf_precedence_and_exact_errors() {
        let t = tree(&[
            ("projects/a", "elsewhere"),
            ("renamed", "projects/a"),
            ("third/a", "third/path"),
        ]);
        assert_eq!(t.scope("projects/a").unwrap().name, "projects/a");
        assert_eq!(t.scope("elsewhere").unwrap().name, "projects/a");
        assert_eq!(t.scope("path").unwrap().name, "third/a");
        assert!(
            matches!(t.scope("a"), Err(FactoryError::BadRequest(s)) if s == "\"a\" could mean any of: projects/a, renamed, third/a -- name one of these instead")
        );
        assert!(matches!(t.scope("gone"), Err(FactoryError::NoSuchScope(s)) if s == "gone"));
        assert_eq!(t.canonical_scope_name("a"), "a");
        assert_eq!(t.canonical_scope_name("gone"), "gone");
    }
    #[test]
    fn ancestry_uses_paths_skips_missing_configs_and_preserves_equal_depth_order() {
        let t = tree(&[
            ("leaf", "./projects/a/deep"),
            ("name/does/not/matter", "projects/a"),
            ("same-depth", "projects/a"),
            ("root", "."),
            ("leaf/fake-parent", "projects/a-x"),
            ("absolute", "/projects/a"),
        ]);
        let names: Vec<_> = t
            .ancestors_of(&t.scopes[0])
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["root", "name/does/not/matter", "same-depth"]);
        let (_, descendants) = t.subtree_scopes(Some("name/does/not/matter")).unwrap();
        assert_eq!(
            descendants
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["leaf", "name/does/not/matter"]
        );
        assert_eq!(t.subtree_scopes(None).unwrap().1.len(), 6);
    }
    #[test]
    fn absolute_and_parent_components_are_not_rewritten() {
        let t = tree(&[
            ("absolute-root", "/"),
            ("absolute-parent", "/a"),
            ("absolute-child", "/a/b"),
            ("relative", "a"),
            ("parent-dir", "a/.."),
            ("after-parent", "a/../b"),
        ]);
        assert_eq!(
            t.ancestors_of(&t.scopes[2])
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["absolute-root", "absolute-parent"]
        );
        assert_eq!(
            t.ancestors_of(&t.scopes[5])
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["relative", "parent-dir"]
        );
        assert_eq!(t.scope("//a/b").unwrap().name, "absolute-child");
        assert!(t.scope("/a/b").is_err()); // legacy component-join identity, not path normalization
    }
}
