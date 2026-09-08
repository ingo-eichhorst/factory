//! Git repository identity: whether two directories belong to the same
//! repository.
//!
//! Backlog Slice 6 ("Session lifecycle and safe workspace leasing") needs
//! this to accept "a Git worktree belonging to the same repository as the
//! scope" (design §2.3) as a session workspace. That requires answering "are
//! these two directories the same repository" without assuming either one is
//! the repository's main checkout — a worktree is a second directory for one
//! repository, not a repository of its own.
//!
//! ## The test
//!
//! Two paths belong to the same repository when
//! `git rev-parse --path-format=absolute --git-common-dir`, run in each,
//! resolves to the same directory — compared as a [`FileId`], not as a
//! string. Measured on this machine (`git version 2.50.1 (Apple Git-155)`,
//! `git -C <dir> rev-parse --path-format=absolute --git-common-dir`, working
//! directories under a fresh `tempdir`, `GIT_DIR`/`GIT_WORK_TREE`/
//! `GIT_COMMON_DIR` unset):
//!
//! ```text
//! repo-a                  -> …/repo-a/.git       exit 0
//! repo-a's own worktree   -> …/repo-a/.git       exit 0   (same as repo-a)
//! repo-b (independent)    -> …/repo-b/.git       exit 0   (different dir)
//! a plain, non-repo dir   -> fatal: not a git repository (or any of the
//!                            parent directories): .git     exit 128
//! ```
//!
//! `tests::repository_id_agrees_for_a_repository_and_its_own_worktree` and
//! `tests::repository_id_disagrees_for_two_independent_repositories`
//! reproduce this on-disk rather than trusting the transcript above.
//!
//! ## A second, deliberately different question: is this path a checkout *root*?
//!
//! `--git-common-dir` answers "same repository or not". It does not answer
//! "does this directory *own* that repository, or merely sit somewhere
//! inside it" — every subdirectory of a repository reports the same
//! common dir as the repository's root. [`crate::workspace::validate`] needs
//! that second, stricter question (see its doc comment for why), and
//! `--show-toplevel` answers it — but only once its behaviour *inside a
//! worktree* is checked, because that is exactly the case where the two
//! flags disagree. Measured on this machine, same Git version, same
//! isolation:
//!
//! ```text
//! repo-a (root)                  --show-toplevel -> …/repo-a       --git-common-dir -> …/repo-a/.git
//! repo-a/subdir (not root)       --show-toplevel -> …/repo-a       --git-common-dir -> …/repo-a/.git
//! repo-a/subdir/deeper           --show-toplevel -> …/repo-a       --git-common-dir -> …/repo-a/.git
//! repo-a-wt (worktree root)      --show-toplevel -> …/repo-a-wt    --git-common-dir -> …/repo-a/.git
//! repo-a-wt/subdir               --show-toplevel -> …/repo-a-wt    --git-common-dir -> …/repo-a/.git
//! ```
//!
//! Two things follow, and both matter to `validate`:
//!
//! - A directory is a checkout root exactly when `--show-toplevel`, run
//!   there, names that same directory — true for `repo-a` and for
//!   `repo-a-wt`, false for anything beneath either.
//! - Being *inside* a worktree does not merely inherit the main checkout's
//!   root: `--show-toplevel` names the worktree's own root, distinct from
//!   `--git-common-dir`'s answer (which stays pinned to the main checkout's
//!   `.git` even from inside the worktree). A scope that is itself a
//!   worktree is therefore correctly recognised as its own checkout root,
//!   not incorrectly deferred to whatever checked out the main branch.
//!
//! `tests::checkout_root_is_the_repository_root_from_the_root_or_any_subdirectory`
//! and `tests::checkout_root_is_the_worktrees_own_root_not_the_main_checkouts`
//! reproduce this on-disk.
//!
//! ## Why `FileId`, not string comparison
//!
//! `(st_dev, st_ino)` is used here — via [`FileId`] — because ADR 0009
//! already made it this codebase's identity primitive for "are these the
//! same directory", and a second identity notion (comparing the two
//! `--git-common-dir` strings directly) would be the more expensive choice:
//! one more comparison rule to document, test, and keep consistent with the
//! first, for no demonstrated benefit. This is *not* a claim that string
//! comparison of the common-dir output was shown to fail the way ADR 0009
//! §3a documents a real failure for scope paths (a stored path compared
//! against a case-renamed directory later). Nothing here exhibits that
//! failure — `--path-format=absolute` already yields an absolute path in one
//! call, so there is no "stored yesterday, compared today" gap for a rename
//! to fall into. `FileId` is reused because it is the established primitive,
//! not because string comparison was caught being wrong.
//!
//! ## Why not the scope's `git:` field
//!
//! A registered scope's `git:` value (`RegisteredScope::git` in
//! `factory-registry`) is a **remote URL**, and that crate's own doc comment
//! says it is "recorded, not verified" — Factory never contacts it. It
//! cannot answer "does this local directory belong to that repository":
//! two independent local clones of one remote have two different, unrelated
//! `.git` directories, and nothing in configuration stops two unrelated
//! local repositories from naming the same remote (a typo, a fork, a
//! mirror). Repository identity is a local filesystem fact, so it has to
//! come from the local filesystem — which is what this module asks `git`
//! for, never the registry.

use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use crate::{CanonicalPath, FileId, PathError};

/// The identity of a Git repository, independent of which checkout or
/// worktree path was used to reach it.
///
/// Two [`RepositoryId`]s comparing equal means the two paths that produced
/// them are part of the same repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RepositoryId(FileId);

/// Run `git -C <path> rev-parse --path-format=absolute <mode>` and return the
/// single absolute path it prints, or the classified failure.
///
/// Shared by [`repository_id`] (`mode = "--git-common-dir"`) and
/// [`checkout_root`] (`mode = "--show-toplevel"`): both are "ask git for one
/// absolute path rooted at `path`", with identical spawning,
/// environment-clearing, and success/failure handling — only the flag, and
/// what the answer means, differ. See the module docs for why both questions
/// are needed rather than one standing in for the other.
///
/// `path` must already exist — enforced by requiring a [`CanonicalPath`]
/// rather than a bare [`Path`], this crate's existing proof of existence —
/// because a nonexistent path cannot be an existing worktree in the first
/// place (backlog Slice 6: "REJECT: a path that does not exist"). Asking Git
/// about a path that was never resolved would only replace a clear
/// `WorkspaceRejection::DoesNotExist` (see [`crate::workspace`]) with a
/// confusing Git error.
fn rev_parse_absolute(path: &CanonicalPath, mode: &str) -> Result<PathBuf, RepoError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path.as_path())
        .args(["rev-parse", "--path-format=absolute", mode])
        // Cleared, not inherited. `GIT_DIR`, `GIT_WORK_TREE`, and
        // `GIT_COMMON_DIR` set in the calling process's environment (a
        // parent shell, a test harness, a wrapper script) would silently
        // redirect `rev-parse` at a repository other than the one actually
        // rooted at `path` — exactly the aliasing mistake this function
        // exists to prevent, just moved into the environment instead of the
        // argument list.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .output()
        .map_err(|source| RepoError::Spawn {
            path: path.as_path().to_path_buf(),
            source,
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        // Measured (see module docs): a path outside any repository exits
        // 128 with this exact message on Apple Git 2.50.1. Matched by
        // substring rather than trusting the exit code alone, because other
        // failures — a corrupt `.git`, a permissions problem — also exit
        // non-zero and must not be folded into "not a repository"; see
        // `GitFailed` below, which callers treat as an error rather than a
        // classification.
        if stderr.contains("not a git repository") {
            return Err(RepoError::NotARepository {
                path: path.as_path().to_path_buf(),
            });
        }
        return Err(RepoError::GitFailed {
            path: path.as_path().to_path_buf(),
            status: output.status,
            stderr,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(PathBuf::from(stdout.trim()))
}

/// Resolve the Git repository identity of `path`.
pub fn repository_id(path: &CanonicalPath) -> Result<RepositoryId, RepoError> {
    let common_dir = rev_parse_absolute(path, "--git-common-dir")?;
    // `--path-format=absolute` guarantees an absolute *string*, but unlike
    // `FileId::of` it does not stat the filesystem to resolve symlinks or
    // normalise case. `FileId::of` is what turns this string into an
    // identity, the same way every other runtime aliasing check in this
    // crate does.
    let file_id = FileId::of(&common_dir).map_err(|source| RepoError::CommonDirUnreachable {
        path: path.as_path().to_path_buf(),
        common_dir,
        source: Box::new(source),
    })?;
    Ok(RepositoryId(file_id))
}

/// Resolve the checkout root containing `path`: the top of a plain
/// repository, or the top of a linked worktree when `path` is inside one.
///
/// **Not the same question as [`repository_id`].** See the module docs'
/// measurement: from inside a worktree, `--show-toplevel` names the
/// *worktree's own* root, while `--git-common-dir` (what `repository_id`
/// uses) stays pinned to the *main* checkout's `.git`. This function is what
/// lets [`crate::workspace::validate`] tell "this scope directory owns the
/// repository it is in" apart from "this scope directory merely sits
/// somewhere inside one" — the distinction that gates whether the scope may
/// offer worktrees of that repository as workspaces at all.
pub fn checkout_root(path: &CanonicalPath) -> Result<CanonicalPath, RepoError> {
    let toplevel = rev_parse_absolute(path, "--show-toplevel")?;
    CanonicalPath::resolve(&toplevel).map_err(|source| RepoError::CheckoutRootUnreachable {
        path: path.as_path().to_path_buf(),
        toplevel,
        source: Box::new(source),
    })
}

/// Everything that can go wrong asking Git for a path's repository identity.
///
/// [`RepoError::NotARepository`] is deliberately not folded together with
/// the others: it is a normal classification outcome (not every workspace
/// candidate is inside a repository at all), and
/// [`crate::workspace::validate`] matches it out explicitly rather than
/// treating it as a failure. The remaining variants mean the question could
/// not be answered at all, which `validate` surfaces as an error rather than
/// guessing a verdict.
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("{path} is not inside a Git repository")]
    NotARepository { path: PathBuf },

    #[error("could not run git in {path}: {source}")]
    Spawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("git failed in {path} ({status}): {stderr}")]
    GitFailed {
        path: PathBuf,
        status: ExitStatus,
        stderr: String,
    },

    #[error(
        "git reported {path}'s common directory as {common_dir}, but it could not be read: {source}"
    )]
    CommonDirUnreachable {
        path: PathBuf,
        common_dir: PathBuf,
        #[source]
        source: Box<PathError>,
    },

    #[error(
        "git reported {path}'s checkout root as {toplevel}, but it could not be read: {source}"
    )]
    CheckoutRootUnreachable {
        path: PathBuf,
        toplevel: PathBuf,
        #[source]
        source: Box<PathError>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// Test setup only. Production code (`repository_id` above) never
    /// assumes a `git` invocation succeeds — it treats failure as data. This
    /// helper panics on failure because a test fixture that cannot be built
    /// invalidates the test, not the code under test.
    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git {args:?} in {dir:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_repo(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q"]);
        // A worktree cannot be added to a repository with no commits.
        git(dir, &["commit", "--allow-empty", "-q", "-m", "init"]);
    }

    #[test]
    fn repository_id_agrees_for_a_repository_and_its_own_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);

        let worktree_dir = tmp.path().join("repo-worktree");
        git(
            &repo_dir,
            &[
                "worktree",
                "add",
                "-q",
                worktree_dir.to_str().unwrap(),
                "-b",
                "wt",
            ],
        );

        let repo = CanonicalPath::resolve(&repo_dir).unwrap();
        let worktree = CanonicalPath::resolve(&worktree_dir).unwrap();
        assert_eq!(
            repository_id(&repo).unwrap(),
            repository_id(&worktree).unwrap()
        );
    }

    #[test]
    fn repository_id_disagrees_for_two_independent_repositories() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_a_dir = tmp.path().join("repo-a");
        let repo_b_dir = tmp.path().join("repo-b");
        init_repo(&repo_a_dir);
        init_repo(&repo_b_dir);

        let repo_a = CanonicalPath::resolve(&repo_a_dir).unwrap();
        let repo_b = CanonicalPath::resolve(&repo_b_dir).unwrap();
        assert_ne!(
            repository_id(&repo_a).unwrap(),
            repository_id(&repo_b).unwrap()
        );
    }

    #[test]
    fn checkout_root_is_the_repository_root_from_the_root_or_any_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);
        let deeper_dir = repo_dir.join("subdir").join("deeper");
        fs::create_dir_all(&deeper_dir).unwrap();

        let repo = CanonicalPath::resolve(&repo_dir).unwrap();
        let deeper = CanonicalPath::resolve(&deeper_dir).unwrap();

        assert_eq!(
            checkout_root(&repo).unwrap(),
            repo,
            "the root is its own checkout root"
        );
        assert_eq!(
            checkout_root(&deeper).unwrap(),
            repo,
            "a subdirectory's checkout root is the repository root, not itself"
        );
    }

    #[test]
    fn checkout_root_is_the_worktrees_own_root_not_the_main_checkouts() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        init_repo(&repo_dir);

        let worktree_dir = tmp.path().join("repo-worktree");
        git(
            &repo_dir,
            &[
                "worktree",
                "add",
                "-q",
                worktree_dir.to_str().unwrap(),
                "-b",
                "wt",
            ],
        );
        let worktree_subdir = worktree_dir.join("subdir");
        fs::create_dir(&worktree_subdir).unwrap();

        let worktree = CanonicalPath::resolve(&worktree_dir).unwrap();
        let worktree_sub = CanonicalPath::resolve(&worktree_subdir).unwrap();

        // The critical case: inside a worktree, the checkout root is the
        // worktree's own directory, never the main checkout's — even though
        // `repository_id` (via `--git-common-dir`) reports both as the same
        // repository. If this collapsed to the main checkout's root, a scope
        // that is itself a worktree could never pass the "is a checkout
        // root" test `workspace::validate` relies on.
        assert_eq!(checkout_root(&worktree).unwrap(), worktree);
        assert_eq!(checkout_root(&worktree_sub).unwrap(), worktree);
        assert_ne!(
            checkout_root(&worktree).unwrap(),
            CanonicalPath::resolve(&repo_dir).unwrap()
        );
    }

    #[test]
    fn repository_id_fails_clearly_for_a_non_repository_path() {
        let tmp = tempfile::tempdir().unwrap();
        let plain_dir = tmp.path().join("plain");
        fs::create_dir(&plain_dir).unwrap();

        let plain = CanonicalPath::resolve(&plain_dir).unwrap();
        match repository_id(&plain) {
            // Compared against `plain.as_path()`, not `plain_dir`: on this
            // machine `$TMPDIR` is itself a symlink (`/var` -> `/private/var`),
            // so the canonicalized path and the as-constructed one are
            // legitimately different strings for the same directory. The
            // function receives (and must echo back) the canonical one.
            Err(RepoError::NotARepository { path }) => assert_eq!(path, plain.as_path()),
            other => panic!("expected NotARepository, got {other:?}"),
        }
    }
}
