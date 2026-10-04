//! A release comparison is captured once, at recording. Live build/SBOM reads
//! belong to their producers and are composed only by the release-detail facade.
use super::*;
use crate::facts::{Facts, ReleaseBuildQuery, ReleaseSbomQuery};
use factory_core::environments::{ReleaseChange, ReleaseChanges, ReleaseDetail, ReleaseQuery};
use tokio::io::AsyncReadExt;

const MAX_CHANGES: usize = 200;
const MAX_GIT_BYTES: u64 = 256 * 1024;

async fn git(dir: &std::path::Path, arguments: &[&str]) -> std::result::Result<String, String> {
    let mut child = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Git could not start".to_string())?;
    let read = async {
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or("Git has no output")?
            .take(MAX_GIT_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Git output could not be read")?;
        if bytes.len() as u64 > MAX_GIT_BYTES {
            return Err("Git comparison exceeds the bounded output limit");
        }
        if !child.wait().await.map_err(|_| "Git could not finish")?.success() {
            return Err("Git revision or comparison is unavailable");
        }
        String::from_utf8(bytes).map_err(|_| "Git output is not UTF-8")
    };
    tokio::time::timeout(GIT_TIMEOUT, read)
        .await
        .map_err(|_| "Git comparison timed out".to_string())?
        .map_err(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_run(root: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git").arg("-C").arg(root).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn reference_kinds_and_github_origins_are_conservative() {
        let change = change(
            "sha",
            "Merge pull request #42 from feature",
            "Fixes #8\nRefs #99 and foreign/repo#100, #0, #18446744073709551616",
        );
        assert_eq!(change.pull_requests, vec![42]);
        assert_eq!(change.issues, vec![8]);
        assert_eq!(change.references, vec![99]);
        for origin in
            ["https://github.com/owner/repo.git", "git@github.com:owner/repo.git", "ssh://git@github.com/owner/repo"]
        {
            assert_eq!(github_repository(origin).as_deref(), Some("owner/repo"));
        }
        for origin in [
            "https://github.com.evil/owner/repo",
            "https://user:secret@github.com/owner/repo",
            "git@github.com:../..",
            "/tmp/repo",
            "javascript:evil",
        ] {
            assert_eq!(github_repository(origin), None);
        }
    }

    #[tokio::test]
    async fn comparison_is_frozen_with_full_revisions_and_missing_evidence_is_not_invented() {
        let (engine, root) =
            super::super::tests::engine_with("  - name: prod\n    checks: [{ kind: command, command: 'true' }]\n");
        git_run(&root, &["init", "--quiet"]);
        git_run(&root, &["config", "user.email", "release@example.invalid"]);
        git_run(&root, &["config", "user.name", "Release QA"]);
        git_run(&root, &["remote", "add", "origin", "https://github.com/owner/repo.git"]);
        git_run(&root, &["commit", "--quiet", "--allow-empty", "-m", "baseline"]);
        let base = git_run(&root, &["rev-parse", "HEAD"]);
        engine
            .release_add(ReleaseAdd {
                scope: "company".into(),
                release: ReleaseFacts { commit: base.clone(), ..Default::default() },
            })
            .await
            .unwrap();
        std::fs::write(root.join("product-source"), "release source\n").unwrap();
        git_run(&root, &["add", "product-source"]);
        git_run(
            &root,
            &[
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "Merge pull request #42 from feature",
                "-m",
                "Fixes #8\nRefs #99",
            ],
        );
        let expected = git_run(&root, &["rev-parse", "HEAD"]);
        let (_, release) = engine
            .release_add(ReleaseAdd {
                scope: "company".into(),
                release: ReleaseFacts {
                    commit: "HEAD".into(),
                    build_run: Some("missing-build".into()),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        assert_eq!(release.commit, expected);
        assert_eq!(release.build_scope.as_deref(), Some("company"));
        let comparison = release.changes.unwrap();
        assert_eq!(comparison.base.as_deref(), Some(base.as_str()));
        assert_eq!(comparison.commits.len(), 1);
        assert_eq!(comparison.commits[0].pull_requests, vec![42]);
        assert_eq!(comparison.commits[0].issues, vec![8]);
        assert_eq!(comparison.commits[0].references, vec![99]);
        assert!(comparison.unavailable.is_none());
        assert!(comparison.diffstat.as_ref().unwrap().contains("1 file changed"));
        let (_, rollback) = engine
            .release_add(ReleaseAdd {
                scope: "company".into(),
                release: ReleaseFacts {
                    commit: base.clone(),
                    compare_to: Some(expected.clone()),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let rollback = rollback.changes.unwrap();
        assert!(rollback.commits.is_empty());
        assert_eq!(rollback.removed_commits.len(), 1);
        assert_eq!(rollback.removed_commits[0].pull_requests, vec![42]);
        assert!(rollback.diffstat.as_ref().unwrap().contains("deletion"));
        std::fs::rename(root.join(".git"), root.join("unavailable.git")).unwrap();
        let query = ReleaseQuery { scope: "company".into(), commit: expected.clone(), deployment: None };
        let detail = engine.release_detail(query.clone()).await.unwrap();
        assert_eq!(detail.changes, Some(comparison.clone()), "reading is not a new Git observation");
        assert!(detail.build.is_none() && detail.build_reason.is_some());
        assert!(detail.sboms.is_empty() && detail.sbom_reason.is_some());
        assert!(engine
            .release_detail(ReleaseQuery { deployment: Some("not-a-deployment".into()), ..query.clone() })
            .await
            .is_err());
        assert!(engine.release_detail(ReleaseQuery { scope: "other".into(), ..query.clone() }).await.is_err());
        let reopened = EnvironmentStore::open(&root.join(".factory/factory.sqlite")).unwrap();
        let records = reopened.releases_added().await.unwrap();
        assert_eq!(
            records.iter().find(|(_, release, _)| release.commit == expected).unwrap().1.changes,
            Some(comparison)
        );
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn an_unavailable_comparison_is_explicit_and_a_wire_caller_cannot_forge_it() {
        let (engine, root) =
            super::super::tests::engine_with("  - name: prod\n    checks: [{ kind: command, command: 'true' }]\n");
        let (_, release) = engine
            .release_add(ReleaseAdd {
                scope: "company".into(),
                release: ReleaseFacts {
                    commit: "a".repeat(40),
                    changes: Some(ReleaseChanges {
                        base: Some("b".repeat(40)),
                        commit: "a".repeat(40),
                        repository: Some("owner/repo".into()),
                        commits: vec![],
                        removed_commits: vec![],
                        diffstat: None,
                        truncated: false,
                        unavailable: None,
                    }),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        assert!(release.changes.unwrap().unavailable.is_some());
        assert!(engine
            .release_add(ReleaseAdd {
                scope: "company".into(),
                release: ReleaseFacts { commit: "b".repeat(40), build_run: Some(" ".into()), ..Default::default() }
            })
            .await
            .is_err());
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }
}

async fn resolve(dir: &std::path::Path, revision: &str) -> std::result::Result<String, String> {
    let commit = git(dir, &["rev-parse", "--verify", "--end-of-options", &format!("{revision}^{{commit}}")]).await?;
    let commit = commit.trim();
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Git did not resolve a full commit".into());
    }
    Ok(commit.to_owned())
}

/// Only exact github.com origins become hyperlinks; arbitrary remotes never do.
fn github_repository(origin: &str) -> Option<String> {
    let path = origin
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| origin.trim().strip_prefix("git@github.com:"))
        .or_else(|| origin.trim().strip_prefix("ssh://git@github.com/"))?;
    let path = path.trim_end_matches('/').strip_suffix(".git").unwrap_or(path.trim_end_matches('/'));
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        })
    {
        return None;
    }
    Some(path.into())
}

fn change(commit: &str, subject: &str, body: &str) -> ReleaseChange {
    let text = format!("{subject}\n{body}");
    let mut references = BTreeSet::new();
    let mut pull_requests = BTreeSet::new();
    let mut issues = BTreeSet::new();
    for (index, _) in text.match_indices('#') {
        let before = &text[..index];
        if before.as_bytes().last().is_some_and(|byte| byte.is_ascii_alphanumeric() || b"/_-".contains(byte)) {
            continue;
        }
        let digits: String = text[index + 1..].chars().take_while(char::is_ascii_digit).collect();
        let Ok(number) = digits.parse::<u64>() else {
            continue;
        };
        if number == 0 {
            continue;
        }
        let prefix = before.trim_end().to_ascii_lowercase();
        if prefix.ends_with("merge pull request") {
            pull_requests.insert(number);
        } else if ["close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved"]
            .iter()
            .any(|word| prefix.split_whitespace().last() == Some(word))
        {
            issues.insert(number);
        } else {
            references.insert(number);
        }
    }
    for known in pull_requests.iter().chain(&issues) {
        references.remove(known);
    }
    ReleaseChange {
        commit: commit.into(),
        subject: subject.into(),
        pull_requests: pull_requests.into_iter().collect(),
        issues: issues.into_iter().collect(),
        references: references.into_iter().collect(),
    }
}

async fn comparison(
    dir: &std::path::Path,
    base: &str,
    commit: &str,
) -> std::result::Result<(Vec<ReleaseChange>, bool), String> {
    let range = format!("{base}..{commit}");
    let output = git(dir, &["log", "--topo-order", "-n", "201", "--format=%H%x00%s%x00%b%x00", &range, "--"]).await?;
    let pieces: Vec<_> = output.split('\0').collect();
    let mut changes = Vec::new();
    for fields in pieces.chunks(3) {
        if fields.len() < 3 {
            break;
        }
        changes.push(change(fields[0].trim(), fields[1], fields[2]));
    }
    let truncated = changes.len() > MAX_CHANGES;
    changes.truncate(MAX_CHANGES);
    Ok((changes, truncated))
}

impl Engine {
    pub(super) async fn enrich_release(
        &self,
        scope: &str,
        release: &mut ReleaseFacts,
        previous: Option<&str>,
    ) -> Result<()> {
        let snapshot = self.factory_snapshot();
        if let Some(run) = &release.build_run {
            if run.trim().is_empty() || run.len() > 200 {
                return Err(FactoryError::BadRequest(
                    "build_run must be a nonempty run id of at most 200 bytes".into(),
                ));
            }
            let build_scope = release.build_scope.get_or_insert_with(|| scope.to_owned());
            snapshot.scope(build_scope)?;
        } else if release.build_scope.is_some() {
            return Err(FactoryError::BadRequest("build_scope needs an explicit build_run".into()));
        }
        let mut evidence = ReleaseChanges {
            base: release.compare_to.clone().or_else(|| previous.map(str::to_owned)),
            commit: release.commit.clone(),
            repository: None,
            commits: vec![],
            removed_commits: vec![],
            diffstat: None,
            truncated: false,
            unavailable: None,
        };
        let capture = async {
            let dir = snapshot.scope_path(scope).map_err(|_| "scope repository is unavailable".to_string())?;
            let commit = resolve(&dir, &release.commit).await?;
            release.commit = commit.clone();
            evidence.commit = commit.clone();
            if release.committed_at.is_none() {
                release.committed_at = committed_at(dir.clone(), commit.clone()).await;
            }
            if let Ok(origin) = git(&dir, &["remote", "get-url", "origin"]).await {
                evidence.repository = github_repository(&origin);
            }
            let asked = evidence
                .base
                .clone()
                .ok_or_else(|| "no previous release; select --compare-to to capture a range".to_string())?;
            let base = resolve(&dir, &asked).await?;
            evidence.base = Some(base.clone());
            let (commits, truncated) = comparison(&dir, &base, &commit).await?;
            evidence.commits = commits;
            let (removed, removed_truncated) = comparison(&dir, &commit, &base).await?;
            evidence.removed_commits = removed;
            evidence.truncated = truncated || removed_truncated;
            evidence.diffstat = Some(
                git(&dir, &["diff", "--shortstat", "--no-ext-diff", "--no-textconv", &base, &commit, "--"])
                    .await?
                    .trim()
                    .into(),
            );
            Ok::<_, String>(())
        }
        .await;
        if let Err(reason) = capture {
            evidence.unavailable = Some(reason);
        }
        // A wire caller cannot supply a forged daemon-captured Git comparison.
        release.changes = Some(evidence);
        Ok(())
    }

    pub(crate) async fn release_detail(&self, query: ReleaseQuery) -> Result<ReleaseDetail> {
        let snapshot = self.factory_snapshot();
        snapshot.scope(&query.scope)?;
        let report = self.environment_report(Some(query.scope.clone()), false).await?;
        let mut release = report
            .releases
            .into_iter()
            .find(|release| release.scope == query.scope && release.facts.commit == query.commit)
            .ok_or_else(|| FactoryError::BadRequest("release is not recorded in the selected scope".into()))?;
        let facts = if let Some(id) = &query.deployment {
            let deployment = self
                .environments
                .deployment(id)
                .await?
                .filter(|deployment| deployment.scope == query.scope && deployment.release.commit == query.commit)
                .ok_or_else(|| {
                    FactoryError::BadRequest("deployment does not belong to the selected release and scope".into())
                })?;
            deployment.release
        } else {
            release.facts.clone()
        };
        // A selected attempt's version/build identity must agree with its evidence,
        // even when the catalogue aggregates other attempts at the same commit.
        release.facts = facts.clone();
        let reader = Facts::<factory_kernel::People>::new(self);
        let mut build = None;
        let build_reason = if let Some(run_id) = &facts.build_run {
            let asked = ReleaseBuildQuery {
                scope: facts.build_scope.clone().unwrap_or_else(|| query.scope.clone()),
                commit: query.commit.clone(),
                run_id: run_id.clone(),
            };
            match reader.get::<factory_kernel::ReleaseBuildFact>(&asked).await {
                Ok(evidence) => {
                    build = evidence;
                    build
                        .is_none()
                        .then(|| "selected run has no completed, clean, source-matching artifact provenance".into())
                }
                Err(error) => Some(format!("build evidence could not be read: {error}")),
            }
        } else {
            Some("no producing build run was recorded; a deployment actor is not a build".into())
        };
        let asked = ReleaseSbomQuery {
            scope: facts.build_scope.clone().unwrap_or_else(|| query.scope.clone()),
            commit: query.commit,
            version: facts.version.clone(),
        };
        let (sboms, sbom_reason) = match reader.get::<factory_kernel::ReleaseSbomFact>(&asked).await {
            Ok(sboms) => {
                let reason = sboms
                    .is_empty()
                    .then(|| "no build SBOM attachment matches this exact product commit/version".into());
                (sboms, reason)
            }
            Err(error) => (vec![], Some(format!("SBOM evidence could not be read: {error}"))),
        };
        Ok(ReleaseDetail { changes: facts.changes, release, build, sboms, build_reason, sbom_reason })
    }
}
