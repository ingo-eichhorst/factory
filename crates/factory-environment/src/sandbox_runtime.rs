//! `sandbox: openshell`, the half that runs things (`#218`). Everything a
//! run's sandbox is -- its name, argv, policy, uploads, launcher -- is
//! decided in `crate::openshell::plan`; this module carries it out at
//! dispatch, tears it down when the run ends, and on start deletes any
//! sandbox a run left behind.
//!
//! Fail closed, always. A missing CLI, a gateway that does not answer, a
//! provider that does not exist, an image or policy the gateway will not
//! activate: every one of them is an `Err` out of `prepare`, which
//! the dispatch caller turns into a failed run with that reason. Nothing here
//! ever hands back a launch that would start the harness on the host.

use crate::openshell::{Plan, LABEL_INSTANCE, LABEL_RUN, SANDBOX_CLAUDE_PROJECTS};
use factory_kernel::error::{FactoryError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// The session-meta key `close_session` finds the teardown under.
pub const META_KEY: &str = "openshell";

/// Above this, a preserved conversation is not kept (`#274`): the point is
/// resuming a sandboxed claude-code run cheaply, not an unbounded transcript
/// store. The size is journaled and the next run falls back to fresh.
pub const PRESERVE_CAP_BYTES: u64 = 64 * 1024 * 1024;

/// A gateway answers status in well under a second; creating a sandbox
/// waits for the image (a first pull can take minutes) and for the
/// gateway's own provisioning window (300s) to reject a bad policy.
const QUICK: Duration = Duration::from_secs(30);
const CREATE: Duration = Duration::from_secs(30 * 60);
const TRANSFER: Duration = Duration::from_secs(10 * 60);

/// What `close_session` needs to finish a run's sandbox, kept on the
/// session itself so it survives a daemon restart between dispatch and the
/// run's end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Teardown {
    pub sandbox: String,
    pub state_dir: PathBuf,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<Vec<String>>,
    pub download_dir: PathBuf,
    #[serde(default)]
    pub fast_forward: bool,
    pub delete: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_evidence: Option<crate::service_observations::CaptureContext>,
    /// The task this run belongs to -- `#274`'s preserved conversation is
    /// kept per task, not per run, so a later run's preservation can find
    /// and replace an earlier one. Empty on a teardown recorded before this
    /// field existed; preservation is then skipped rather than guessed at.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub task: String,
    /// The sandbox working directory this run used, carried along so a
    /// later `--continue` can check a preserved session was captured from
    /// the same place it would now resume into.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workdir: String,
    /// `sandbox download <name> SANDBOX_CLAUDE_PROJECTS <incoming>/projects`,
    /// built once at dispatch from the plan actually used -- `Some` only for
    /// a claude-code run, which is the one harness `Agent::resume_spec`
    /// supports; `shell` has no conversation to preserve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve_download: Option<Vec<String>>,
}

impl Teardown {
    pub fn of(plan: &Plan, cwd: &Path, fast_forward: bool, task: &str) -> Self {
        let state_dir = plan
            .policy_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let preserve_download = (plan.launch.agent_kind.as_deref() == Some("claude")).then(|| {
            let sessions_root = sessions_root_of(&state_dir);
            let mut argv = plan.base.clone();
            argv.extend([
                "sandbox".to_string(),
                "download".to_string(),
                plan.sandbox.clone(),
                SANDBOX_CLAUDE_PROJECTS.to_string(),
                incoming_projects_dir(&sessions_root, task)
                    .display()
                    .to_string(),
            ]);
            argv
        });
        Self {
            sandbox: plan.sandbox.clone(),
            state_dir,
            cwd: cwd.to_path_buf(),
            download: plan.download.clone(),
            download_dir: plan.download_dir.clone(),
            fast_forward,
            delete: plan.delete.clone(),
            service_evidence: None,
            task: task.to_string(),
            workdir: plan.workdir.clone(),
            preserve_download,
        }
    }

    pub fn to_meta(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_meta(meta: &std::collections::BTreeMap<String, String>) -> Option<Self> {
        meta.get(META_KEY)
            .and_then(|raw| serde_json::from_str(raw).ok())
    }
}

/// `.factory/openshell/sessions/`, beside the per-run `state_dir`s --
/// never inside a scope, like every other daemon-owned OpenShell state.
/// `state_dir` is `<instance>/.factory/openshell/<run>`; its parent is the
/// shared `openshell` directory every one of these roots sits under.
fn sessions_root_of(state_dir: &Path) -> PathBuf {
    state_dir
        .parent()
        .map(|openshell_dir| openshell_dir.join("sessions"))
        .unwrap_or_default()
}

/// Where a task's preserved conversation lives once captured.
pub fn preserved_dir(sessions_root: &Path, task: &str) -> PathBuf {
    sessions_root.join(task)
}

/// Where a task's preserved conversation lands while it is being captured,
/// before it is promoted over the previous one -- a fixed name rather than
/// a random one, so `Pending::valid_at` can check it exactly and a crashed
/// capture leaves a name the next one reuses rather than litters.
pub fn incoming_dir(sessions_root: &Path, task: &str) -> PathBuf {
    sessions_root.join(format!("{task}-incoming"))
}

pub fn incoming_projects_dir(sessions_root: &Path, task: &str) -> PathBuf {
    incoming_dir(sessions_root, task).join("projects")
}

/// Written before create, outside the uploaded run files. Configuration
/// can disappear or move to another gateway while a sandbox still exists;
/// its cleanup destination must survive independently of that declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub instance: String,
    pub run: String,
    pub task: String,
    pub base: Vec<String>,
    pub teardown: Teardown,
}

impl Pending {
    pub fn save(&self) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let directory = &self.teardown.state_dir;
        std::fs::create_dir_all(directory)?;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        let temporary = directory.join(format!("pending-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
            std::fs::rename(&temporary, directory.join("pending.json"))?;
            std::fs::File::open(directory)?.sync_all()
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }

    fn valid_at(&self, directory: &Path, instance: &str) -> bool {
        let mut delete = self.base.clone();
        delete.extend([
            "sandbox".into(),
            "delete".into(),
            self.teardown.sandbox.clone(),
        ]);
        let download_valid = self.teardown.download.as_ref().is_none_or(|argv| {
            argv.len() == self.base.len() + 5
                && argv.starts_with(&self.base)
                && argv[self.base.len()..self.base.len() + 3]
                    == ["sandbox", "download", self.teardown.sandbox.as_str()]
                && argv
                    .last()
                    .is_some_and(|path| Path::new(path) == self.teardown.download_dir)
        });
        // `#274`: the preserve-download argv, when a claude-code run staged
        // one, names exactly the incoming directory this task's run would
        // use -- never a path a tampered record could redirect.
        let preserve_valid = self.teardown.preserve_download.as_ref().is_none_or(|argv| {
            safe_run_id(&self.task)
                && self.teardown.task == self.task
                && argv.len() == self.base.len() + 5
                && argv.starts_with(&self.base)
                && argv[self.base.len()..self.base.len() + 4]
                    == [
                        "sandbox",
                        "download",
                        self.teardown.sandbox.as_str(),
                        crate::openshell::SANDBOX_CLAUDE_PROJECTS,
                    ]
                && argv.last().is_some_and(|path| {
                    Path::new(path)
                        == incoming_projects_dir(&sessions_root_of(directory), &self.task)
                })
        });
        self.instance == instance
            && safe_run_id(&self.run)
            && directory
                .file_name()
                .is_some_and(|name| name == self.run.as_str())
            && self.teardown.state_dir == directory
            && self.teardown.download_dir == directory.join("download")
            && self.teardown.sandbox
                == format!("factory-{}", self.run.chars().take(8).collect::<String>())
            && !self.base.is_empty()
            && !self.base[0].is_empty()
            && self.teardown.delete == delete
            && download_valid
            && preserve_valid
            && self
                .teardown
                .service_evidence
                .as_ref()
                .is_none_or(|context| {
                    context.valid(&self.teardown)
                        && context.instance == self.instance
                        && context.run == self.run
                        && context.task == self.task
                        && context.base == self.base
                })
    }
}

fn safe_run_id(run: &str) -> bool {
    !run.is_empty() && run.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Only immediate, non-symlink run directories and bounded, non-symlink
/// records are read. Bad or foreign records never become cleanup commands.
pub fn pending(root: &Path, instance: &str) -> std::io::Result<Vec<Pending>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let directory = entry.path();
        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(directory.join("pending.json"))
        else {
            continue;
        };
        if !file.metadata()?.is_file() || file.metadata()?.len() > 64 * 1024 {
            continue;
        }
        let mut bytes = Vec::new();
        if file.take(64 * 1024 + 1).read_to_end(&mut bytes).is_err() || bytes.len() > 64 * 1024 {
            continue;
        }
        if let Ok(record) = serde_json::from_slice::<Pending>(&bytes) {
            if record.valid_at(&directory, instance) {
                records.push(record);
            }
        }
    }
    Ok(records)
}

/// The `openshell` CLI: the block's `cli`, else the first on PATH, else
/// Homebrew's or `/usr/local`'s. A daemon started by launchd has a bare
/// PATH, which is why the last two are looked at at all.
pub fn resolve_cli(configured: Option<&str>) -> Result<String> {
    if let Some(cli) = configured {
        let path = Path::new(cli);
        if path.is_absolute() && !path.is_file() {
            return Err(FactoryError::BadRequest(format!(
                "openshell.cli names {cli}, which does not exist; install OpenShell or correct the path"
            )));
        }
        return Ok(cli.to_string());
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("openshell"))
                .collect()
        })
        .unwrap_or_default();
    candidates.push(PathBuf::from("/opt/homebrew/bin/openshell"));
    candidates.push(PathBuf::from("/usr/local/bin/openshell"));
    candidates
        .into_iter()
        .find(|c| c.is_file())
        .map(|c| c.display().to_string())
        .ok_or_else(|| {
            FactoryError::BadRequest(
                "sandbox: openshell, but no openshell CLI was found on PATH, in /opt/homebrew/bin or in /usr/local/bin; \
                 install OpenShell (see the README's Sandboxes section) or set openshell.cli. \
                 The run was not started on the host instead"
                    .into(),
            )
        })
}

/// One CLI call, with a deadline and nothing on stdin, answering stdout or
/// a reason built from stderr. `what` says what the call was for, so the
/// reason a run fails with reads as a sentence.
async fn run(
    argv: &[String],
    deadline: Duration,
    what: &str,
) -> std::result::Result<String, String> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| format!("{what}: nothing to run"))?;
    let child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .env("NO_COLOR", "1")
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(deadline, child).await {
        Err(_) => {
            return Err(format!(
                "{what}: `{}` did not finish within {}s",
                short(argv),
                deadline.as_secs()
            ))
        }
        Ok(Err(e)) => return Err(format!("{what}: could not run {program}: {e}")),
        Ok(Ok(output)) => output,
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = clean(&stderr);
    let reason = if reason.is_empty() {
        clean(&stdout)
    } else {
        reason
    };
    Err(format!(
        "{what}: {} ({})",
        if reason.is_empty() {
            "no output".into()
        } else {
            reason
        },
        output.status
    ))
}

/// OpenShell's error text, without the box drawing and spinner lines, on one
/// line short enough to be a run's failure reason.
fn clean(text: &str) -> String {
    let joined = text
        .lines()
        .map(|l| {
            l.trim_start_matches(|c: char| {
                c.is_whitespace() || matches!(c, '×' | '│' | '╰' | '─' | '╭' | '|')
            })
            .trim()
        })
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("Provisioning sandbox")
                && !l.starts_with("Uploading")
                && !l.starts_with("Downloading")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut out: String = joined.chars().take(600).collect();
    if joined.chars().count() > 600 {
        out.push('…');
    }
    out
}

fn short(argv: &[String]) -> String {
    argv.iter().take(4).cloned().collect::<Vec<_>>().join(" ")
}

/// Before anything is created: the CLI runs, the gateway is connected, and
/// every provider the block names exists. Each failure says which.
pub async fn preflight(base: &[String], providers: &[String]) -> std::result::Result<(), String> {
    let mut version = base[..1].to_vec();
    version.push("--version".into());
    run(&version, QUICK, "the openshell CLI does not run").await?;

    let mut status = base.to_vec();
    status.extend(["status", "-o", "json"].map(String::from));
    let answer = run(&status, QUICK, "the OpenShell gateway is not reachable").await?;
    let parsed: serde_json::Value = serde_json::from_str(answer.trim()).map_err(|_| {
        format!(
            "the OpenShell gateway is not reachable: `openshell status` answered {:?}",
            clean(&answer)
        )
    })?;
    let state = parsed
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("unknown");
    if !state.eq_ignore_ascii_case("connected") {
        return Err(format!(
            "the OpenShell gateway {} is {state}, not connected; start it (brew services restart openshell) and retry",
            parsed.get("gateway").and_then(|g| g.as_str()).unwrap_or("(active)")
        ));
    }

    for provider in providers {
        let mut get = base.to_vec();
        get.extend(["provider".to_string(), "get".to_string(), provider.clone()]);
        run(&get, QUICK, &format!("the OpenShell provider {provider:?} is missing; create it on the host with `openshell provider create --name {provider} ...`"))
            .await?;
    }
    Ok(())
}

/// Write the plan's files: the policy, the staged run files (the env file
/// owner-only), and the pane's launcher.
pub fn write_files(plan: &Plan) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = plan.policy_path.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::write(&plan.policy_path, &plan.policy)?;
    std::fs::create_dir_all(&plan.stage_dir)?;
    for (path, contents, mode) in &plan.stage_files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Created with its mode, so the env file is never readable by
        // anyone else even for the moment between write and chmod.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, *mode);
        let mut file = options.open(path)?;
        std::io::Write::write_all(&mut file, contents.as_bytes())?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(*mode))?;
    }
    std::fs::write(&plan.host_launcher, &plan.host_launcher_script)?;
    Ok(())
}

/// Make the run's sandbox: preflight, write the files, create, restore a
/// tentative resume (`#274`), upload. On any failure past `create`, the
/// sandbox is deleted before the reason is returned, so a failed dispatch
/// leaves nothing running.
pub async fn prepare(plan: &Plan, providers: &[String], restore: Option<&Restore>) -> Result<RestoreOutcome> {
    let fail = |reason: String| {
        FactoryError::BadRequest(format!(
            "{reason}. The run was not started on the host instead"
        ))
    };
    preflight(&plan.base, providers).await.map_err(fail)?;
    write_files(plan).map_err(|e| {
        fail(format!(
            "writing the sandbox's files under {}: {e}",
            plan.stage_dir.display()
        ))
    })?;

    if let Err(reason) = run(
        &plan.create,
        CREATE,
        &format!("OpenShell could not create sandbox {}", plan.sandbox),
    )
    .await
    {
        // `create` can fail after the gateway accepted the sandbox -- a
        // policy it would not activate leaves one in `Error` -- so delete
        // by name either way. Deleting one that does not exist is a no-op.
        discard(plan).await;
        return Err(fail(reason));
    }

    // `#274`: attempted before the run's own files are uploaded, so a
    // failed restore can still correct the host copy of `launch.sh` and
    // `prompt.md` -- which `write_files` staged as the plan's own, fresh
    // versions -- before either ever leaves the host. Never attempted
    // after: by then the sandbox would already be running on whichever
    // version it got, and re-uploading over a session already launched
    // into is not a thing to try.
    let outcome = match restore {
        None => RestoreOutcome::NotAttempted,
        Some(restore) => {
            let mut upload = plan.base.clone();
            upload.extend([
                "sandbox".to_string(),
                "upload".to_string(),
                plan.sandbox.clone(),
                restore.local_dir.join("projects").display().to_string(),
                crate::openshell::SANDBOX_CLAUDE_DIR.to_string(),
                "--no-git-ignore".to_string(),
            ]);
            match run(
                &upload,
                TRANSFER,
                &format!("restoring the preserved conversation into sandbox {}", plan.sandbox),
            )
            .await
            {
                Err(reason) => RestoreOutcome::FellBack(reason),
                Ok(_) => match stage_resume_files(&restore.override_files) {
                    Ok(()) => RestoreOutcome::Restored { local_dir: restore.local_dir.clone() },
                    Err(e) => RestoreOutcome::FellBack(format!(
                        "the preserved conversation uploaded, but its launcher could not be staged: {e}"
                    )),
                },
            }
        }
    };

    for upload in &plan.uploads {
        if let Err(reason) = run(
            upload,
            TRANSFER,
            &format!("uploading into sandbox {}", plan.sandbox),
        )
        .await
        {
            discard(plan).await;
            return Err(fail(reason));
        }
    }
    // The env file carries the run token. It is inside the sandbox now; the
    // host copy has done its job.
    let _ = std::fs::remove_file(plan.stage_dir.join("env"));
    Ok(outcome)
}

/// Overwrite the plan's own (fresh) `launch.sh`/`prompt.md` with the
/// resumed versions, on the host, before the stage directory that holds
/// them is uploaded.
fn stage_resume_files(files: &[(PathBuf, String, u32)]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for (path, contents, mode) in files {
        std::fs::write(path, contents)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(*mode))?;
    }
    Ok(())
}

/// Discard a sandbox `prepare` made for a run whose session then never came
/// up: delete it and its files. Best effort; reconcile catches the rest.
pub async fn discard(plan: &Plan) {
    let mut get = plan.base.clone();
    get.extend(["sandbox", "get", &plan.sandbox, "-o", "json"].map(String::from));
    // A failed create may mean that this name already belongs to another
    // run or instance. Never turn that failure into deletion of its sandbox.
    let mut retired = false;
    if let Ok(raw) = run(&get, QUICK, "checking sandbox ownership").await {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
            if plan_owns(plan, &value) {
                retired = run(&plan.delete, QUICK, "deleting").await.is_ok();
            } else {
                retired = true;
            }
        }
    }
    // Never retain the callback credential, even when an unreachable
    // gateway requires the non-secret cleanup record to survive restart.
    let _ = std::fs::remove_file(plan.stage_dir.join("env"));
    if retired {
        if let Some(dir) = plan.policy_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn plan_owns(plan: &Plan, sandbox: &serde_json::Value) -> bool {
    [LABEL_INSTANCE, LABEL_RUN].into_iter().all(|key| {
        let expected = plan
            .create
            .windows(2)
            .filter(|pair| pair[0] == "--label")
            .filter_map(|pair| pair[1].split_once('='))
            .find_map(|(name, value)| (name == key).then_some(value));
        expected.is_some()
            && sandbox
                .get("labels")
                .and_then(|labels| labels.get(key))
                .and_then(|value| value.as_str())
                == expected
    })
}

/// One teardown per sandbox at a time: a run can be closed twice (a report
/// racing a cancel, the watchdog racing either), and the second teardown
/// would only find the first one's sandbox half gone.
pub struct Claim(String);

static CLAIMED: std::sync::Mutex<std::collections::BTreeSet<String>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

impl Claim {
    pub fn take(sandbox: &str) -> Option<Self> {
        let mut claimed = CLAIMED.lock().unwrap_or_else(|p| p.into_inner());
        claimed
            .insert(sandbox.to_string())
            .then(|| Self(sandbox.to_string()))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        CLAIMED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.0);
    }
}

/// The end of a run: download, fast-forward, delete, forget. Every step is
/// attempted whatever the one before it did, and each says what happened,
/// for the run's journal.
pub async fn finish(teardown: &Teardown) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(context) = &teardown.service_evidence {
        let capture = crate::service_observations::collect_final(context, teardown).await;
        let count = capture.accesses.len();
        let context = context.clone();
        let evidence_teardown = teardown.clone();
        let saved = tokio::task::spawn_blocking(move || {
            crate::service_observations::save(&context, &evidence_teardown, &capture)
        })
        .await;
        match saved {
            Ok(Ok(())) => notes.push(format!(
                "preserved {count} sandbox service observations (partial enforcement log)"
            )),
            _ => notes.push("sandbox service evidence could not be preserved".into()),
        }
    }
    if let Some(download) = &teardown.preserve_download {
        notes.push(preserve(teardown, download).await);
    }
    let mut retain = false;
    let mut restore_failed = false;
    if let Some(download) = &teardown.download {
        let _ = std::fs::remove_dir_all(&teardown.download_dir);
        match std::fs::create_dir_all(&teardown.download_dir) {
            Err(e) => {
                restore_failed = true;
                notes.push(format!("not downloaded: could not make {}: {e}", teardown.download_dir.display()));
            }
            Ok(()) => match run(download, TRANSFER, "downloading the working tree").await {
                Err(reason) => {
                    restore_failed = true;
                    notes.push(format!("not downloaded: {reason}"));
                }
                Ok(_) => match copy_tree(&teardown.download_dir, &teardown.cwd) {
                    Ok(n) => notes.push(format!("downloaded the working tree into {} ({n} files; .git is never copied back)", teardown.cwd.display())),
                    Err(e) => {
                        restore_failed = true;
                        notes.push(format!("downloaded, but copying into {} failed: {e}", teardown.cwd.display()));
                    }
                },
            },
        }
    }
    if teardown.fast_forward {
        notes.push(fast_forward(&teardown.cwd).await);
    }
    match run(
        &teardown.delete,
        QUICK,
        &format!("deleting sandbox {}", teardown.sandbox),
    )
    .await
    {
        Ok(_) => notes.push(format!("deleted sandbox {}", teardown.sandbox)),
        Err(reason) => {
            retain = true;
            notes.push(format!(
                "{reason}; it will be retried when the daemon next starts"
            ));
        }
    }
    if restore_failed {
        retain = true;
        let _ = std::fs::write(
            teardown.state_dir.join("recovery"),
            "Download restoration failed. Preserve this run's staged output for recovery.\n",
        );
    }
    if retain {
        notes.push(format!(
            "run state retained at {} for recovery",
            teardown.state_dir.display()
        ));
    } else {
        let _ = std::fs::remove_dir_all(&teardown.state_dir);
    }
    notes
}

/// What a teardown's preserve step left behind for the task, once captured
/// -- `#274`. `record.json`, beside `projects/` (absent when the capture
/// was over the cap), inside `preserved_dir`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreservedRecord {
    pub task: String,
    pub run: String,
    /// The one session this preserved tree holds, when exactly one
    /// `projects/<dir>/<id>.jsonl` was found at the expected depth. `None`
    /// when none or more than one was -- resuming needs it unambiguous.
    pub session_id: Option<String>,
    /// The sandbox working directory the download was taken from, matched
    /// against the next dispatch's own before it is trusted.
    pub workdir: String,
    pub bytes: u64,
}

/// Capture a claude-code sandbox's conversation before its sandbox is
/// deleted, keeping it per task so the task's next run can resume it
/// (`#274`). One note, win or lose, for the run's journal -- the deletion
/// of the sandbox itself is `finish`'s to report, not this.
async fn preserve(teardown: &Teardown, download: &[String]) -> String {
    if teardown.task.is_empty() {
        return "sandbox conversation not preserved: no task id was recorded for this run".into();
    }
    let sessions_root = sessions_root_of(&teardown.state_dir);
    let incoming = incoming_dir(&sessions_root, &teardown.task);
    let incoming_projects = incoming_projects_dir(&sessions_root, &teardown.task);
    let _ = std::fs::remove_dir_all(&incoming);
    if let Err(e) = std::fs::create_dir_all(&incoming_projects) {
        return format!("sandbox conversation not preserved: could not make {}: {e}", incoming.display());
    }
    if let Err(e) = lock_down(&incoming) {
        let _ = std::fs::remove_dir_all(&incoming);
        return format!("sandbox conversation not preserved: {e}");
    }
    if let Err(reason) = run(download, TRANSFER, "preserving the sandboxed conversation").await {
        let _ = std::fs::remove_dir_all(&incoming);
        return format!("sandbox conversation not preserved: {reason}");
    }
    // OpenShell's own download writes with whatever modes it chooses;
    // never trust them for what the rest of this code treats as an
    // owner-only record (`Pending::save`'s own rule).
    if let Err(e) = lock_down_tree(&incoming_projects) {
        let _ = std::fs::remove_dir_all(&incoming);
        return format!("sandbox conversation not preserved: {e}");
    }
    let bytes = match tree_bytes(&incoming_projects) {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&incoming);
            return format!("sandbox conversation not preserved: {e}");
        }
    };
    let over_cap = bytes > PRESERVE_CAP_BYTES;
    let session_id = if over_cap { None } else { sole_session_id(&incoming_projects) };
    if over_cap {
        // The point is a cheap resume, not an unbounded transcript store --
        // keep the size on record (so the next dispatch's reason is exact)
        // but never the content itself.
        let _ = std::fs::remove_dir_all(&incoming_projects);
    }
    let run_id = teardown
        .state_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let record = PreservedRecord {
        task: teardown.task.clone(),
        run: run_id,
        session_id,
        workdir: teardown.workdir.clone(),
        bytes,
    };
    if let Err(e) = write_record(&incoming, &record) {
        let _ = std::fs::remove_dir_all(&incoming);
        return format!("sandbox conversation not preserved: {e}");
    }
    let preserved = preserved_dir(&sessions_root, &teardown.task);
    let _ = std::fs::remove_dir_all(&preserved);
    if let Err(e) = std::fs::rename(&incoming, &preserved) {
        let _ = std::fs::remove_dir_all(&incoming);
        return format!("sandbox conversation not preserved: {e}");
    }
    if over_cap {
        format!(
            "the sandboxed conversation was {bytes} bytes, over the {PRESERVE_CAP_BYTES}-byte cap; not kept for resume"
        )
    } else {
        format!("preserved the sandboxed conversation ({bytes} bytes) for this task's next run to resume")
    }
}

fn lock_down(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Every directory 0o700, every file 0o600 -- owner-only, the same as
/// `Pending::save`, regardless of what the download wrote them as.
fn lock_down_tree(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            lock_down_tree(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        } else if entry.file_type()?.is_file() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

fn tree_bytes(dir: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total += if metadata.is_dir() { tree_bytes(&entry.path())? } else { metadata.len() };
    }
    Ok(total)
}

/// The session id a preserved tree holds, when it is unambiguous: exactly
/// one `<projects>/<cwd-dir>/<id>.jsonl` at this exact depth. Claude Code
/// nests subagent transcripts deeper still, under a session's own
/// directory, which this never descends into -- so a run with subagent
/// activity correctly reads as ambiguous rather than picking the wrong one.
fn sole_session_id(projects: &Path) -> Option<String> {
    let mut found = None;
    for cwd_dir in std::fs::read_dir(projects).ok()?.flatten() {
        if !cwd_dir.file_type().ok()?.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(cwd_dir.path()).ok()?.flatten() {
            if !entry.file_type().ok()?.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let id = path.file_stem()?.to_str()?.to_string();
            if found.is_some() {
                return None;
            }
            found = Some(id);
        }
    }
    found
}

fn write_record(incoming: &Path, record: &PreservedRecord) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(incoming.join("record.json"))?;
    file.write_all(&serde_json::to_vec(record)?)
}

/// This task's preserved conversation, when there is one -- bounded and
/// symlink-refused the same way `pending()` reads `pending.json`.
pub fn load_preserved(sessions_root: &Path, task: &str) -> Option<PreservedRecord> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let path = preserved_dir(sessions_root, task).join("record.json");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return None;
    }
    let mut bytes = Vec::new();
    if file.take(64 * 1024 + 1).read_to_end(&mut bytes).is_err() || bytes.len() > 64 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// Both a settled preserved conversation and an interrupted capture --
/// `#274`: a task closed or deleted keeps neither.
pub fn remove_preserved(sessions_root: &Path, task: &str) {
    let _ = std::fs::remove_dir_all(preserved_dir(sessions_root, task));
    let _ = std::fs::remove_dir_all(incoming_dir(sessions_root, task));
}

/// What `prepare` should try to resume into the new sandbox: the preserved
/// conversation `resolve_continue` already matched against this dispatch,
/// and the exact `launch.sh`/`prompt.md` content that resumes it. `prepare`
/// stages the plan's own (fresh) files first and only switches to these
/// once the upload actually lands -- fail-closed by construction, rather
/// than staging the resumed files and trying to undo that on a late
/// failure.
#[derive(Debug, Clone)]
pub struct Restore {
    /// `preserved_dir` for this task: holds `projects/`.
    pub local_dir: PathBuf,
    pub override_files: Vec<(PathBuf, String, u32)>,
}

/// What happened to a tentative resume, once `prepare` returns.
#[derive(Debug)]
pub enum RestoreOutcome {
    /// Nothing was attempted: a fresh dispatch, or a non-sandboxed one.
    NotAttempted,
    /// The preserved conversation is uploaded and staged as the run's
    /// `launch.sh`. The caller deletes `local_dir` only once the run's
    /// session has actually started -- an earlier delete would lose the
    /// only copy to a later failure this function cannot see.
    Restored { local_dir: PathBuf },
    /// The upload (or staging the resumed files) failed; `prepare` left the
    /// plan's fresh files in place and `local_dir` untouched for a later
    /// run to try again.
    FellBack(String),
}

/// Copy `from` into `to`, overwriting what is there and deleting nothing,
/// never `.git`: git's own state comes back through the remote. Destination
/// traversal is relative to open directory handles with O_NOFOLLOW, so a
/// downloaded path cannot follow a host symlink outside the working tree.
/// Existing multiply-linked files and special files are refused as well.
/// Source symlinks are recreated as links, never followed.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<usize> {
    use std::ffi::{CString, OsStr};
    use std::fs::{File, OpenOptions};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    fn name(value: &OsStr) -> std::io::Result<CString> {
        CString::new(value.as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    }

    fn open_at(parent: &File, child: &CString, flags: i32) -> std::io::Result<File> {
        // SAFETY: parent owns a live directory descriptor, child is a live
        // NUL-terminated single entry name, and ownership of a successful
        // descriptor is transferred to File exactly once.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                child.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o644,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn walk(from: &Path, to: &File, count: &mut usize) -> std::io::Result<()> {
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let entry_name = entry.file_name();
            if entry_name == ".git" {
                continue;
            }
            let child = name(&entry_name)?;
            let source = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                // SAFETY: the owned descriptor and CString stay live;
                // mkdirat can create only this immediate child entry.
                let made = unsafe { libc::mkdirat(to.as_raw_fd(), child.as_ptr(), 0o755) };
                if made < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(error);
                    }
                }
                let directory = open_at(to, &child, libc::O_RDONLY | libc::O_DIRECTORY)?;
                walk(&source, &directory, count)?;
            } else if kind.is_symlink() {
                let link = name(std::fs::read_link(&source)?.as_os_str())?;
                // SAFETY: both names and the directory descriptor are
                // valid; unlinkat removes the entry, never its referent.
                let removed = unsafe { libc::unlinkat(to.as_raw_fd(), child.as_ptr(), 0) };
                if removed < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::NotFound {
                        return Err(error);
                    }
                }
                let linked =
                    unsafe { libc::symlinkat(link.as_ptr(), to.as_raw_fd(), child.as_ptr()) };
                if linked < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                *count += 1;
            } else if kind.is_file() {
                let mut target = open_at(
                    to,
                    &child,
                    libc::O_WRONLY | libc::O_CREAT | libc::O_NONBLOCK | libc::O_NOCTTY,
                )?;
                let metadata = target.metadata()?;
                if !metadata.is_file() || metadata.nlink() != 1 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "download destination is not an unshared regular file",
                    ));
                }
                let mut input = File::open(&source)?;
                target.set_len(0)?;
                std::io::copy(&mut input, &mut target)?;
                target.set_permissions(std::fs::Permissions::from_mode(
                    input.metadata()?.permissions().mode() & 0o777,
                ))?;
                *count += 1;
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "download contains a special file",
                ));
            }
        }
        Ok(())
    }
    std::fs::create_dir_all(to)?;
    // Resolve the configured working directory once; all entries beneath
    // this root are then opened relative to its descriptor, not by path.
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(to.canonicalize()?)?;
    let mut count = 0;
    walk(from, &root, &mut count)?;
    Ok(count)
}

/// `git fetch` then `git merge --ff-only @{u}` in the run's host checkout --
/// how a run whose work landed by being pushed and merged leaves the host
/// level with what it published. Never forced: a checkout that cannot be
/// fast-forwarded is left exactly as it was, and the reason is the note.
async fn fast_forward(cwd: &Path) -> String {
    let git = |args: &[&str]| {
        let mut argv = vec![
            "git".to_string(),
            "-C".to_string(),
            cwd.display().to_string(),
        ];
        argv.extend(args.iter().map(|a| a.to_string()));
        argv
    };
    if let Err(reason) = run(
        &git(&["fetch", "--quiet"]),
        TRANSFER,
        "not fast-forwarded: git fetch failed",
    )
    .await
    {
        return reason;
    }
    match run(
        &git(&["merge", "--ff-only", "@{u}"]),
        QUICK,
        "not fast-forwarded",
    )
    .await
    {
        Ok(out) => {
            let head = run(
                &git(&["rev-parse", "--short", "HEAD"]),
                QUICK,
                "reading HEAD",
            )
            .await
            .unwrap_or_default();
            if out.contains("Already up to date") {
                format!(
                    "{} is already level with its upstream ({})",
                    cwd.display(),
                    head.trim()
                )
            } else {
                format!(
                    "fast-forwarded {} to its upstream ({})",
                    cwd.display(),
                    head.trim()
                )
            }
        }
        Err(reason) => reason,
    }
}

/// Every sandbox this instance made, as `(name, run id)`, from the labels
/// `plan` put on them.
pub async fn list_ours(
    base: &[String],
    instance_id: &str,
) -> std::result::Result<Vec<(String, String)>, String> {
    let mut argv = base.to_vec();
    argv.extend([
        "sandbox".to_string(),
        "list".to_string(),
        "--selector".to_string(),
        format!("{LABEL_INSTANCE}={instance_id}"),
        "-o".to_string(),
        "json".to_string(),
    ]);
    let out = run(&argv, QUICK, "listing sandboxes").await?;
    let parsed: serde_json::Value =
        serde_json::from_str(out.trim()).map_err(|e| format!("listing sandboxes: {e}"))?;
    Ok(parsed
        .get("sandboxes")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let name = s.get("name")?.as_str()?.to_string();
            let labels = s.get("labels")?;
            if labels.get(LABEL_INSTANCE)?.as_str()? != instance_id {
                return None;
            }
            let run = labels.get(LABEL_RUN)?.as_str()?.to_string();
            if !safe_run_id(&run)
                || !name.starts_with("factory-")
                || !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                return None;
            }
            Some((name, run))
        })
        .collect())
}

pub async fn delete(base: &[String], name: &str) -> std::result::Result<(), String> {
    let mut argv = base.to_vec();
    argv.extend([
        "sandbox".to_string(),
        "delete".to_string(),
        name.to_string(),
    ]);
    run(&argv, QUICK, &format!("deleting sandbox {name}"))
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openshell::{plan, CallbackTarget, OpenshellConfig, PlanInput};
    use factory_kernel::{LaunchKind, LaunchSpec};
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> Dir {
        let d = std::env::temp_dir().join(format!("factory-openshell-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    /// A stand-in `openshell` that logs every argv to `calls` and answers
    /// per subcommand from the environment it was written with.
    fn fake_cli(
        dir: &Path,
        gateway: &str,
        missing_provider: Option<&str>,
        create_fails: bool,
    ) -> PathBuf {
        let path = dir.join("openshell");
        let calls = dir.join("calls");
        let script = format!(
            "#!/bin/sh\n\
             echo \"$*\" >> '{calls}'\n\
             case \"$1 $2\" in\n\
             '--version ') echo 'openshell 0.1.2' ;;\n\
             'status -o') echo '{{\"gateway\":\"g\",\"status\":\"{gateway}\"}}' ;;\n\
             'provider get') if [ \"$3\" = '{missing}' ]; then echo '  × provider not found' >&2; exit 1; fi ;;\n\
             'sandbox create') if [ {create_fails} = 1 ]; then printf 'Provisioning sandbox...\\n  × sandbox entered error phase: ConfigurationInvalid\\n' >&2; exit 1; fi; echo '{{\"phase\":\"Ready\"}}' ;;\n\
             'sandbox get') echo '{{\"labels\":{{\"factory.instance\":\"inst\",\"factory.run\":\"aaaabbbb-run\"}}}}' ;;\n\
             'sandbox download') if [ \"$4\" = '/sandbox/.claude/projects' ]; then mkdir -p \"$5/cwd-dir\" && echo '{{}}' > \"$5/cwd-dir/session-abc.jsonl\"; else mkdir -p \"$5\" && echo downloaded > \"$5/result.txt\" && mkdir -p \"$5/.git\" && echo x > \"$5/.git/HEAD\"; fi ;;\n\
             'sandbox list') echo '{{\"sandboxes\":[{{\"name\":\"factory-aaaa\",\"labels\":{{\"factory.instance\":\"inst-1\",\"factory.run\":\"run-a\"}}}},{{\"name\":\"factory-bbbb\",\"labels\":{{\"factory.instance\":\"inst-1\",\"factory.run\":\"run-b\"}}}}]}}' ;;\n\
             esac\n",
            calls = calls.display(),
            missing = missing_provider.unwrap_or("-"),
            create_fails = if create_fails { 1 } else { 0 },
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn a_plan(dir: &Path, cli: &Path, download: bool) -> (Plan, PathBuf) {
        let cwd = dir.join("scope");
        let guides = dir.join("guides");
        std::fs::create_dir_all(cwd.join(".git")).unwrap();
        std::fs::create_dir_all(&guides).unwrap();
        std::fs::write(guides.join("run-t1.md"), "guide").unwrap();
        let mut config: OpenshellConfig = serde_yaml_ng::from_str(
            "image: img\nproviders: [factory-claude, factory-github]\npolicy:\n  network_policies: {}\n",
        )
        .unwrap();
        if download {
            config.download = crate::openshell::Transfer::Workdir;
        }
        let launch = LaunchSpec {
            kind: LaunchKind::Named("claude".into()),
            args: vec![
                "--append-system-prompt-file".into(),
                guides.join("run-t1.md").display().to_string(),
            ],
            env: BTreeMap::from([("FACTORY_TOKEN".to_string(), "tok".to_string())]),
            agent_kind: None,
        };
        let target = CallbackTarget {
            host: "host.openshell.internal".into(),
            port: 8787,
        };
        let state = dir.join("state/run-1");
        let providers: Vec<String> = config
            .providers
            .iter()
            .map(|p| p.gateway_name("inst"))
            .collect();
        let plan = plan(&PlanInput {
            config: &config,
            cli: &cli.display().to_string(),
            image: config.image.as_deref().unwrap_or_default(),
            providers: &providers,
            instance_id: "inst",
            run_id: "aaaabbbb-run",
            task_id: "t1",
            cwd: &cwd,
            guides_dir: &guides,
            state_dir: &state,
            launch: &launch,
            prompt: "go",
            callback: &target,
        })
        .unwrap();
        (plan, cwd)
    }

    /// Same shape as `a_plan`, but the shell agent -- which has no
    /// `Agent::resume_spec` and so nothing `#274` preserves.
    fn a_shell_plan(dir: &Path, cli: &Path) -> (Plan, PathBuf) {
        let cwd = dir.join("scope");
        let guides = dir.join("guides");
        std::fs::create_dir_all(cwd.join(".git")).unwrap();
        std::fs::create_dir_all(&guides).unwrap();
        let config: OpenshellConfig = serde_yaml_ng::from_str(
            "image: img\nproviders: [factory-claude]\npolicy:\n  network_policies: {}\n",
        )
        .unwrap();
        let launch = LaunchSpec {
            kind: LaunchKind::Command(Vec::new()),
            args: Vec::new(),
            env: BTreeMap::from([("FACTORY_TOKEN".to_string(), "tok".to_string())]),
            agent_kind: None,
        };
        let target = CallbackTarget { host: "host.openshell.internal".into(), port: 8787 };
        let state = dir.join("state/run-1");
        let providers: Vec<String> = config.providers.iter().map(|p| p.gateway_name("inst")).collect();
        let plan = plan(&PlanInput {
            config: &config,
            cli: &cli.display().to_string(),
            image: config.image.as_deref().unwrap_or_default(),
            providers: &providers,
            instance_id: "inst",
            run_id: "aaaabbbb-run",
            task_id: "t1",
            cwd: &cwd,
            guides_dir: &guides,
            state_dir: &state,
            launch: &launch,
            prompt: ". /sandbox/.factory-run/run.sh\n",
            callback: &target,
        })
        .unwrap();
        (plan, cwd)
    }

    #[tokio::test]
    async fn a_missing_cli_is_a_reason_and_never_a_fallback() {
        let e = resolve_cli(Some("/nonexistent/openshell"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("does not exist"), "{e}");
        let dir = tempdir();
        let (plan, _) = a_plan(&dir.0, &dir.0.join("absent-openshell"), false);
        let e = prepare(&plan, &["factory-claude".into()], None)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("the openshell CLI does not run") && e.contains("not started on the host"),
            "{e}"
        );
        assert!(
            !plan.host_launcher.exists(),
            "nothing a pane could run was written"
        );
    }

    #[tokio::test]
    async fn an_unreachable_gateway_fails_before_anything_is_created() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "disconnected", None, false);
        let (plan, _) = a_plan(&dir.0, &cli, false);
        let e = prepare(&plan, &[], None).await.unwrap_err().to_string();
        assert!(e.contains("disconnected, not connected"), "{e}");
        assert!(
            !calls(&dir.0).iter().any(|c| c.starts_with("sandbox")),
            "{:?}",
            calls(&dir.0)
        );
    }

    #[tokio::test]
    async fn a_missing_provider_is_named() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", Some("factory-github"), false);
        let (plan, _) = a_plan(&dir.0, &cli, false);
        let e = prepare(&plan, &["factory-claude".into(), "factory-github".into()], None)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("\"factory-github\" is missing"), "{e}");
        assert!(!calls(&dir.0)
            .iter()
            .any(|c| c.starts_with("sandbox create")));
    }

    #[tokio::test]
    async fn failed_dispatch_retains_cleanup_identity_without_retaining_the_token() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        let (plan, _) = a_plan(&dir.0, &cli, false);
        write_files(&plan).unwrap();
        let directory = plan.policy_path.parent().unwrap();
        std::fs::write(directory.join("pending.json"), "cleanup identity").unwrap();
        let script = std::fs::read_to_string(&cli)
            .unwrap()
            .replace("'sandbox get') echo", "'sandbox get') exit 3; echo");
        std::fs::write(&cli, script).unwrap();
        discard(&plan).await;
        assert!(directory.join("pending.json").exists());
        assert!(!plan.stage_dir.join("env").exists());
        assert!(!calls(&dir.0)
            .iter()
            .any(|call| call.starts_with("sandbox delete")));
    }

    #[test]
    fn cleanup_records_are_private_bounded_and_bound_to_the_instance_run_directory() {
        let dir = tempdir();
        let (plan, cwd) = a_plan(&dir.0, Path::new("openshell"), false);
        let root = dir.0.join("pending");
        let directory = root.join("aaaabbbb-run");
        let mut teardown = Teardown::of(&plan, &cwd, false, "t1");
        teardown.state_dir = directory.clone();
        teardown.download_dir = directory.join("download");
        if let Some(argv) = &mut teardown.preserve_download {
            *argv.last_mut().unwrap() = incoming_projects_dir(&sessions_root_of(&directory), "t1")
                .display()
                .to_string();
        }
        let record = Pending {
            instance: "inst".into(),
            run: "aaaabbbb-run".into(),
            task: "t1".into(),
            base: plan.base.clone(),
            teardown,
        };
        record.save().unwrap();
        assert_eq!(pending(&root, "inst").unwrap().len(), 1);
        assert!(pending(&root, "other").unwrap().is_empty());
        assert_eq!(
            std::fs::metadata(directory.join("pending.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for mutation in 0..7 {
            let mut invalid = record.clone();
            match mutation {
                0 => invalid.run = "../outside".into(),
                1 => invalid.teardown.state_dir = dir.0.clone(),
                2 => invalid.teardown.download_dir = dir.0.clone(),
                3 => invalid.teardown.delete.push("unexpected".into()),
                4 => invalid.teardown.sandbox = "factory-other".into(),
                5 => invalid.teardown.task = "../outside".into(),
                _ => {
                    if let Some(argv) = &mut invalid.teardown.preserve_download {
                        *argv.last_mut().unwrap() = dir.0.join("elsewhere").display().to_string();
                    }
                }
            }
            std::fs::write(
                directory.join("pending.json"),
                serde_json::to_vec(&invalid).unwrap(),
            )
            .unwrap();
            assert!(
                pending(&root, "inst").unwrap().is_empty(),
                "mutation {mutation}"
            );
        }
        std::fs::write(directory.join("pending.json"), vec![b' '; 65537]).unwrap();
        assert!(pending(&root, "inst").unwrap().is_empty());
        let outside = dir.0.join("outside.json");
        std::fs::write(&outside, serde_json::to_vec(&record).unwrap()).unwrap();
        std::fs::remove_file(directory.join("pending.json")).unwrap();
        std::os::unix::fs::symlink(&outside, directory.join("pending.json")).unwrap();
        assert!(
            pending(&root, "inst").unwrap().is_empty(),
            "a symlink is not a cleanup record"
        );
    }

    #[tokio::test]
    async fn a_sandbox_the_gateway_will_not_activate_is_deleted_and_the_reason_kept() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, true);
        let (plan, _) = a_plan(&dir.0, &cli, false);
        let e = prepare(&plan, &[], None).await.unwrap_err().to_string();
        assert!(
            e.contains("ConfigurationInvalid") && !e.contains("Provisioning sandbox"),
            "{e}"
        );
        let log = calls(&dir.0);
        assert!(
            log.last()
                .unwrap()
                .starts_with("sandbox delete factory-aaaabbbb"),
            "{log:?}"
        );
    }

    #[tokio::test]
    async fn a_good_sandbox_is_created_then_uploaded_and_the_host_env_file_is_gone() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        let (plan, _) = a_plan(&dir.0, &cli, false);
        prepare(&plan, &["factory-claude".into()], None).await.unwrap();
        let log = calls(&dir.0);
        let sandbox: Vec<&String> = log.iter().filter(|c| c.starts_with("sandbox")).collect();
        assert!(
            sandbox[0].starts_with("sandbox create --name factory-aaaabbbb --from img --policy"),
            "{sandbox:?}"
        );
        assert!(
            sandbox[1].starts_with("sandbox upload factory-aaaabbbb"),
            "{sandbox:?}"
        );
        assert_eq!(
            sandbox.len(),
            4,
            "create, the tree, its .git, the run files: {sandbox:?}"
        );
        assert!(
            !log.iter().any(|c| c.contains("tok")),
            "the token is on no command line: {log:?}"
        );
        assert!(
            !plan.stage_dir.join("env").exists(),
            "the env file does not outlive the upload on the host"
        );
        assert!(plan.host_launcher.is_file() && plan.policy_path.is_file());
        let mode = std::fs::metadata(plan.policy_path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o700,
            "the run's state directory is the owner's alone"
        );
    }

    #[tokio::test]
    async fn a_failed_create_never_deletes_a_sandbox_owned_by_another_instance() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, true);
        let script = std::fs::read_to_string(&cli).unwrap().replace(
            "\"factory.instance\":\"inst\"",
            "\"factory.instance\":\"other\"",
        );
        std::fs::write(&cli, script).unwrap();
        let (plan, _) = a_plan(&dir.0, &cli, false);
        assert!(prepare(&plan, &[], None).await.is_err());
        assert!(!calls(&dir.0)
            .iter()
            .any(|call| call.starts_with("sandbox delete")));
        assert!(
            !plan.stage_dir.join("env").exists(),
            "a failed dispatch does not retain a run token file"
        );
    }

    #[test]
    fn cleanup_requires_both_exact_instance_and_run_labels() {
        let dir = tempdir();
        let (plan, _) = a_plan(&dir.0, Path::new("openshell"), false);
        assert!(plan_owns(
            &plan,
            &serde_json::json!({"labels":{"factory.instance":"inst", "factory.run":"aaaabbbb-run"}})
        ));
        for labels in [
            serde_json::json!({}),
            serde_json::json!({"factory.instance":"inst"}),
            serde_json::json!({"factory.instance":"inst", "factory.run":"another-run"}),
        ] {
            assert!(!plan_owns(&plan, &serde_json::json!({"labels":labels})));
        }
    }

    #[tokio::test]
    async fn the_end_of_a_run_downloads_without_git_and_deletes_the_sandbox_and_its_files() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        let (plan, cwd) = a_plan(&dir.0, &cli, true);
        prepare(&plan, &[], None).await.unwrap();
        std::fs::write(cwd.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let teardown = Teardown::of(&plan, &cwd, false, "t1");
        let restored = Teardown::from_meta(&BTreeMap::from([(
            META_KEY.to_string(),
            teardown.to_meta(),
        )]))
        .unwrap();
        assert_eq!(
            restored, teardown,
            "the teardown survives the session's meta"
        );
        let notes = finish(&restored).await;
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with("downloaded the working tree")),
            "{notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|n| n == "deleted sandbox factory-aaaabbbb"),
            "{notes:?}"
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("result.txt")).unwrap(),
            "downloaded\n"
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join(".git/HEAD")).unwrap(),
            "ref: refs/heads/main\n",
            "the host repository's .git is never replaced"
        );
        assert!(
            !teardown.state_dir.exists(),
            "nothing of the run is left under .factory"
        );
        assert!(calls(&dir.0)
            .last()
            .unwrap()
            .starts_with("sandbox delete factory-aaaabbbb"));
    }

    #[tokio::test]
    async fn a_failed_delete_says_it_will_be_retried() {
        let dir = tempdir();
        let teardown = Teardown {
            sandbox: "factory-x".into(),
            state_dir: dir.0.join("state"),
            cwd: dir.0.clone(),
            download: None,
            download_dir: dir.0.join("dl"),
            fast_forward: false,
            service_evidence: None,
            task: String::new(),
            workdir: String::new(),
            preserve_download: None,
            delete: vec![
                "/bin/sh".into(),
                "-c".into(),
                "echo gateway gone >&2; exit 3".into(),
            ],
        };
        let notes = finish(&teardown).await;
        assert!(
            notes[0].contains("gateway gone") && notes[0].contains("retried"),
            "{notes:?}"
        );
    }

    #[tokio::test]
    async fn unsafe_download_restoration_retains_output_and_still_deletes_the_sandbox() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        let (plan, cwd) = a_plan(&dir.0, &cli, true);
        prepare(&plan, &[], None).await.unwrap();
        let outside = dir.0.join("host.txt");
        std::fs::write(&outside, "host data").unwrap();
        std::os::unix::fs::symlink(&outside, cwd.join("result.txt")).unwrap();
        let teardown = Teardown::of(&plan, &cwd, false, "t1");
        let notes = finish(&teardown).await;
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "host data");
        assert_eq!(
            std::fs::read_to_string(teardown.download_dir.join("result.txt")).unwrap(),
            "downloaded\n"
        );
        assert!(teardown.state_dir.join("recovery").is_file());
        assert!(notes
            .iter()
            .any(|note| note.contains("retained") && note.contains("recovery")));
        assert!(calls(&dir.0).last().unwrap().starts_with("sandbox delete"));
    }

    #[test]
    fn downloaded_output_cannot_follow_a_host_file_symlink_outside_the_workdir() {
        let dir = tempdir();
        let source = dir.0.join("download");
        let work = dir.0.join("work");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let outside = dir.0.join("outside.txt");
        std::fs::write(&outside, "host data").unwrap();
        std::fs::write(source.join("result.txt"), "sandbox output").unwrap();
        std::os::unix::fs::symlink(&outside, work.join("result.txt")).unwrap();
        assert!(copy_tree(&source, &work).is_err());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "host data");
    }

    #[test]
    fn downloaded_output_cannot_follow_a_host_directory_symlink_outside_the_workdir() {
        let dir = tempdir();
        let source = dir.0.join("download");
        let work = dir.0.join("work");
        let outside = dir.0.join("outside");
        std::fs::create_dir_all(source.join("generated")).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(source.join("generated/result.txt"), "sandbox output").unwrap();
        std::os::unix::fs::symlink(&outside, work.join("generated")).unwrap();
        assert!(copy_tree(&source, &work).is_err());
        assert!(!outside.join("result.txt").exists());
    }

    #[test]
    fn downloaded_output_refuses_host_hardlinks_and_preserves_nested_git_metadata() {
        let dir = tempdir();
        let source = dir.0.join("download");
        let work = dir.0.join("work");
        std::fs::create_dir_all(source.join("nested/.git")).unwrap();
        std::fs::create_dir_all(work.join("nested/.git")).unwrap();
        std::fs::write(source.join("nested/.git/HEAD"), "guest git data").unwrap();
        std::fs::write(work.join("nested/.git/HEAD"), "host git data").unwrap();
        copy_tree(&source, &work).unwrap();
        assert_eq!(
            std::fs::read_to_string(work.join("nested/.git/HEAD")).unwrap(),
            "host git data"
        );
        let outside = dir.0.join("outside.txt");
        std::fs::write(&outside, "host data").unwrap();
        std::fs::hard_link(&outside, work.join("result.txt")).unwrap();
        std::fs::write(source.join("result.txt"), "sandbox data").unwrap();
        assert!(copy_tree(&source, &work).is_err());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "host data");
    }

    #[tokio::test]
    async fn fast_forward_moves_a_clean_checkout_to_its_upstream_and_refuses_otherwise() {
        let dir = tempdir();
        let sh = |cwd: &Path, script: &str| {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{script}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        let origin = dir.0.join("origin.git");
        let work = dir.0.join("work");
        let host = dir.0.join("host");
        sh(&dir.0, &format!(
            "git init -q --bare -b main {o} && git clone -q {o} {w} 2>/dev/null && cd {w} && git -c user.email=a@b -c user.name=a commit -q --allow-empty -m one && git push -q origin HEAD:main && cd .. && git clone -q {o} {h}",
            o = origin.display(), w = work.display(), h = host.display()
        ));
        sh(&work, "git -c user.email=a@b -c user.name=a commit -q --allow-empty -m two && git push -q origin HEAD:main");
        let note = fast_forward(&host).await;
        assert!(note.starts_with("fast-forwarded"), "{note}");
        assert!(fast_forward(&host).await.contains("already level"));
        // Diverged: a local commit the upstream does not have.
        sh(
            &host,
            "git -c user.email=a@b -c user.name=a commit -q --allow-empty -m local",
        );
        sh(&work, "git -c user.email=a@b -c user.name=a commit -q --allow-empty -m three && git push -q origin HEAD:main");
        let note = fast_forward(&host).await;
        assert!(note.starts_with("not fast-forwarded"), "{note}");
    }

    #[tokio::test]
    async fn reconcile_lists_by_this_instances_label() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        let ours = list_ours(&[cli.display().to_string()], "inst-1")
            .await
            .unwrap();
        assert_eq!(
            ours,
            vec![
                ("factory-aaaa".into(), "run-a".into()),
                ("factory-bbbb".into(), "run-b".into())
            ]
        );
        assert_eq!(
            calls(&dir.0),
            vec!["sandbox list --selector factory.instance=inst-1 -o json"]
        );
    }

    #[tokio::test]
    async fn reconcile_refuses_foreign_missing_and_path_shaped_run_labels() {
        for label in ["", "../another-run", ".", "run/a"] {
            let dir = tempdir();
            let cli = fake_cli(&dir.0, "connected", None, false);
            let script = std::fs::read_to_string(&cli).unwrap().replace(
                "\"factory.run\":\"run-a\"",
                &format!("\"factory.run\":\"{label}\""),
            );
            std::fs::write(&cli, script).unwrap();
            let ours = list_ours(&[cli.display().to_string()], "inst-1")
                .await
                .unwrap();
            assert_eq!(ours, vec![("factory-bbbb".into(), "run-b".into())]);
            assert!(list_ours(&[cli.display().to_string()], "other-instance")
                .await
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn a_sandbox_is_torn_down_by_one_closer_at_a_time() {
        let first = Claim::take("factory-claim").expect("free");
        assert!(
            Claim::take("factory-claim").is_none(),
            "a second closer backs off"
        );
        drop(first);
        assert!(
            Claim::take("factory-claim").is_some(),
            "and the claim is released when done"
        );
    }

    #[test]
    fn openshells_boxed_errors_become_one_line() {
        let text = "Provisioning sandbox (structured output on stdout)...\nError:   × sandbox provisioning timed out after 300s. Last reported status:\n  │ ConfigurationInvalid: Effective configuration could not be activated;\n  │ replace the policy\n";
        assert_eq!(
            clean(text),
            "Error:   × sandbox provisioning timed out after 300s. Last reported status: ConfigurationInvalid: Effective configuration could not be activated; replace the policy"
        );
    }

    // -- `#274`: preserving a sandboxed conversation and resuming it -------

    #[tokio::test]
    async fn finish_preserves_a_claude_runs_conversation_under_its_task_and_still_deletes_the_sandbox() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        let (plan, cwd) = a_plan(&dir.0, &cli, false);
        prepare(&plan, &[], None).await.unwrap();
        let teardown = Teardown::of(&plan, &cwd, false, "t1");
        assert!(
            teardown.preserve_download.is_some(),
            "a claude-code plan stages a preserve download"
        );
        let sessions_root = sessions_root_of(&teardown.state_dir);
        let notes = finish(&teardown).await;
        assert!(
            notes.iter().any(|n| n.contains("preserved the sandboxed conversation")),
            "{notes:?}"
        );
        assert!(
            notes.iter().any(|n| n == "deleted sandbox factory-aaaabbbb"),
            "{notes:?}"
        );
        let record = load_preserved(&sessions_root, "t1").expect("a record was saved");
        assert_eq!(record.task, "t1");
        // `preserve` reads the run id off `state_dir`'s own basename --
        // true in production (`factory_dir/openshell/<run>`), and `a_plan`'s
        // fixture names its state directory `run-1` rather than the
        // `run_id` it otherwise uses.
        assert_eq!(record.run, "run-1");
        assert_eq!(record.session_id.as_deref(), Some("session-abc"));
        assert_eq!(record.workdir, plan.workdir);
        assert!(preserved_dir(&sessions_root, "t1")
            .join("projects/cwd-dir/session-abc.jsonl")
            .is_file());
        assert!(
            !incoming_dir(&sessions_root, "t1").exists(),
            "the incoming staging name is gone once promoted"
        );
    }

    #[tokio::test]
    async fn finish_preserves_nothing_for_the_shell_agent_which_has_no_resume_spec() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        let (plan, cwd) = a_shell_plan(&dir.0, &cli);
        prepare(&plan, &[], None).await.unwrap();
        let teardown = Teardown::of(&plan, &cwd, false, "t1");
        assert!(teardown.preserve_download.is_none());
        let notes = finish(&teardown).await;
        assert!(
            !notes.iter().any(|n| n.contains("preserved") || n.contains("conversation")),
            "{notes:?}"
        );
        let sessions_root = sessions_root_of(&teardown.state_dir);
        assert!(load_preserved(&sessions_root, "t1").is_none());
    }

    #[tokio::test]
    async fn a_run_that_ends_blocked_timeout_still_preserves_and_deletes() {
        // `finish` has no notion of why the run ended -- `close_session` and
        // `reconcile_openshell` call it the same way whatever the run's
        // terminal status, which is what makes "every teardown preserves"
        // true for done, failed and a blocked-timeout reclaim alike.
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        let (plan, cwd) = a_plan(&dir.0, &cli, false);
        prepare(&plan, &[], None).await.unwrap();
        let teardown = Teardown::of(&plan, &cwd, false, "t1");
        let sessions_root = sessions_root_of(&teardown.state_dir);
        let notes = finish(&teardown).await;
        assert!(notes.iter().any(|n| n.contains("preserved")));
        assert!(load_preserved(&sessions_root, "t1").is_some());
    }

    #[tokio::test]
    async fn an_oversized_capture_keeps_its_size_on_record_but_not_the_transcript() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        // `truncate` makes a sparse file of exactly this logical size
        // without writing real bytes, so the test stays fast.
        let script = std::fs::read_to_string(&cli).unwrap().replace(
            "mkdir -p \"$5/cwd-dir\" && echo '{}' > \"$5/cwd-dir/session-abc.jsonl\"",
            "mkdir -p \"$5/cwd-dir\" && truncate -s 67108865 \"$5/cwd-dir/session-abc.jsonl\"",
        );
        std::fs::write(&cli, script).unwrap();
        let (plan, cwd) = a_plan(&dir.0, &cli, false);
        prepare(&plan, &[], None).await.unwrap();
        let teardown = Teardown::of(&plan, &cwd, false, "t1");
        let sessions_root = sessions_root_of(&teardown.state_dir);
        let notes = finish(&teardown).await;
        assert!(
            notes.iter().any(|n| n.contains("over the") && n.contains("cap")),
            "{notes:?}"
        );
        let record = load_preserved(&sessions_root, "t1").expect("the size is still recorded");
        assert_eq!(record.bytes, 67108865);
        assert!(record.session_id.is_none(), "no content was kept to derive one from");
        assert!(
            !preserved_dir(&sessions_root, "t1").join("projects").exists(),
            "the oversized transcript itself is never kept"
        );
    }

    #[tokio::test]
    async fn remove_preserved_deletes_a_settled_record_and_an_interrupted_capture() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        let (plan, cwd) = a_plan(&dir.0, &cli, false);
        prepare(&plan, &[], None).await.unwrap();
        let teardown = Teardown::of(&plan, &cwd, false, "t1");
        let sessions_root = sessions_root_of(&teardown.state_dir);
        finish(&teardown).await;
        assert!(load_preserved(&sessions_root, "t1").is_some());
        // An interrupted capture: the fixed incoming name, with no record
        // promoted over it yet.
        std::fs::create_dir_all(incoming_dir(&sessions_root, "t1")).unwrap();
        remove_preserved(&sessions_root, "t1");
        assert!(load_preserved(&sessions_root, "t1").is_none());
        assert!(!preserved_dir(&sessions_root, "t1").exists());
        assert!(!incoming_dir(&sessions_root, "t1").exists());
    }

    #[tokio::test]
    async fn an_old_format_pending_record_with_no_preserve_field_still_loads() {
        let dir = tempdir();
        let (plan, cwd) = a_plan(&dir.0, Path::new("openshell"), false);
        let root = dir.0.join("pending");
        let directory = root.join("aaaabbbb-run");
        let teardown = Teardown::of(&plan, &cwd, false, "t1");
        // The shape `Pending::save` wrote before `#274`: no `task`,
        // `workdir` or `preserve_download` on the teardown at all.
        let old = serde_json::json!({
            "instance": "inst",
            "run": "aaaabbbb-run",
            "task": "t1",
            "base": plan.base,
            "teardown": {
                "sandbox": teardown.sandbox,
                "state_dir": directory,
                "cwd": cwd,
                "download_dir": directory.join("download"),
                "delete": plan
                    .base
                    .iter()
                    .cloned()
                    .chain(["sandbox".to_string(), "delete".to_string(), teardown.sandbox.clone()])
                    .collect::<Vec<_>>(),
            },
        });
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("pending.json"), serde_json::to_vec(&old).unwrap()).unwrap();
        let loaded = pending(&root, "inst").unwrap();
        assert_eq!(loaded.len(), 1, "an old-format record is still readable");
        assert_eq!(loaded[0].teardown.task, "", "new fields default rather than fail to parse");
        assert!(loaded[0].teardown.preserve_download.is_none());
    }

    #[tokio::test]
    async fn prepare_restores_a_preserved_conversation_before_the_runs_own_files_upload() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        let (plan, _cwd) = a_plan(&dir.0, &cli, false);
        let local_dir = dir.0.join("preserved-t1");
        std::fs::create_dir_all(local_dir.join("projects")).unwrap();
        let override_launch = plan.stage_dir.join("launch.sh");
        let override_prompt = plan.stage_dir.join("prompt.md");
        let restore = Restore {
            local_dir: local_dir.clone(),
            override_files: vec![
                (override_launch.clone(), "exec claude --resume session-abc\n".into(), 0o644),
                (override_prompt.clone(), "continue where you left off".into(), 0o644),
            ],
        };
        let outcome = prepare(&plan, &[], Some(&restore)).await.unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
        let log = calls(&dir.0);
        let restore_upload = log
            .iter()
            .find(|c| c.starts_with("sandbox upload factory-aaaabbbb") && c.contains("/sandbox/.claude"))
            .unwrap_or_else(|| panic!("no restore upload in {log:?}"));
        let stage_upload_index = log
            .iter()
            .position(|c| c.contains(&plan.stage_dir.display().to_string()))
            .expect("the run's own files are still uploaded");
        let restore_index = log.iter().position(|c| c == restore_upload).unwrap();
        assert!(
            restore_index < stage_upload_index,
            "the restore upload happens before the run's own files upload: {log:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&override_launch).unwrap(),
            "exec claude --resume session-abc\n",
            "the staged launcher now carries the resumed command"
        );
    }

    #[tokio::test]
    async fn prepare_falls_back_to_the_fresh_files_when_the_restore_upload_fails() {
        let dir = tempdir();
        let cli = fake_cli(&dir.0, "connected", None, false);
        // Make `sandbox upload` fail only for the restore's own source path.
        let script = std::fs::read_to_string(&cli).unwrap().replace(
            "case \"$1 $2\" in\n",
            "case \"$1 $2\" in\n'sandbox upload') case \"$4\" in */preserved-t1/projects) exit 1 ;; esac ;;\n",
        );
        std::fs::write(&cli, script).unwrap();
        let (plan, _cwd) = a_plan(&dir.0, &cli, false);
        let local_dir = dir.0.join("preserved-t1");
        std::fs::create_dir_all(local_dir.join("projects")).unwrap();
        let override_launch = plan.stage_dir.join("launch.sh");
        let fresh_launch = plan
            .stage_files
            .iter()
            .find(|(path, _, _)| path == &override_launch)
            .map(|(_, contents, _)| contents.clone())
            .expect("the plan stages its own fresh launch.sh");
        let restore = Restore {
            local_dir: local_dir.clone(),
            override_files: vec![(override_launch.clone(), "exec claude --resume session-abc\n".into(), 0o644)],
        };
        let outcome = prepare(&plan, &[], Some(&restore)).await.unwrap();
        let reason = match outcome {
            RestoreOutcome::FellBack(reason) => reason,
            other => panic!("expected FellBack, got {other:?}"),
        };
        assert!(!reason.is_empty());
        // `write_files` already staged the fresh launcher before `create`;
        // a failed restore leaves it exactly as it was.
        assert_eq!(
            std::fs::read_to_string(&override_launch).unwrap(),
            fresh_launch,
            "the fresh launcher is untouched by a failed restore"
        );
        assert!(local_dir.exists(), "a failed restore leaves the preserved copy for a later run");
    }

}
