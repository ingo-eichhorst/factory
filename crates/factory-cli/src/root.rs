//! Locating a Factory instance root.
//!
//! An agent running `factory task done|fail|block` is invoked from inside its
//! own session, whose current directory is its *workspace* — a descendant of
//! the instance root, not the root itself (design §4: one `.factory/` at the
//! company root, never duplicated per scope). So every command but `init`
//! resolves the root by, in order: an explicit `--root`, the `FACTORY_ROOT`
//! environment variable, or walking up from the current directory for the
//! nearest ancestor holding a `.factory/` directory.
//!
//! `factory init` is the one exception (ADR 0009 §3b): the root need not
//! exist yet, so it defaults to the current directory instead of searching
//! for one that is already there.

use std::env;
use std::path::{Path, PathBuf};

/// Resolve the instance root for any command except `init`.
///
/// # Errors
///
/// A message naming every place this looked, when none of them found a
/// `.factory/` directory (or `--root` was not given and the current
/// directory itself could not be read).
pub fn discover(root_flag: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(root) = root_flag {
        return Ok(root.to_path_buf());
    }

    if let Ok(env_root) = env::var("FACTORY_ROOT") {
        if !env_root.trim().is_empty() {
            return Ok(PathBuf::from(env_root));
        }
    }

    let cwd = env::current_dir()
        .map_err(|source| format!("could not read the current directory: {source}"))?;

    for ancestor in cwd.ancestors() {
        if ancestor.join(".factory").is_dir() {
            return Ok(ancestor.to_path_buf());
        }
    }

    Err(format!(
        "could not find a Factory instance: no `--root` given, `FACTORY_ROOT` is not set, and no \
         ancestor of {} contains a `.factory/` directory\n  help: pass `--root <path>`, set \
         `FACTORY_ROOT`, or run `factory init` there first",
        cwd.display()
    ))
}

/// Resolve the root for `factory init`, which does not require `.factory/`
/// to exist yet.
///
/// # Errors
///
/// Only when `--root` was not given and the current directory could not be
/// read.
pub fn for_init(root_flag: Option<&Path>) -> Result<PathBuf, String> {
    match root_flag {
        Some(root) => Ok(root.to_path_buf()),
        None => env::current_dir()
            .map_err(|source| format!("could not read the current directory: {source}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_flag_wins_even_when_it_does_not_exist() {
        let root = discover(Some(Path::new("/does/not/exist"))).unwrap();
        assert_eq!(root, PathBuf::from("/does/not/exist"));
    }

    #[test]
    fn for_init_defaults_to_current_directory() {
        let root = for_init(None).unwrap();
        assert_eq!(root, env::current_dir().unwrap());
    }

    #[test]
    fn for_init_prefers_the_flag() {
        let root = for_init(Some(Path::new("/tmp/wherever"))).unwrap();
        assert_eq!(root, PathBuf::from("/tmp/wherever"));
    }
}
