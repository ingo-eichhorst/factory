//! `#155`: for every registered scope, what `git` itself says about its
//! repository's current branch -- its remote and how many commits are not
//! on it, as of the last fetch -- and, on macOS, whether Time Machine has a
//! destination configured. Both are read-only host facts for L1 Backup:
//! this never fetches, pushes, configures a remote or touches Time Machine.
//!
//! Two scopes whose directories are the same repository (a nested scope
//! sharing the root's checkout, say) are probed once: [`repository_facts`]
//! groups by `git`'s own `--show-toplevel` answer before it asks anything
//! else, and the resulting [`RepositoryFact`] lists every scope that shares
//! it.
//!
//! **Never takes the daemon down.** Every probe runs through [`run`], which
//! applies a timeout and `kill_on_drop` the same way `github_intake.rs`
//! runs `gh` -- a slow disk, a missing binary or a repository `git` refuses
//! to touch becomes `inspection_failed`/`unavailable` for that one row, and
//! `GET /api/backup` still answers.

use std::path::{Path, PathBuf};
use std::time::Duration;

use factory_core::backup::{parse_destinationinfo, redact_remote, RepositoryFact, RepositoryState, TimeMachineFact};
use factory_core::config::Scope;

/// A handful of seconds: long enough for `git` or `tmutil` on a slow disk,
/// short enough that one unresponsive repository never delays the page.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

/// One subprocess, with a timeout and `kill_on_drop` so a probe that hangs
/// is killed rather than orphaned. Never panics, never inherits stdin.
async fn run(program: &Path, args: &[&str], cwd: Option<&Path>, timeout: Duration) -> Result<Output, String> {
    let mut command = tokio::process::Command::new(program);
    command.kill_on_drop(true);
    command.stdin(std::process::Stdio::null());
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| format!("{} timed out after {}s", program.display(), timeout.as_secs()))?
        .map_err(|error| format!("starting {}: {error}", program.display()))?;
    Ok(Output {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn command_failed(context: &str, out: &Output) -> String {
    if out.stderr.is_empty() {
        format!("{context} exited with {}", if out.success { "no output" } else { "an error" })
    } else {
        format!("{context}: {}", out.stderr)
    }
}

// ------------------------------------------------------------- repositories

/// Every registered scope's repository fact: the real `git` binary, one
/// probe per distinct repository.
pub(crate) async fn repository_facts(root: &Path, scopes: &[Scope]) -> Vec<RepositoryFact> {
    repository_facts_with(Path::new("git"), root, scopes).await
}

/// Testable with an injected `git` program path.
async fn repository_facts_with(git: &Path, root: &Path, scopes: &[Scope]) -> Vec<RepositoryFact> {
    // `git rev-parse --show-toplevel` resolves symlinks; match it against a
    // canonicalized root so two scopes in the real instance still group
    // (and so `path` is relative, not the absolute toplevel).
    let canon_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());

    // Resolve every scope's directory concurrently: either a toplevel, or
    // the reason there is none. `scope.path` is already relative to `root`
    // (`discovery.rs`), so a row with no toplevel to resolve just uses it
    // as is -- no canonicalizing a directory that may not even exist.
    let resolved = futures_util::future::join_all(scopes.iter().map(|scope| {
        let dir = root.join(&scope.path);
        let scope_path = scope.path.display().to_string();
        async move {
            if !dir.is_dir() {
                return (scope.name.clone(), scope_path, Err(RepositoryState::NoDirectory));
            }
            match run(git, &["rev-parse", "--show-toplevel"], Some(&dir), PROBE_TIMEOUT).await {
                Ok(out) if out.success && !out.stdout.is_empty() => {
                    (scope.name.clone(), scope_path, Ok(PathBuf::from(out.stdout)))
                }
                Ok(_) => (scope.name.clone(), scope_path, Err(RepositoryState::NotARepository)),
                Err(error) => (scope.name.clone(), scope_path, Err(RepositoryState::InspectionFailed { reason: error })),
            }
        }
    }))
    .await;

    // Group the ones with a repository by toplevel, preserving the order
    // each was first seen in; everything else is its own row already.
    let mut groups: Vec<(PathBuf, Vec<String>)> = Vec::new();
    let mut facts: Vec<RepositoryFact> = Vec::new();
    for (name, path, outcome) in resolved {
        match outcome {
            Ok(toplevel) => match groups.iter_mut().find(|(t, _)| *t == toplevel) {
                Some((_, names)) => names.push(name),
                None => groups.push((toplevel, vec![name])),
            },
            Err(state) => facts.push(RepositoryFact { scopes: vec![name], path, state, remote_url: None }),
        }
    }

    // Probe each distinct repository once, concurrently.
    let probed = futures_util::future::join_all(groups.iter().map(|(toplevel, _)| probe_repo(git, toplevel))).await;
    for ((toplevel, scope_names), (state, remote_url)) in groups.into_iter().zip(probed) {
        facts.push(RepositoryFact {
            scopes: scope_names,
            path: relative_display(&canon_root, &toplevel),
            state,
            remote_url,
        });
    }
    facts
}

/// `path`, relative to `root` when it is underneath it -- `.` for `root`
/// itself, matching how a scope's own `path` spells the instance root
/// (`discovery.rs`) -- or its own display otherwise (a repository this
/// instance does not contain, which should not happen but is not this
/// function's place to refuse).
fn relative_display(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// The current branch's state in the repository at `toplevel`, and its
/// tracked remote's redacted URL when there is one. Exactly the six-step
/// sequence the design settled on -- `symbolic-ref`, `rev-parse --verify`,
/// `remote`, `rev-parse --abbrev-ref @{upstream}`, `rev-list --count`, then
/// `remote get-url` -- and never a seventh that would fetch anything.
async fn probe_repo(git: &Path, toplevel: &Path) -> (RepositoryState, Option<String>) {
    let symbolic = match run(git, &["symbolic-ref", "-q", "HEAD"], Some(toplevel), PROBE_TIMEOUT).await {
        Ok(out) => out,
        Err(error) => return (RepositoryState::InspectionFailed { reason: error }, None),
    };
    if !symbolic.success {
        return (RepositoryState::DetachedHead, None);
    }

    let head = match run(git, &["rev-parse", "--verify", "-q", "HEAD"], Some(toplevel), PROBE_TIMEOUT).await {
        Ok(out) => out,
        Err(error) => return (RepositoryState::InspectionFailed { reason: error }, None),
    };
    if !head.success {
        return (RepositoryState::NoCommits, None);
    }

    let remote_list = match run(git, &["remote"], Some(toplevel), PROBE_TIMEOUT).await {
        Ok(out) => out,
        Err(error) => return (RepositoryState::InspectionFailed { reason: error }, None),
    };
    if !remote_list.success {
        return (RepositoryState::InspectionFailed { reason: command_failed("git remote", &remote_list) }, None);
    }
    let has_remote = remote_list.stdout.lines().any(|line| !line.trim().is_empty());
    if !has_remote {
        return (RepositoryState::NoRemote, None);
    }

    let upstream_out = match run(git, &["rev-parse", "--abbrev-ref", "@{upstream}"], Some(toplevel), PROBE_TIMEOUT).await {
        Ok(out) => out,
        Err(error) => return (RepositoryState::InspectionFailed { reason: error }, None),
    };
    if !upstream_out.success {
        return (RepositoryState::NoUpstream, None);
    }
    let upstream = upstream_out.stdout;
    let remote_name = upstream.split('/').next().unwrap_or_default().to_string();

    let ahead_out = match run(git, &["rev-list", "--count", "@{upstream}..HEAD"], Some(toplevel), PROBE_TIMEOUT).await {
        Ok(out) => out,
        Err(error) => return (RepositoryState::InspectionFailed { reason: error }, None),
    };
    if !ahead_out.success {
        return (RepositoryState::InspectionFailed { reason: command_failed("git rev-list --count", &ahead_out) }, None);
    }
    let ahead: u64 = ahead_out.stdout.parse().unwrap_or(0);

    // Best-effort only: a `remote get-url` that fails or times out never
    // fails the row, and its output -- the only place a secret could be --
    // is used only when the call actually succeeded, redacted before it is
    // kept anywhere.
    let remote_url = if remote_name.is_empty() {
        None
    } else {
        match run(git, &["remote", "get-url", &remote_name], Some(toplevel), PROBE_TIMEOUT).await {
            Ok(out) if out.success && !out.stdout.is_empty() => Some(redact_remote(&out.stdout)),
            _ => None,
        }
    };

    (RepositoryState::Tracked { remote: remote_name, upstream, ahead }, remote_url)
}

// ------------------------------------------------------------- Time Machine

/// The real platform's Time Machine fact -- `tmutil destinationinfo` on
/// macOS, [`TimeMachineFact::Unsupported`] everywhere else, exactly the
/// `cfg(target_os = "macos")` split `host.rs` uses.
pub(crate) async fn time_machine_fact() -> TimeMachineFact {
    platform::time_machine_fact().await
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{probe_time_machine, TimeMachineFact, PROBE_TIMEOUT};
    use std::path::Path;

    pub(super) async fn time_machine_fact() -> TimeMachineFact {
        probe_time_machine(Path::new("tmutil"), PROBE_TIMEOUT).await
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::TimeMachineFact;

    pub(super) async fn time_machine_fact() -> TimeMachineFact {
        TimeMachineFact::Unsupported
    }
}

/// `tmutil destinationinfo` behind [`run`], parsed by the pure
/// `factory_core::backup::parse_destinationinfo`. Not `cfg`-gated itself --
/// only the real program path `platform::time_machine_fact` chooses is --
/// so a test can inject a fake `tmutil` on any host.
async fn probe_time_machine(tmutil: &Path, timeout: Duration) -> TimeMachineFact {
    match run(tmutil, &["destinationinfo"], None, timeout).await {
        Ok(out) => {
            let text = if out.stdout.is_empty() { &out.stderr } else { &out.stdout };
            parse_destinationinfo(text, out.success)
        }
        Err(reason) => TimeMachineFact::Unavailable { reason },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    async fn git(args: &[&str], dir: &Path) {
        assert!(
            tokio::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .await
                .unwrap()
                .success(),
            "git {args:?} in {}",
            dir.display()
        );
    }

    async fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(&["init", "-q"], dir).await;
        git(&["config", "user.email", "factory@example.com"], dir).await;
        git(&["config", "user.name", "factory"], dir).await;
    }

    async fn commit(dir: &Path, message: &str) {
        git(&["commit", "-q", "--allow-empty", "-m", message], dir).await;
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("factory-backup-repos-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn a_missing_directory_is_no_directory_without_ever_running_git() {
        let root = temp_dir("missing"); // never created
        let facts = repository_facts_with(Path::new("git"), &root, &[scope("s", "nowhere")]).await;
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].state, RepositoryState::NoDirectory);
        assert_eq!(facts[0].scopes, vec!["s".to_string()]);
    }

    #[tokio::test]
    async fn a_plain_directory_is_not_a_repository() {
        let dir = temp_dir("plain");
        std::fs::create_dir_all(&dir).unwrap();
        let facts = repository_facts_with(Path::new("git"), &dir, &[scope("s", ".")]).await;
        assert_eq!(facts[0].state, RepositoryState::NotARepository);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_repository_with_no_commit_has_no_commits() {
        let dir = temp_dir("no-commits");
        init_repo(&dir).await;
        let facts = repository_facts_with(Path::new("git"), &dir, &[scope("s", ".")]).await;
        assert_eq!(facts[0].state, RepositoryState::NoCommits);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_detached_head_is_reported_as_such() {
        let dir = temp_dir("detached");
        init_repo(&dir).await;
        commit(&dir, "base").await;
        git(&["checkout", "-q", "--detach", "HEAD"], &dir).await;
        let facts = repository_facts_with(Path::new("git"), &dir, &[scope("s", ".")]).await;
        assert_eq!(facts[0].state, RepositoryState::DetachedHead);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_repository_with_commits_and_no_remote_says_so() {
        let dir = temp_dir("no-remote");
        init_repo(&dir).await;
        commit(&dir, "base").await;
        let facts = repository_facts_with(Path::new("git"), &dir, &[scope("s", ".")]).await;
        assert_eq!(facts[0].state, RepositoryState::NoRemote);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_remote_with_no_tracking_branch_is_no_upstream() {
        let dir = temp_dir("no-upstream");
        init_repo(&dir).await;
        commit(&dir, "base").await;
        let bare = temp_dir("no-upstream-bare");
        git(&["init", "-q", "--bare", bare.to_str().unwrap()], &dir).await;
        git(&["remote", "add", "origin", bare.to_str().unwrap()], &dir).await;
        let facts = repository_facts_with(Path::new("git"), &dir, &[scope("s", ".")]).await;
        assert_eq!(facts[0].state, RepositoryState::NoUpstream);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&bare).ok();
    }

    #[tokio::test]
    async fn a_tracked_branch_ahead_by_one_reports_the_exact_count_and_the_redacted_remote() {
        let dir = temp_dir("tracked");
        init_repo(&dir).await;
        commit(&dir, "base").await;
        let bare = temp_dir("tracked-bare");
        git(&["init", "-q", "--bare", bare.to_str().unwrap()], &dir).await;
        // A secret-bearing URL only to prove it never survives to the fact:
        // `push` never contacts it, since a bare local path never reads it.
        let bare_url = format!("file://{}", bare.display());
        git(&["remote", "add", "origin", &bare_url], &dir).await;
        git(&["push", "-q", "-u", "origin", "HEAD"], &dir).await;
        commit(&dir, "second").await; // one commit ahead of the pushed upstream

        let facts = repository_facts_with(Path::new("git"), &dir, &[scope("s", ".")]).await;
        assert_eq!(facts.len(), 1);
        match &facts[0].state {
            RepositoryState::Tracked { remote, ahead, .. } => {
                assert_eq!(remote, "origin");
                assert_eq!(*ahead, 1);
            }
            other => panic!("expected Tracked, got {other:?}"),
        }
        assert_eq!(facts[0].remote_url.as_deref(), Some(bare_url.as_str()), "a file:// URL has no userinfo to redact");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&bare).ok();
    }

    #[tokio::test]
    async fn a_secret_in_the_remote_url_is_redacted_before_it_is_kept() {
        let dir = temp_dir("secret");
        init_repo(&dir).await;
        commit(&dir, "base").await;
        let bare = temp_dir("secret-bare");
        std::fs::create_dir_all(&bare).unwrap();
        git(&["init", "-q", "--bare"], &bare).await;
        git(&["remote", "add", "origin", bare.to_str().unwrap()], &dir).await;
        git(&["push", "-q", "-u", "origin", "HEAD"], &dir).await;
        // Now repoint the remote at a URL carrying a token, as a real scope's
        // GitHub remote might: never fetched or pushed to by this probe, so
        // the token is never contacted, only read back and redacted.
        git(&["remote", "set-url", "origin", "https://x-access-token:SECRET-TOKEN@github.com/o/r.git"], &dir).await;

        let facts = repository_facts_with(Path::new("git"), &dir, &[scope("s", ".")]).await;
        let remote_url = facts[0].remote_url.clone().unwrap();
        assert_eq!(remote_url, "https://github.com/o/r.git");
        assert!(!remote_url.contains("SECRET-TOKEN"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&bare).ok();
    }

    #[tokio::test]
    async fn two_scopes_in_one_repository_are_probed_once_and_share_the_fact() {
        let dir = temp_dir("shared");
        init_repo(&dir).await;
        commit(&dir, "base").await;
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested/.keep"), b"").unwrap();

        let facts = repository_facts_with(Path::new("git"), &dir, &[scope("root", "."), scope("nested", "nested")]).await;
        assert_eq!(facts.len(), 1, "one repository, one row: {facts:?}");
        let mut scopes = facts[0].scopes.clone();
        scopes.sort();
        assert_eq!(scopes, vec!["nested".to_string(), "root".to_string()]);
        assert_eq!(facts[0].state, RepositoryState::NoRemote);
    }

    fn scope(name: &str, path: &str) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(&format!("id: {name}-id\nname: {name}\n")).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    fn fake_script(name: &str, body: &str) -> PathBuf {
        let path = temp_dir(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tmutil_that_sleeps_past_its_timeout_is_unavailable() {
        let script = fake_script("sleepy-tmutil", "#!/bin/sh\nsleep 5\n");
        let fact = probe_time_machine(&script, Duration::from_millis(200)).await;
        assert!(matches!(fact, TimeMachineFact::Unavailable { .. }), "{fact:?}");
        std::fs::remove_file(&script).ok();
    }

    #[tokio::test]
    async fn a_missing_tmutil_binary_is_unavailable_not_a_panic() {
        let missing = temp_dir("no-such-tmutil-binary");
        let fact = probe_time_machine(&missing, PROBE_TIMEOUT).await;
        assert!(matches!(fact, TimeMachineFact::Unavailable { .. }), "{fact:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tmutil_that_says_nothing_is_configured_is_read_as_such() {
        let script = fake_script("off-tmutil", "#!/bin/sh\necho 'tmutil: No destinations configured.'\nexit 0\n");
        let fact = probe_time_machine(&script, PROBE_TIMEOUT).await;
        assert_eq!(fact, TimeMachineFact::NotConfigured);
        std::fs::remove_file(&script).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tmutil_that_exits_non_zero_is_unavailable_with_its_own_reason() {
        let script = fake_script("failing-tmutil", "#!/bin/sh\necho 'permission denied' >&2\nexit 1\n");
        let fact = probe_time_machine(&script, PROBE_TIMEOUT).await;
        let TimeMachineFact::Unavailable { reason } = fact else { panic!("expected Unavailable") };
        assert!(reason.contains("permission denied"), "{reason}");
        std::fs::remove_file(&script).ok();
    }
}
