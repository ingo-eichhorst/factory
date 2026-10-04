//! Bounded, read-only git context for an agent returning to a conversation.
use factory_core::run::ResumeContext;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

async fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(3), command.output())
        .await
        .ok()?
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(crate) async fn checkpoint(dir: &Path, fingerprint: String, resumes: u32) -> ResumeContext {
    let main_head = match git(dir, &["rev-parse", "--verify", "refs/remotes/origin/main"]).await {
        Some(head) => Some(head),
        None => git(dir, &["rev-parse", "--verify", "refs/heads/main"]).await,
    };
    ResumeContext {
        fingerprint,
        branch_head: git(dir, &["rev-parse", "--verify", "HEAD"]).await,
        main_head,
        resumes,
    }
}

pub(crate) async fn on_branch(dir: &Path, branch: &str) -> bool {
    git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
        .as_deref()
        == Some(branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn checkpoint_and_changes_are_read_only_and_unknown_is_explicit() {
        let directory =
            std::env::temp_dir().join(format!("factory-resume-git-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        assert!(git(&directory, &["init", "--initial-branch=main"])
            .await
            .is_some());
        assert!(git(
            &directory,
            &[
                "-c",
                "user.name=QA",
                "-c",
                "user.email=qa@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "before"
            ]
        )
        .await
        .is_some());
        let before = checkpoint(&directory, "guide".into(), 2).await;
        assert!(on_branch(&directory, "main").await);
        assert!(!on_branch(&directory, "changed").await);
        assert_eq!(
            changes(
                &directory,
                before.branch_head.as_deref(),
                before.branch_head.as_deref()
            )
            .await,
            "none"
        );
        assert!(git(
            &directory,
            &[
                "-c",
                "user.name=QA",
                "-c",
                "user.email=qa@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "after"
            ]
        )
        .await
        .is_some());
        let after = checkpoint(&directory, "guide".into(), 3).await;
        let commits = changes(
            &directory,
            before.branch_head.as_deref(),
            after.branch_head.as_deref(),
        )
        .await;
        assert!(commits.contains("after"));
        assert!(!commits.contains("before"));
        assert!(changes(&directory, None, after.main_head.as_deref())
            .await
            .contains("unknown"));
        assert_eq!(
            git(&directory, &["status", "--porcelain"]).await.as_deref(),
            Some("")
        );
        assert_eq!(
            git(&directory, &["rev-parse", "HEAD"]).await,
            after.branch_head
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}

async fn changes(dir: &Path, before: Option<&str>, after: Option<&str>) -> String {
    let (Some(before), Some(after)) = (before, after) else {
        return "unknown (no recorded git baseline)".into();
    };
    if before == after {
        return "none".into();
    }
    let range = format!("{before}..{after}");
    match git(dir, &["log", "--oneline", "--max-count=12", &range]).await {
        Some(rows) if !rows.is_empty() => rows,
        _ => "changed, but commits could not be read (history may have been rewritten)".into(),
    }
}

pub(crate) async fn summary(
    dir: &Path,
    previous: &ResumeContext,
    current: &ResumeContext,
) -> String {
    let branch = changes(
        dir,
        previous.branch_head.as_deref(),
        current.branch_head.as_deref(),
    )
    .await;
    let main = changes(
        dir,
        previous.main_head.as_deref(),
        current.main_head.as_deref(),
    )
    .await;
    let mut command = Command::new("gh");
    command
        .args(["pr", "view", "--json", "mergeable", "--jq", ".mergeable"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let conflicts = match tokio::time::timeout(Duration::from_secs(3), command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            match String::from_utf8_lossy(&output.stdout).trim() {
                "MERGEABLE" => "GitHub reports mergeable",
                "CONFLICTING" => "GitHub reports merge conflicts",
                _ => "unknown (GitHub has not computed mergeability)",
            }
        }
        _ => "unknown (no readable PR or GitHub unavailable)",
    };
    format!("Branch commits since the previous run:\n{branch}\nMain commits since the previous run:\n{main}\nPR conflicts: {conflicts}. No merge, fetch or reset was performed.")
}
