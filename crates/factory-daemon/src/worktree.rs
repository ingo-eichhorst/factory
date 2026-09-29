//! A run's own git worktree: made before the agent starts, never inside the
//! scope. A later run of the same task may resume in it (`#178`: a rework
//! round, or `--continue`), and the daemon itself releases it once the work
//! it belongs to is finished -- see `release` for when and how carefully.
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

/// `git -C dir <args>`, its stdout trimmed, or `None` when git did not
/// succeed -- for the read-only questions below, where an answer git cannot
/// give is simply not part of the answer.
async fn git_read(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().await.ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `#178` rule 4: what moved in the repository since a previous run ended,
/// for a run that resumes its conversation in `dir` -- that agent's memory
/// of the files is exactly that old. New commits on the branch checked out
/// in `dir`, new commits on the scope's own current branch (`main`, as a
/// rule), and whether merging that into this one would now conflict.
/// `None` when nothing moved, or when git could not say. Local refs only:
/// nothing here fetches, so a push nobody fetched yet is not seen.
pub async fn changes_since(dir: &Path, scope_path: &Path, since: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let since_arg = format!("--since={}", since.to_rfc3339());
    let commits = |rev: String| {
        let since_arg = since_arg.clone();
        async move {
            let text = git_read(dir, &["log", "--oneline", "--no-decorate", &since_arg, "-n", "20", &rev]).await?;
            Some(text.lines().map(str::to_string).collect::<Vec<_>>())
        }
    };
    let branch = git_read(dir, &["symbolic-ref", "--short", "HEAD"]).await;
    let default = git_read(scope_path, &["symbolic-ref", "--short", "HEAD"]).await;
    let mut parts = Vec::new();
    if let Some(on_branch) = commits("HEAD".into()).await.filter(|c| !c.is_empty()) {
        parts.push(format!(
            "{} new commit(s) on {} ({})",
            on_branch.len(),
            branch.as_deref().unwrap_or("this branch"),
            on_branch.join("; ")
        ));
    }
    if let Some(default) = default.filter(|d| Some(d) != branch.as_ref()) {
        if let Some(on_default) = commits(default.clone()).await.filter(|c| !c.is_empty()) {
            parts.push(format!("{} new commit(s) on {default} ({})", on_default.len(), on_default.join("; ")));
            // Exit 1 with conflicts, 0 clean; anything else is no answer.
            let merge = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["merge-tree", "--write-tree", "--quiet", "HEAD", &default])
                .output()
                .await
                .ok();
            if merge.is_some_and(|m| m.status.code() == Some(1)) {
                parts.push(format!("merging {default} into this branch now conflicts"));
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// What `release` did with one worktree (`#178`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Release {
    /// Removed. Its branch was deleted too when every commit on it is on a
    /// remote or another branch; otherwise the branch stays, holding
    /// `unpushed` commits nothing else has.
    Removed { branch_deleted: bool, unpushed: u32 },
    /// Left exactly as it is, and why -- work in it that removing would
    /// throw away, or git refusing.
    Kept { reason: String },
    /// Not a worktree of the scope any more: removed by hand, or never made.
    Gone,
}

/// Release a run's worktree once nothing will resume in it (`#178`) --
/// carefully, unlike `remove`, which is the bench's evidence-clearing
/// variant. Nothing that is not already safe elsewhere is thrown away:
///
/// * uncommitted changes (tracked or untracked; ignored build output like
///   `target/` does not count) keep the worktree, untouched -- refusing is
///   the triage's choice over pushing a salvage branch, which would be an
///   external side effect;
/// * a clean worktree is removed with a plain `git worktree remove` (never
///   `--force`), and its branch deleted only when every commit on it is
///   also on a remote-tracking ref or another local branch. A branch with
///   commits nothing else has stays, so the commits do too.
pub async fn release(scope_path: &Path, dir: &Path, branch: Option<&str>) -> Release {
    if !is_registered(scope_path, dir).await {
        return Release::Gone;
    }
    match git_read(dir, &["status", "--porcelain"]).await {
        None => return Release::Kept { reason: "git could not say whether it holds uncommitted work".into() },
        Some(status) if !status.is_empty() => {
            return Release::Kept {
                reason: format!("{} uncommitted change(s) in it", status.lines().count()),
            };
        }
        Some(_) => {}
    }
    let unpushed = match branch {
        Some(branch) => {
            let head = format!("refs/heads/{branch}");
            // `--exclude` before `--branches` takes the name as `--branches`
            // sees it, without `refs/heads/` -- spelled with it, it excludes
            // nothing, and the branch's own commits count as "elsewhere".
            let exclude = format!("--exclude={branch}");
            git_read(scope_path, &["rev-list", "--count", &head, "--not", &exclude, "--branches", "--remotes"])
                .await
                .and_then(|n| n.parse::<u32>().ok())
        }
        None => Some(0),
    };
    let output = Command::new("git").arg("-C").arg(scope_path).args(["worktree", "remove"]).arg(dir).output().await;
    match output {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Release::Kept { reason: format!("git worktree remove refused: {stderr}") };
        }
        Err(e) => return Release::Kept { reason: format!("running git: {e}") },
    }
    let branch_deleted = match (branch, unpushed) {
        (Some(branch), Some(0)) => Command::new("git")
            .arg("-C")
            .arg(scope_path)
            .args(["branch", "-D", branch])
            .output()
            .await
            .is_ok_and(|o| o.status.success()),
        _ => false,
    };
    Release::Removed { branch_deleted, unpushed: unpushed.unwrap_or(0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// A scope repository on `main` with one commit, and a worktree of it on
    /// `work` -- the shape every run's worktree has.
    fn scope_with_worktree() -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("factory-worktree-test-{}", uuid::Uuid::new_v4()));
        let scope = root.join("scope");
        std::fs::create_dir_all(&scope).unwrap();
        git(&scope, &["init", "-q", "-b", "main"]);
        std::fs::write(scope.join("a.txt"), "one\n").unwrap();
        git(&scope, &["add", "."]);
        git(&scope, &["commit", "-q", "-m", "first"]);
        let wt = root.join("wt");
        git(&scope, &["worktree", "add", "-q", "-b", "work", wt.to_str().unwrap()]);
        (scope, wt)
    }

    #[tokio::test]
    async fn release_refuses_uncommitted_work_and_keeps_a_branch_with_commits_nothing_else_has() {
        // Uncommitted work: the worktree stays exactly as it is.
        let (scope, wt) = scope_with_worktree();
        std::fs::write(wt.join("new.txt"), "draft\n").unwrap();
        match release(&scope, &wt, Some("work")).await {
            Release::Kept { reason } => assert!(reason.contains("1 uncommitted change"), "{reason}"),
            other => panic!("expected kept, got {other:?}"),
        }
        assert!(wt.join("new.txt").exists());

        // Committed but on no other branch: removed, the branch kept.
        git(&wt, &["add", "."]);
        git(&wt, &["commit", "-q", "-m", "work"]);
        assert_eq!(release(&scope, &wt, Some("work")).await, Release::Removed { branch_deleted: false, unpushed: 1 });
        assert!(!wt.exists());
        git(&scope, &["rev-parse", "--verify", "refs/heads/work"]);
        assert_eq!(release(&scope, &wt, Some("work")).await, Release::Gone, "a second release finds nothing");

        // Everything on it is on main too: removed, and the branch with it.
        let (scope, wt) = scope_with_worktree();
        assert_eq!(release(&scope, &wt, Some("work")).await, Release::Removed { branch_deleted: true, unpushed: 0 });
        let out = std::process::Command::new("git").arg("-C").arg(&scope).args(["rev-parse", "--verify", "-q", "refs/heads/work"]).output().unwrap();
        assert!(!out.status.success(), "the branch had nothing of its own");
    }

    #[tokio::test]
    async fn changes_since_names_new_commits_on_both_sides_and_a_conflict() {
        let (scope, wt) = scope_with_worktree();
        let since = chrono::Utc::now() - chrono::Duration::seconds(2);
        assert_eq!(
            changes_since(&wt, &scope, chrono::Utc::now() + chrono::Duration::seconds(5)).await,
            None,
            "nothing moved after the previous run"
        );
        std::fs::write(wt.join("a.txt"), "branch\n").unwrap();
        git(&wt, &["commit", "-q", "-am", "on the branch"]);
        std::fs::write(scope.join("a.txt"), "main\n").unwrap();
        git(&scope, &["commit", "-q", "-am", "on main"]);
        let said = changes_since(&wt, &scope, since).await.expect("both sides moved");
        assert!(said.contains("on work") && said.contains("on the branch"), "{said}");
        assert!(said.contains("on main (") && said.contains("on main"), "{said}");
        assert!(said.contains("merging main into this branch now conflicts"), "{said}");
    }

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
