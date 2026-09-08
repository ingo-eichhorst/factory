//! Workspace validation for a session, including repository identity.
//!
//! Implements the workspace half of backlog Slice 6 ("Session lifecycle and
//! safe workspace leasing") and design §2.3's session rule:
//!
//! > A session workspace may be: the scope directory itself; an unregistered
//! > descendant directory; or a Git worktree belonging to the same
//! > repository as the scope. Only one live session may lease a given
//! > canonical workspace path. A descendant that is already a registered
//! > scope belongs to its own agent and cannot be used as another agent's
//! > workspace.
//!
//! [`validate`] decides ACCEPT or REJECT for a candidate path. It is pure
//! path and repository classification: it reads the filesystem and runs
//! `git`, but it records nothing in `factory-store` and leases nothing —
//! lease acquisition, `max_sessions`, and the one-live-lease-per-workspace
//! constraint are Slice 6's other half, built against the `sessions` and
//! `workspace_leases` tables this crate does not own.

use std::path::{Path, PathBuf};

use crate::repo::{self, RepositoryId};
use crate::{CanonicalPath, FileId, PathError, is_descendant};

/// Why a candidate workspace was accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceKind {
    /// The scope's own directory.
    ScopeDirectory,
    /// An existing directory strictly beneath the scope directory, and not
    /// itself a registered scope.
    UnregisteredDescendant,
    /// An existing Git worktree of the same repository as the scope, where
    /// the scope directory is also that repository's own checkout root.
    ///
    /// Not required to be a descendant of the scope directory. Worktrees
    /// commonly live as siblings on disk (`git worktree add ../feature`),
    /// and design §2.3 lists this as an *alternative* to the descendant
    /// case, not a special case of it. The checkout-root requirement is not
    /// in that design text — see [`validate`]'s doc comment for why this
    /// implementation adds it and the measurement behind that choice.
    SameRepositoryWorktree,
}

/// Why a candidate workspace was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceRejection {
    /// The path does not exist. Factory never creates one to make a
    /// candidate valid retroactively: ADR 0009 §3b lists `factory init
    /// --root` as the *only* command permitted to name a path that does not
    /// yet exist, and starting a session is not that command.
    DoesNotExist,
    /// The candidate is a descendant of the scope, but is itself a
    /// registered scope belonging to its own agent (design §2.3).
    /// `canonical_path` names the registered scope that conflicts.
    RegisteredScope { canonical_path: PathBuf },
    /// The candidate is an existing Git worktree, but of a different
    /// repository than the scope.
    ForeignRepository,
    /// The candidate is an existing worktree of the *same* repository the
    /// scope's directory sits inside of — but `scope` is not that
    /// repository's own checkout root, merely somewhere beneath it.
    /// `repository_root` names the checkout root `scope` was found inside.
    ///
    /// Not folded into [`WorkspaceRejection::OutsideScope`]: unlike that
    /// variant, a real repository relationship *does* exist between `scope`
    /// and the candidate here. The rejection is specifically that the scope
    /// does not own that repository, which is worth surfacing distinctly
    /// from "no relationship at all" — an operator seeing this needs to know
    /// the worktree passed the same-repository test and was still refused,
    /// not that it failed the test. See [`validate`]'s doc comment for the
    /// measured reason this rule exists and why it narrows design §2.3's
    /// literal text.
    ScopeNotRepositoryRoot { repository_root: PathBuf },
    /// The candidate is none of: the scope directory, a descendant of it, or
    /// a worktree of its repository.
    OutsideScope,
}

/// The verdict [`validate`] reaches once it can be reached at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceOutcome {
    Accepted(WorkspaceKind),
    Rejected(WorkspaceRejection),
}

/// Everything that stops [`validate`] from reaching a verdict — distinct
/// from [`WorkspaceRejection`], which *is* a verdict. An error here means the
/// filesystem or Git could not be consulted at all (permissions, a crashed
/// `git`, a scope directory that vanished mid-call), so no accept/reject
/// decision can be trusted; matches the existing split in this crate between
/// [`PathError::NotFound`] (a fact) and [`PathError::Unresolvable`] (a
/// failure to learn the fact).
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error(transparent)]
    Path(#[from] PathError),
    #[error(transparent)]
    Repo(#[from] repo::RepoError),
}

/// Decide whether `candidate` may host a session of the scope rooted at
/// `scope`.
///
/// `registered_scopes` is every *other* registered scope's canonical path
/// (gathering that list — e.g. from `factory-registry::resolve` — is the
/// caller's concern; this function has no database access). Including the
/// scope being validated in that list is harmless: a candidate equal to
/// `scope` itself is decided in step 2 below, before the registered-scope
/// check in step 3 ever runs.
///
/// ## Order, and why it is fixed
///
/// 1. **Existence.** A nonexistent path is rejected outright, before
///    anything else is asked of it, because no later check is meaningful for
///    a path that is not there — and because resolving it is also how step
///    2 gets a [`FileId`] to compare.
/// 2. **Identity with `scope` itself.**
/// 3. **Descendant of `scope`.** Checked *before* the worktree test in step
///    4. Two things depend on this order, not just on the individual
///    checks:
///    - A directory that is both a descendant of `scope` *and*,
///      coincidentally, part of the scope's own repository (an unusual but
///      possible layout — a subdirectory that happens to share the parent
///      repository) is classified as a descendant. Containment decides
///      first; repository identity is only consulted when containment does
///      not.
///    - A *registered* descendant scope is rejected here, in step 3, rather
///      than ever reaching step 4. If the worktree test ran first, a
///      registered scope living inside the same repository as its parent
///      would pass the repository-identity check and be wrongly accepted as
///      a worktree, silently bypassing the "belongs to its own agent" rule
///      design §2.3 states.
/// 4. **Same-repository worktree, gated by checkout-root ownership.** Tried
///    only for a candidate that is neither `scope` nor beneath it. Sharing a
///    repository with `scope` is necessary but, since the narrowing below,
///    no longer sufficient: the candidate is accepted only if `scope` is
///    itself that repository's checkout root
///    (`repo::checkout_root(scope) == scope`), not merely a directory
///    somewhere inside it.
///
/// ## A deliberate narrowing of design §2.3's literal text — open item, not settled
///
/// Design §2.3 accepts "a Git worktree belonging to the same repository as
/// the scope" without saying which directory must *own* that repository.
/// Read fully literally, step 4 would accept any worktree of the scope's
/// repository once the scope resolves to a repository at all, worktree
/// sitting inside the scope's own subtree or not (worktrees routinely do
/// not — `git worktree add ../feature`).
///
/// That reading was measured against the live registry rather than accepted
/// on the strength of the sentence alone, and it does not hold up. Five of
/// the seven registered scopes (`business-factory`, `assistant`, `ingo`,
/// `irrlicht`, `model-lab`) carry no `git:` of their own — design §2.1:
/// their files "live in the instance's repository" — so `repository_id`
/// resolves every one of them to the *same* instance repository, not five
/// separate ones. Under the literal reading they would therefore all accept
/// exactly the same set of worktrees, and the rule would stop distinguishing
/// scopes at all. Worse: a worktree of the instance repository contains a
/// checked-out copy of every other registered scope's directory, at
/// canonical paths distinct from the registered ones. Leasing such a
/// worktree to, say, `assistant` would hand that agent working copies of
/// `projects/factory`, `projects/irrlicht`, and every sibling scope. Design
/// line 127's rule — "a descendant that is already a registered scope
/// belongs to its own agent" — survives *by letter*, because those copies
/// sit at canonical paths the step 3 `RegisteredScope` check never sees, but
/// the boundary that rule exists to protect does not.
///
/// This function does **not** implement the fully literal reading. It
/// narrows it with the checkout-root gate in step 4: a scope that is merely
/// *inside* a repository it does not own gets
/// [`WorkspaceRejection::ScopeNotRepositoryRoot`] instead of a worktree it
/// should never have been offered. That scope still gets its own directory
/// and its unregistered descendants exactly as before — neither goes through
/// step 4 — so a scope with no `git:` loses only the worktree affordance it
/// was never safely entitled to, not the workspaces it already relies on.
///
/// **This is recorded as a deliberate narrowing pending a specification
/// answer, not as what design §2.3 already says.** The measurement above is
/// an open item against §2.3 itself, alongside the other decisions recorded
/// in `docs/implementation-backlog.md` §6. If §2.3 is amended to state the
/// containment requirement explicitly, this comment — and the
/// `ScopeNotRepositoryRoot` variant it justifies — becomes the
/// implementation of settled text, and should be revised to say so rather
/// than left reading as a narrowing of something the design no longer
/// leaves ambiguous.
pub fn validate(
    scope: &CanonicalPath,
    candidate: &Path,
    registered_scopes: &[CanonicalPath],
) -> Result<WorkspaceOutcome, WorkspaceError> {
    // Step 1: existence. `CanonicalPath::resolve` is also how every later
    // step gets a proof-of-existence path to work from.
    let candidate = match CanonicalPath::resolve(candidate) {
        Ok(canonical) => canonical,
        Err(PathError::NotFound { .. }) => {
            return Ok(WorkspaceOutcome::Rejected(WorkspaceRejection::DoesNotExist));
        }
        Err(other) => return Err(other.into()),
    };

    // Step 2: identity with the scope itself. Compared as `FileId`, not as
    // the two `CanonicalPath` strings, for the same reason every other
    // runtime aliasing check in this crate does: two canonical strings
    // resolved at different times can legitimately disagree about a
    // directory that has since been case-renamed (ADR 0009's corrected
    // §3a), and `scope` may have been resolved long before this call.
    let scope_id = FileId::of(scope.as_path())?;
    let candidate_id = FileId::of(candidate.as_path())?;
    if scope_id == candidate_id {
        return Ok(WorkspaceOutcome::Accepted(WorkspaceKind::ScopeDirectory));
    }

    // Step 3: descendant of the scope.
    if is_descendant(scope, &candidate)? {
        for registered in registered_scopes {
            // Re-resolved live rather than compared by string: ADR 0009's
            // corrected rule is that a stored canonical path is
            // re-canonicalized before comparison, never string-matched as
            // read back from wherever the caller obtained it.
            let registered_id = FileId::of(registered.as_path())?;
            if registered_id == candidate_id {
                return Ok(WorkspaceOutcome::Rejected(
                    WorkspaceRejection::RegisteredScope {
                        canonical_path: registered.as_path().to_path_buf(),
                    },
                ));
            }
        }
        return Ok(WorkspaceOutcome::Accepted(
            WorkspaceKind::UnregisteredDescendant,
        ));
    }

    // Step 4: same-repository worktree. The only acceptance path left for a
    // candidate that is neither the scope directory nor beneath it.
    let scope_repo = match repo::repository_id(scope) {
        Ok(id) => id,
        Err(repo::RepoError::NotARepository { .. }) => {
            // The scope itself is not inside any Git repository (a
            // standalone, unversioned project directory). With no
            // repository to compare against, no worktree test can possibly
            // succeed, so this is decided the same as any other
            // non-descendant, non-worktree candidate rather than treated as
            // an error.
            return Ok(WorkspaceOutcome::Rejected(WorkspaceRejection::OutsideScope));
        }
        Err(other) => return Err(other.into()),
    };
    let candidate_repo: RepositoryId = match repo::repository_id(&candidate) {
        Ok(id) => id,
        Err(repo::RepoError::NotARepository { .. }) => {
            return Ok(WorkspaceOutcome::Rejected(WorkspaceRejection::OutsideScope));
        }
        Err(other) => return Err(other.into()),
    };

    if scope_repo != candidate_repo {
        return Ok(WorkspaceOutcome::Rejected(
            WorkspaceRejection::ForeignRepository,
        ));
    }

    // Narrowing gate (see the doc comment above): sharing a repository is
    // not enough. `scope` must be that repository's own checkout root, not
    // merely a directory somewhere inside it — otherwise every scope sharing
    // one host repository would accept the same worktrees, including ones
    // containing copies of sibling scopes' directories.
    let scope_checkout_root = repo::checkout_root(scope)?;
    if FileId::of(scope_checkout_root.as_path())? == scope_id {
        Ok(WorkspaceOutcome::Accepted(
            WorkspaceKind::SameRepositoryWorktree,
        ))
    } else {
        Ok(WorkspaceOutcome::Rejected(
            WorkspaceRejection::ScopeNotRepositoryRoot {
                repository_root: scope_checkout_root.into_path_buf(),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    /// Test setup only — see the identical helper and rationale in
    /// `crate::repo::tests`.
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
        git(dir, &["commit", "--allow-empty", "-q", "-m", "init"]);
    }

    // ---- ACCEPT --------------------------------------------------------

    #[test]
    fn accepts_the_scope_directory_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let scope_dir = tmp.path().join("scope");
        fs::create_dir(&scope_dir).unwrap();
        let scope = CanonicalPath::resolve(&scope_dir).unwrap();

        let outcome = validate(&scope, &scope_dir, &[]).unwrap();
        assert_eq!(
            outcome,
            WorkspaceOutcome::Accepted(WorkspaceKind::ScopeDirectory)
        );
    }

    #[test]
    fn accepts_an_existing_unregistered_descendant() {
        let tmp = tempfile::tempdir().unwrap();
        let scope_dir = tmp.path().join("scope");
        let child_dir = scope_dir.join("child");
        fs::create_dir_all(&child_dir).unwrap();
        let scope = CanonicalPath::resolve(&scope_dir).unwrap();

        // An unrelated registered scope elsewhere on disk must not cause a
        // false-positive rejection: only a registered scope whose identity
        // matches the *candidate* disqualifies it.
        let other_scope_dir = tmp.path().join("other-scope");
        fs::create_dir(&other_scope_dir).unwrap();
        let other_scope = CanonicalPath::resolve(&other_scope_dir).unwrap();

        let outcome = validate(&scope, &child_dir, &[other_scope]).unwrap();
        assert_eq!(
            outcome,
            WorkspaceOutcome::Accepted(WorkspaceKind::UnregisteredDescendant)
        );
    }

    #[test]
    fn accepts_a_worktree_of_the_same_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let scope_dir = tmp.path().join("scope");
        init_repo(&scope_dir);

        let worktree_dir = tmp.path().join("scope-worktree");
        git(
            &scope_dir,
            &[
                "worktree",
                "add",
                "-q",
                worktree_dir.to_str().unwrap(),
                "-b",
                "wt",
            ],
        );

        let scope = CanonicalPath::resolve(&scope_dir).unwrap();
        let outcome = validate(&scope, &worktree_dir, &[]).unwrap();
        assert_eq!(
            outcome,
            WorkspaceOutcome::Accepted(WorkspaceKind::SameRepositoryWorktree)
        );
    }

    // ---- REJECT ----------------------------------------------------------

    #[test]
    fn rejects_a_path_that_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let scope_dir = tmp.path().join("scope");
        fs::create_dir(&scope_dir).unwrap();
        let scope = CanonicalPath::resolve(&scope_dir).unwrap();

        let missing = tmp.path().join("does-not-exist");
        let outcome = validate(&scope, &missing, &[]).unwrap();
        assert_eq!(
            outcome,
            WorkspaceOutcome::Rejected(WorkspaceRejection::DoesNotExist)
        );
    }

    #[test]
    fn never_creates_the_missing_path_it_rejects() {
        let tmp = tempfile::tempdir().unwrap();
        let scope_dir = tmp.path().join("scope");
        fs::create_dir(&scope_dir).unwrap();
        let scope = CanonicalPath::resolve(&scope_dir).unwrap();

        let missing = tmp.path().join("does-not-exist");
        let _ = validate(&scope, &missing, &[]).unwrap();
        assert!(
            !missing.exists(),
            "validate() must never create the path it is asked to validate"
        );
    }

    #[test]
    fn rejects_a_descendant_that_is_itself_a_registered_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let scope_dir = tmp.path().join("scope");
        let child_scope_dir = scope_dir.join("child-scope");
        fs::create_dir_all(&child_scope_dir).unwrap();
        let scope = CanonicalPath::resolve(&scope_dir).unwrap();
        let child_scope = CanonicalPath::resolve(&child_scope_dir).unwrap();

        let outcome =
            validate(&scope, &child_scope_dir, std::slice::from_ref(&child_scope)).unwrap();
        assert_eq!(
            outcome,
            WorkspaceOutcome::Rejected(WorkspaceRejection::RegisteredScope {
                canonical_path: child_scope.as_path().to_path_buf(),
            })
        );
    }

    #[test]
    fn rejects_a_worktree_of_a_foreign_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let scope_dir = tmp.path().join("scope");
        init_repo(&scope_dir);
        let scope = CanonicalPath::resolve(&scope_dir).unwrap();

        let foreign_dir = tmp.path().join("foreign");
        init_repo(&foreign_dir);
        let foreign_worktree = tmp.path().join("foreign-worktree");
        git(
            &foreign_dir,
            &[
                "worktree",
                "add",
                "-q",
                foreign_worktree.to_str().unwrap(),
                "-b",
                "wt",
            ],
        );

        let outcome = validate(&scope, &foreign_worktree, &[]).unwrap();
        assert_eq!(
            outcome,
            WorkspaceOutcome::Rejected(WorkspaceRejection::ForeignRepository)
        );
    }

    #[test]
    fn rejects_a_worktree_of_the_enclosing_repository_when_the_scope_is_not_its_root() {
        // Reproduces the live-registry shape this rule exists for: a scope
        // (e.g. `assistant`) that is a *subdirectory* of a larger host
        // repository (e.g. the `business-factory` instance repository),
        // sharing that repository with other registered scopes rather than
        // owning a repository of its own. A worktree of the host repository
        // is a real worktree of a real shared repository — the same-
        // repository test alone would accept it — but accepting it here
        // would hand this scope a checked-out copy of every sibling scope's
        // directory, which is exactly the boundary design line 127 exists to
        // stop. See `validate`'s doc comment for the full argument and the
        // registry measurement behind it.
        let tmp = tempfile::tempdir().unwrap();
        let enclosing_dir = tmp.path().join("enclosing");
        init_repo(&enclosing_dir);

        // The scope is a subdirectory of the enclosing repository, not its
        // root — this is what an instance-repository scope like `assistant`
        // or `irrlicht` looks like relative to `business-factory`.
        let scope_dir = enclosing_dir.join("scope-subdir");
        fs::create_dir(&scope_dir).unwrap();
        let scope = CanonicalPath::resolve(&scope_dir).unwrap();

        let worktree_dir = tmp.path().join("enclosing-worktree");
        git(
            &enclosing_dir,
            &[
                "worktree",
                "add",
                "-q",
                worktree_dir.to_str().unwrap(),
                "-b",
                "wt",
            ],
        );

        let enclosing = CanonicalPath::resolve(&enclosing_dir).unwrap();
        let outcome = validate(&scope, &worktree_dir, &[]).unwrap();
        assert_eq!(
            outcome,
            WorkspaceOutcome::Rejected(WorkspaceRejection::ScopeNotRepositoryRoot {
                repository_root: enclosing.as_path().to_path_buf(),
            })
        );
    }

    #[test]
    fn rejects_an_unrelated_directory_when_neither_side_is_a_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let scope_dir = tmp.path().join("scope");
        fs::create_dir(&scope_dir).unwrap();
        let scope = CanonicalPath::resolve(&scope_dir).unwrap();

        let unrelated_dir = tmp.path().join("unrelated");
        fs::create_dir(&unrelated_dir).unwrap();

        let outcome = validate(&scope, &unrelated_dir, &[]).unwrap();
        assert_eq!(
            outcome,
            WorkspaceOutcome::Rejected(WorkspaceRejection::OutsideScope)
        );
    }

    #[test]
    fn rejects_a_non_repository_sibling_when_the_scope_is_a_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let scope_dir = tmp.path().join("scope");
        init_repo(&scope_dir);
        let scope = CanonicalPath::resolve(&scope_dir).unwrap();

        let unrelated_dir = tmp.path().join("unrelated");
        fs::create_dir(&unrelated_dir).unwrap();

        let outcome = validate(&scope, &unrelated_dir, &[]).unwrap();
        assert_eq!(
            outcome,
            WorkspaceOutcome::Rejected(WorkspaceRejection::OutsideScope)
        );
    }
}
