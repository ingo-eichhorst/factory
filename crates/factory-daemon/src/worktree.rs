//! A run's own git worktree: made before the agent starts, never inside the
//! scope, and never cleaned up once it exists -- see `AGENTS.md` and the
//! ticket this implements for why.
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
pub async fn create(scope_path: &Path, dir: &Path, branch: &str) -> Result<(), String> {
    if let Some(parent) = dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(scope_path)
        .args(["worktree", "add", "-b", branch])
        .arg(dir)
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
        let err = create(&dir, &target, "factory/x").await.unwrap_err();
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
        create(&dir, &target, "factory/deadbeef-do-the-thing")
            .await
            .expect("a capable scope should not refuse this");
        assert!(target.join(".git").exists());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(target.parent().unwrap().parent().unwrap()).ok();
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
