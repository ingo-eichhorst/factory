//! Canonical path identity for Factory scopes and workspaces.
//!
//! ADR 0009 §3a established the rule this crate exists to enforce: on the
//! production volume (APFS, case-insensitive by default) a canonical path
//! string is *not* a unique identity. `/…/projects/factory` and
//! `/…/PROJECTS/factory` are the same directory, and `realpath` — like
//! [`std::fs::canonicalize`] — resolves symlinks without normalising case, so
//! string comparison reports "different" where the filesystem reports "same".
//!
//! The resulting split runs through every consumer:
//!
//! - a [`CanonicalPath`] is the **stored** identity: what the Slice 2 unique
//!   constraint indexes and what Slice 3 rewrites when a directory moves;
//! - a [`FileId`] is the **runtime aliasing check**: `(st_dev, st_ino)`, used
//!   for lease acquisition and descendant tests.
//!
//! Inode is deliberately not durable identity. A rename preserves it, so
//! move-and-reconcile keeps working, but delete-and-recreate at the same path
//! (removing and re-adding a worktree, restoring a backup) yields a new inode.
//!
//! This is not a security boundary. ADR 0009 §3c places TOCTOU explicitly out
//! of scope: design §6 states all agents run under one trusted macOS account
//! and that scope boundaries are cooperative policy. Rejecting symlink escapes
//! is a *correctness* control that stops one directory being leased twice under
//! two names, and must never be cited as a defence against a hostile process.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// A path that has been resolved against the filesystem.
///
/// Constructing one proves the path existed at that moment. Two
/// `CanonicalPath`s comparing equal are certainly the same directory; two
/// comparing unequal may still be, which is what [`FileId`] is for.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalPath(PathBuf);

impl CanonicalPath {
    /// Resolve `path`, following symlinks and rejecting `..` escapes.
    ///
    /// Fails if the path does not exist. Per ADR 0009 §3b this is correct for
    /// every caller but `factory init --root`, which creates the directory
    /// before resolving it: a path that cannot be canonicalized must not be
    /// registered or leased, because a normalized guess would later be compared
    /// against real paths and reintroduce the aliasing defect.
    pub fn resolve(path: impl AsRef<Path>) -> Result<Self, PathError> {
        let path = path.as_ref();
        let canonical = std::fs::canonicalize(path).map_err(|source| {
            io_error_to_path_error(path, source, || {
                format!(
                    "create {} before resolving it, or check the path for a typo",
                    path.display()
                )
            })
        })?;
        Ok(Self(canonical))
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    #[must_use]
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

/// The filesystem's own identity for a file: `(st_dev, st_ino)`.
///
/// This is the comparison that sees through case-insensitivity, hard links, and
/// symlinks. Use it for every runtime aliasing question; use [`CanonicalPath`]
/// for storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId {
    dev: u64,
    ino: u64,
}

impl FileId {
    /// The device number, for persisting this identity.
    #[must_use]
    pub fn dev(self) -> u64 {
        self.dev
    }

    /// The inode number, for persisting this identity.
    #[must_use]
    pub fn ino(self) -> u64 {
        self.ino
    }

    /// Rebuild an identity previously persisted via [`FileId::dev`] and
    /// [`FileId::ino`].
    ///
    /// Touches no filesystem and asserts nothing about whether the pair still
    /// refers to anything. Comparing a stored identity against a live one is
    /// precisely the caller's purpose — see ADR 0016's drift table, where a
    /// mismatch means "this may be a different directory", never "this is".
    #[must_use]
    pub fn from_parts(dev: u64, ino: u64) -> Self {
        Self { dev, ino }
    }

    /// Read the identity of the file or directory at `path`.
    pub fn of(path: impl AsRef<Path>) -> Result<Self, PathError> {
        let path = path.as_ref();
        let metadata = std::fs::metadata(path).map_err(|source| {
            io_error_to_path_error(path, source, || {
                format!(
                    "check that {} exists and is reachable before querying its identity",
                    path.display()
                )
            })
        })?;
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
}

/// Whether two paths name the same file, seeing through case and symlinks.
pub fn same_file(a: impl AsRef<Path>, b: impl AsRef<Path>) -> Result<bool, PathError> {
    Ok(FileId::of(a)? == FileId::of(b)?)
}

/// Whether `candidate` lies within `ancestor`.
///
/// Implemented by walking `candidate`'s parents and comparing [`FileId`], not
/// by testing a string prefix. ADR 0009 §3a records why: a case-variant prefix
/// makes a genuine descendant look like a non-descendant, and the delegation
/// rule in design §6 would then refuse legitimate parent-to-child work. The
/// defect reaches Slices 3, 6, and 8, so it is fixed once, here.
///
/// A path is not its own descendant.
pub fn is_descendant(
    ancestor: &CanonicalPath,
    candidate: &CanonicalPath,
) -> Result<bool, PathError> {
    let ancestor_id = FileId::of(ancestor.as_path())?;

    // Skip the candidate itself: a path is not its own descendant. Each
    // remaining ancestor is compared by FileId, never by string, so a
    // case-variant or symlinked component still lines up with `ancestor`.
    // `Path::ancestors()` walks up to and including the filesystem root, so
    // this loop terminates there without special-casing it.
    for parent in candidate.as_path().ancestors().skip(1) {
        if FileId::of(parent)? == ancestor_id {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Everything that can go wrong resolving or comparing a path.
///
/// Each variant names the path and states an action, matching the diagnostic
/// standard in `docs/slice-1-error-corpus.md`.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("path does not exist: {path}\n  help: {help}")]
    NotFound { path: PathBuf, help: String },

    #[error("cannot resolve {path}: {source}")]
    Unresolvable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Turn an IO failure into a [`PathError`], distinguishing "does not exist"
/// from every other failure (permissions, a component that is not a
/// directory, and so on). `help` is only evaluated for the `NotFound` case.
fn io_error_to_path_error(
    path: &Path,
    source: std::io::Error,
    help: impl FnOnce() -> String,
) -> PathError {
    if source.kind() == std::io::ErrorKind::NotFound {
        PathError::NotFound {
            path: path.to_path_buf(),
            help: help(),
        }
    } else {
        PathError::Unresolvable {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    /// ADR 0009 §3a: the production volume is APFS, case-insensitive by
    /// default, so `Foo` and `foo` name the same directory there. This test
    /// must not silently pass on a case-sensitive volume — that is exactly
    /// the failure mode the ADR warns about — so it detects the volume's
    /// behaviour at runtime and skips loudly if it cannot exercise the case
    /// it exists to check.
    #[test]
    fn case_variant_is_same_file_and_still_a_descendant() {
        let dir = tempdir().unwrap();
        let mixed_case_dir = dir.path().join("Foo");
        fs::create_dir(&mixed_case_dir).unwrap();

        let lower_case_dir = dir.path().join("foo");
        if !lower_case_dir.exists() {
            eprintln!(
                "SKIPPED case_variant_is_same_file_and_still_a_descendant: \
                 {} is on a case-sensitive volume, so `Foo` and `foo` are \
                 genuinely different directories here and this test cannot \
                 exercise the case-insensitivity defect ADR 0009 \u{a7}3a \
                 describes. This is expected on a case-sensitive volume and \
                 not a failure, but it means this test verified nothing.",
                dir.path().display()
            );
            return;
        }

        fs::create_dir(mixed_case_dir.join("child")).unwrap();

        // same_file sees through the case difference directly: it stats
        // both spellings and compares FileId, never comparing strings.
        assert!(same_file(&mixed_case_dir, &lower_case_dir).unwrap());

        // is_descendant must key off FileId too, not a stored string. Resolve
        // the ancestor under its original spelling, then rename the
        // directory to the other case — a legal, inode-preserving rename on
        // a case-insensitive volume (ADR 0009 §3a: rename preserves inode).
        // The `ancestor` string captured below is now permanently stale
        // relative to the live on-disk name, by construction: it was
        // resolved while "Foo" was the only name that existed, and the
        // rename happens strictly afterwards. A freshly resolved candidate
        // under the new name therefore cannot share a string prefix with
        // `ancestor`, regardless of whether this platform's `canonicalize`
        // preserves or normalises input case at resolve time — the two
        // strings are pinned to different points in time, not just
        // different spellings of "now".
        let ancestor = CanonicalPath::resolve(&mixed_case_dir).unwrap();
        fs::rename(&mixed_case_dir, &lower_case_dir).unwrap();
        let candidate = CanonicalPath::resolve(lower_case_dir.join("child")).unwrap();

        assert_ne!(
            ancestor.as_path(),
            candidate.as_path().parent().unwrap(),
            "expected the pre-rename ancestor string to disagree in case \
             with the post-rename candidate's parent string; if they match, \
             this test no longer discriminates a string-based is_descendant \
             from the required FileId-based one"
        );
        assert!(is_descendant(&ancestor, &candidate).unwrap());
    }

    #[test]
    fn symlink_aliasing() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        symlink(&real, &link).unwrap();

        assert!(same_file(&real, &link).unwrap());

        let via_real = CanonicalPath::resolve(&real).unwrap();
        let via_link = CanonicalPath::resolve(&link).unwrap();
        assert_eq!(via_real, via_link);
    }

    #[test]
    fn symlink_escape_is_not_a_descendant() {
        let dir = tempdir().unwrap();
        let inside = dir.path().join("inside");
        fs::create_dir(&inside).unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let escape = inside.join("escape");
        symlink(&outside, &escape).unwrap();

        let ancestor = CanonicalPath::resolve(&inside).unwrap();
        // Resolving the escape symlink follows it straight out of `inside`.
        let candidate = CanonicalPath::resolve(&escape).unwrap();
        assert!(!is_descendant(&ancestor, &candidate).unwrap());
    }

    #[test]
    fn dot_dot_traversal_normalises() {
        let dir = tempdir().unwrap();
        let b = dir.path().join("a").join("b");
        fs::create_dir_all(&b).unwrap();

        let direct = CanonicalPath::resolve(&b).unwrap();
        let via_dotdot = CanonicalPath::resolve(b.join("..").join("b")).unwrap();
        assert_eq!(direct, via_dotdot);
    }

    #[test]
    fn path_is_not_its_own_descendant() {
        let dir = tempdir().unwrap();
        let p = CanonicalPath::resolve(dir.path()).unwrap();
        assert!(!is_descendant(&p, &p).unwrap());
    }

    #[test]
    fn deep_nesting_is_a_descendant() {
        let dir = tempdir().unwrap();
        let deep = dir.path().join("a").join("b").join("c").join("d");
        fs::create_dir_all(&deep).unwrap();

        let ancestor = CanonicalPath::resolve(dir.path()).unwrap();
        let candidate = CanonicalPath::resolve(&deep).unwrap();
        assert!(is_descendant(&ancestor, &candidate).unwrap());
    }

    /// A naive `candidate_str.starts_with(ancestor_str)` implementation
    /// would wrongly call `project` a descendant of `proj`, because `proj`
    /// is a literal string prefix of `project`. Component/FileId-based
    /// comparison must not.
    #[test]
    fn unrelated_siblings_with_prefix_names_are_not_descendants() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj");
        let project = dir.path().join("project");
        fs::create_dir(&proj).unwrap();
        fs::create_dir(&project).unwrap();

        let proj = CanonicalPath::resolve(&proj).unwrap();
        let project = CanonicalPath::resolve(&project).unwrap();
        assert!(!is_descendant(&proj, &project).unwrap());
        assert!(!is_descendant(&project, &proj).unwrap());
    }

    #[test]
    fn nonexistent_path_is_not_found_and_names_the_path() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");

        let err = CanonicalPath::resolve(&missing).unwrap_err();
        match &err {
            PathError::NotFound { path, .. } => assert_eq!(path, &missing),
            other => panic!("expected PathError::NotFound, got {other:?}"),
        }

        let rendered = err.to_string();
        assert!(
            rendered.contains(&missing.display().to_string()),
            "rendered error {rendered:?} does not name the missing path"
        );
    }
}
