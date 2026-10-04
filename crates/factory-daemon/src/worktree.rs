//! L2 workspace ownership: ordinary tasks reuse a worktree across runs;
//! benchmarks keep run-scoped evidence. The owner persists receipts and
//! safely releases eligible closed workspaces, refusing unsaved work.
//!
//! Two separate questions live here, and they are answered two separate ways
//! on purpose. Whether a scope *can* have a worktree at all is advisory: it
//! answers a disabled checkbox, so it says why in words a person can read.
//! Whether making one for a particular run *succeeded* is not advisory --
//! `git worktree add` either did it or it did not, and when it did not, the
//! only honest answer is git's own complaint, verbatim, because that is what
//! the failure will actually be about.

use std::path::Path;
use tokio::process::Command;

mod owner;
pub use owner::{Owner, OwnedWorkspace};

async fn git_output(scope_path: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(scope_path)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("running git: {e}"))
}

fn git_error(action: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{action} failed with {}", output.status)
    } else {
        format!("{action} failed with {}: {stderr}", output.status)
    }
}

/// The branch a task's own worktree runs on, derived from the task rather
/// than the run: a person reading `git branch` sees what the work was, not
/// which attempt made it. The first attempt gets the plain name; a retry
/// that also wants a worktree of its own needs a branch that does not
/// collide with the first, so the second and later attempts carry their
/// number.
pub fn branch_name(task_id: &str, title: &str, attempt: u32) -> String {
    let short = &task_id[..8.min(task_id.len())];
    let slug = slugify(title);
    let base = if slug.is_empty() {
        format!("factory/{short}")
    } else {
        format!("factory/{short}-{slug}")
    };
    if attempt <= 1 {
        base
    } else {
        format!("{base}-{attempt}")
    }
}

/// Lowercase ascii, hyphen-joined, no leading, trailing, or doubled hyphens --
/// what is left of a title once it is safe to put after a `/` in a branch
/// name. Truncated well short of any git limit so the branch reads as a
/// title, not a wrapped paragraph.
fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(40);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Whether a scope can have a worktree made in it, and why not when it
/// cannot, in words fit to show next to a disabled checkbox. `git worktree
/// add` needs a repository and a commit to branch the new worktree from; a
/// scope with neither has nowhere to put one.
pub async fn capability(scope_path: &Path) -> (bool, Option<String>) {
    // `git -C <missing path>` also just exits non-zero, which would read as
    // "not a git repository" -- true, but not the mistake a person actually
    // made. A scope pointed at nothing gets told that instead.
    if !scope_path.is_dir() {
        return (false, Some("no such directory".into()));
    }
    if !run_git_ok(scope_path, &["rev-parse", "--git-dir"]).await {
        return (false, Some("not a git repository".into()));
    }
    if !run_git_ok(scope_path, &["rev-parse", "--verify", "-q", "HEAD"]).await {
        return (
            false,
            Some("git repository has no commit yet to branch a worktree from".into()),
        );
    }
    (true, None)
}

async fn run_git_ok(scope_path: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(scope_path)
        .args(args)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Make the worktree, or say why `git` refused. Never partial: a failure here
/// leaves nothing for a caller to clean up on the way out, and this is the
/// only place that ever runs `git worktree add` -- there is no fallback path
/// that writes into `scope_path` instead.
///
/// `base` pins the commit a bench case names; `None` is today's behaviour --
/// branch from whatever the scope's HEAD happens to be -- which is what
/// every caller but a bench attempt still wants.
pub async fn create(scope_path: &Path, dir: &Path, branch: &str, base: Option<&str>) -> Result<(), String> {
    if let Some(parent) = dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(scope_path)
        .args(["worktree", "add", "-b", branch])
        .arg(dir);
    if let Some(base) = base {
        command.arg(base);
    }
    let output = command
        .output()
        .await
        .map_err(|e| format!("running git: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        "git worktree add failed with no message on stderr".to_string()
    } else {
        stderr
    })
}

/// Refresh one remote base before an integration branch is cut.  The fetch
/// is deliberately explicit: a decomposition must not quietly integrate an
/// old local `main` when `origin/main` has moved.
pub async fn fetch(scope_path: &Path, remote: &str, branch: &str) -> Result<(), String> {
    let output = git_output(scope_path, &["fetch", remote, branch]).await?;
    output
        .status
        .success()
        .then_some(())
        .ok_or_else(|| git_error("git fetch", &output))
}

/// A completed worker is mergeable only when every change is committed.
/// Returning the porcelain text makes the rework feedback concrete.
pub async fn dirty(dir: &Path) -> Result<Option<String>, String> {
    let output = git_output(dir, &["status", "--porcelain"]).await?;
    if !output.status.success() {
        return Err(git_error("git status", &output));
    }
    let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!status.is_empty()).then_some(status))
}

/// Merge one worker branch into the single integration worktree.  A failed
/// merge is always aborted before returning, so the next worker or rework
/// round never inherits conflict state.
pub async fn merge(integration_dir: &Path, branch: &str) -> Result<(), String> {
    let output = git_output(integration_dir, &["merge", "--no-ff", "--no-edit", branch]).await?;
    if output.status.success() {
        return Ok(());
    }
    let error = git_error("git merge", &output);
    let _ = git_output(integration_dir, &["merge", "--abort"]).await;
    Err(error)
}

/// Publish the integration branch after every merge and combined check has
/// passed.  No force option exists here by design.
pub async fn push(integration_dir: &Path, remote: &str, branch: &str) -> Result<(), String> {
    let output = git_output(integration_dir, &["push", "-u", remote, branch]).await?;
    output
        .status
        .success()
        .then_some(())
        .ok_or_else(|| git_error("git push", &output))
}

/// Whether `dir` is still one of `scope_path`'s registered worktrees --
/// `git worktree list --porcelain`, read rather than assumed. `factory task
/// run --continue` (`#178`) checks this before it will ever hand a resumed
/// agent back a previous run's directory: a person (or anything else) that
/// removed it by hand -- Factory itself never does -- must not have that
/// worktree treated as still there. `false` on any git failure: a scope
/// `git` cannot read is not one this can vouch for either.
pub async fn is_registered(scope_path: &Path, dir: &Path) -> bool {
    let target = match tokio::fs::canonicalize(dir).await {
        Ok(p) => p,
        // Gone entirely, or never existed -- either way, not registered.
        Err(_) => return false,
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(scope_path)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .await;
    let Ok(output) = output else { return false };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|path| std::fs::canonicalize(path).map(|p| p == target).unwrap_or(false))
}

/// Remove a worktree and the branch it was on, best-effort: a bench run's
/// evidence is kept until a person explicitly asks to clean it, and cleaning
/// one worktree that is already gone must not stop the rest of a run's from
/// being removed. `git worktree remove --force` first (a bench worktree may
/// hold uncommitted changes -- the whole point of keeping it as evidence --
/// so a plain `remove` would refuse it), then the branch, and a "there is
/// nothing there" from either is not itself an error.
pub async fn remove(scope_path: &Path, dir: &Path, branch: &str) -> Result<(), String> {
    if dir.exists() {
        let output = Command::new("git")
            .arg("-C")
            .arg(scope_path)
            .args(["worktree", "remove", "--force"])
            .arg(dir)
            .output()
            .await
            .map_err(|e| format!("running git: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                "git worktree remove failed with no message on stderr".to_string()
            } else {
                stderr
            });
        }
    }
    // Best effort: the worktree is gone either way, and a branch that never
    // existed (or was removed by another `clean` racing this one) is not a
    // failure worth reporting -- only a genuine git error is.
    let output = Command::new("git")
        .arg("-C")
        .arg(scope_path)
        .args(["branch", "-D", branch])
        .output()
        .await
        .map_err(|e| format!("running git: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.contains("not found") {
        return Ok(());
    }
    Err(stderr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_worktrees_branch_is_derived_from_the_task_and_reads_like_its_title() {
        assert_eq!(
            branch_name("1a2b3c4d5e6f", "Fix the flaky parser test", 1),
            "factory/1a2b3c4d-fix-the-flaky-parser-test"
        );
    }

    #[test]
    fn a_second_attempt_gets_a_suffix_so_it_does_not_collide_with_the_first() {
        let first = branch_name("1a2b3c4d5e6f", "Fix the flaky parser test", 1);
        let second = branch_name("1a2b3c4d5e6f", "Fix the flaky parser test", 2);
        assert_ne!(first, second, "a retry's branch must not be the first attempt's");
        assert_eq!(second, format!("{first}-2"));
        assert_eq!(
            branch_name("1a2b3c4d5e6f", "Fix the flaky parser test", 3),
            format!("{first}-3")
        );
    }

    #[test]
    fn punctuation_and_case_collapse_into_one_clean_slug() {
        assert_eq!(
            branch_name("deadbeefcafe", "Let's Ship v2.0!! (finally)", 1),
            "factory/deadbeef-let-s-ship-v2-0-finally"
        );
    }

    #[test]
    fn a_title_with_nothing_sluggable_still_gets_a_branch() {
        assert_eq!(branch_name("deadbeefcafe0000", "!!!", 1), "factory/deadbeef");
    }

    #[tokio::test]
    async fn a_scope_pointed_at_nothing_says_so_rather_than_blaming_git() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-missing-{}", uuid::Uuid::new_v4()));
        let (capable, reason) = capability(&dir).await; // never created
        assert!(!capable);
        assert_eq!(reason.as_deref(), Some("no such directory"));
    }

    #[tokio::test]
    async fn a_directory_with_no_git_in_it_is_not_worktree_capable() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (capable, reason) = capability(&dir).await;
        assert!(!capable);
        assert_eq!(reason.as_deref(), Some("not a git repository"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_git_repository_with_no_commit_is_not_worktree_capable_either() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&dir)
            .status()
            .await
            .unwrap()
            .success());
        let (capable, reason) = capability(&dir).await;
        assert!(!capable);
        assert_eq!(
            reason.as_deref(),
            Some("git repository has no commit yet to branch a worktree from")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_git_repository_with_a_commit_is_worktree_capable() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_with_a_commit(&dir).await;
        let (capable, reason) = capability(&dir).await;
        assert!(capable);
        assert_eq!(reason, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_worktree_that_cannot_be_made_reports_gits_own_message() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap(); // not a git repository at all
        let target = dir.join("wt");
        let err = create(&dir, &target, "factory/x", None).await.unwrap_err();
        assert!(!err.is_empty(), "git's stderr, not an empty complaint");
        assert!(!target.exists(), "nothing partial is left behind");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_capable_scope_gets_a_worktree_on_the_named_branch() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_with_a_commit(&dir).await;
        let target = std::env::temp_dir()
            .join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()))
            .join("worktrees")
            .join("run-1");
        create(&dir, &target, "factory/deadbeef-do-the-thing", None)
            .await
            .expect("a capable scope should not refuse this");
        assert!(target.join(".git").exists());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(target.parent().unwrap().parent().unwrap()).ok();
    }

    // -- #178: `--continue`'s "is this worktree still there" check ----------

    #[tokio::test]
    async fn a_freshly_made_worktree_is_registered() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_with_a_commit(&dir).await;
        let target = std::env::temp_dir()
            .join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()))
            .join("worktrees")
            .join("run-registered");
        create(&dir, &target, "factory/registered", None).await.unwrap();

        assert!(is_registered(&dir, &target).await);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(target.parent().unwrap().parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn a_worktree_removed_by_hand_is_not_registered() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_with_a_commit(&dir).await;
        let target = std::env::temp_dir()
            .join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()))
            .join("worktrees")
            .join("run-removed");
        create(&dir, &target, "factory/removed", None).await.unwrap();
        // A person deleting the directory outright, rather than going
        // through `git worktree remove` -- the case `--continue` (#178) has
        // to catch before it ever hands the directory back to a resumed
        // agent.
        std::fs::remove_dir_all(&target).unwrap();

        assert!(!is_registered(&dir, &target).await);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(target.parent().unwrap().parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn a_directory_that_was_never_a_worktree_is_not_registered() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_with_a_commit(&dir).await;
        // Exists, but was never `git worktree add`-ed.
        let never = dir.join("never-a-worktree");
        std::fs::create_dir_all(&never).unwrap();
        assert!(!is_registered(&dir, &never).await);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_scopes_own_directory_is_itself_a_registered_worktree() {
        // `git worktree list` names the primary checkout too, not only the
        // ones `git worktree add` made -- worth pinning down since
        // `resolve_continue` (#178) never actually asks this question (a
        // previous run's `worktree_path` is always a dedicated directory
        // under `worktrees_dir()`, never the scope itself), but a reader of
        // `is_registered` should not have to rediscover this from git's
        // manual.
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_with_a_commit(&dir).await;
        assert!(is_registered(&dir, &dir).await);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_worktree_removed_the_proper_way_is_no_longer_registered() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_with_a_commit(&dir).await;
        let target = std::env::temp_dir()
            .join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()))
            .join("worktrees")
            .join("run-clean-removed");
        let branch = "factory/clean-removed";
        create(&dir, &target, branch, None).await.unwrap();
        remove(&dir, &target, branch).await.unwrap();

        assert!(!is_registered(&dir, &target).await);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_worktree_with_a_base_branches_from_exactly_that_commit() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_with_a_commit(&dir).await;
        // A second commit on top, so HEAD and the pinned base genuinely
        // differ -- the case this test exists to prove.
        std::fs::write(dir.join("second.txt"), "second").unwrap();
        run_git(&dir, &["add", "second.txt"]).await;
        run_git(&dir, &["commit", "-q", "-m", "second"]).await;
        let head = git_output(&dir, &["rev-parse", "HEAD"]).await;
        let base = git_output(&dir, &["rev-parse", "HEAD~1"]).await;
        assert_ne!(head, base, "the two commits must genuinely differ");

        let target = std::env::temp_dir()
            .join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()))
            .join("worktrees")
            .join("run-2");
        create(&dir, &target, "factory/pinned-base", Some(&base))
            .await
            .expect("a capable scope should not refuse a pinned base");
        let wt_head = git_output(&target, &["rev-parse", "HEAD"]).await;
        assert_eq!(wt_head, base, "the worktree must branch from the pinned base, not HEAD");
        assert_ne!(wt_head, head);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(target.parent().unwrap().parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn remove_takes_the_worktree_and_its_branch_with_it() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_with_a_commit(&dir).await;
        let target = std::env::temp_dir()
            .join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()))
            .join("worktrees")
            .join("run-3");
        let branch = "factory/to-be-removed";
        create(&dir, &target, branch, None).await.unwrap();
        assert!(target.exists());

        remove(&dir, &target, branch).await.expect("removal should succeed");
        assert!(!target.exists(), "the worktree directory is gone");
        let branches = git_output(&dir, &["branch", "--list", branch]).await;
        assert!(branches.trim().is_empty(), "the branch is gone too: {branches:?}");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(target.parent().unwrap().parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn remove_is_tolerant_of_a_worktree_already_gone() {
        let dir = std::env::temp_dir().join(format!("factory-wt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_with_a_commit(&dir).await;
        let target = dir.join("never-made");
        // Never created at all -- a second `clean` racing the first, or a
        // person who already deleted it by hand.
        remove(&dir, &target, "factory/never-existed")
            .await
            .expect("nothing there is not an error");
        std::fs::remove_dir_all(&dir).ok();
    }

    async fn run_git(dir: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .await
            .unwrap()
            .success());
    }

    async fn git_output(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    async fn init_repo_with_a_commit(dir: &Path) {
        let run = |args: &'static [&'static str]| {
            let dir = dir.to_path_buf();
            async move {
                assert!(Command::new("git")
                    .args(args)
                    .current_dir(&dir)
                    .status()
                    .await
                    .unwrap()
                    .success());
            }
        };
        run(&["init", "-q"]).await;
        run(&["config", "user.email", "factory@example.com"]).await;
        run(&["config", "user.name", "factory"]).await;
        run(&["commit", "-q", "--allow-empty", "-m", "base"]).await;
    }
}
