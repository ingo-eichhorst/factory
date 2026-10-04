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

use crate::openshell::{Plan, LABEL_INSTANCE, LABEL_RUN};
use factory_kernel::error::{FactoryError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// The session-meta key `close_session` finds the teardown under.
pub const META_KEY: &str = "openshell";

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
}

impl Teardown {
    pub fn of(plan: &Plan, cwd: &Path, fast_forward: bool) -> Self {
        Self {
            sandbox: plan.sandbox.clone(),
            state_dir: plan
                .policy_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            cwd: cwd.to_path_buf(),
            download: plan.download.clone(),
            download_dir: plan.download_dir.clone(),
            fast_forward,
            delete: plan.delete.clone(),
            service_evidence: None,
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

/// Make the run's sandbox: preflight, write the files, create, upload. On
/// any failure past `create`, the sandbox is deleted before the reason is
/// returned, so a failed dispatch leaves nothing running.
pub async fn prepare(plan: &Plan, providers: &[String]) -> Result<()> {
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
             'sandbox download') mkdir -p \"$5\" && echo downloaded > \"$5/result.txt\" && mkdir -p \"$5/.git\" && echo x > \"$5/.git/HEAD\" ;;\n\
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

    #[tokio::test]
    async fn a_missing_cli_is_a_reason_and_never_a_fallback() {
        let e = resolve_cli(Some("/nonexistent/openshell"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("does not exist"), "{e}");
        let dir = tempdir();
        let (plan, _) = a_plan(&dir.0, &dir.0.join("absent-openshell"), false);
        let e = prepare(&plan, &["factory-claude".into()])
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
        let e = prepare(&plan, &[]).await.unwrap_err().to_string();
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
        let e = prepare(&plan, &["factory-claude".into(), "factory-github".into()])
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
        let mut teardown = Teardown::of(&plan, &cwd, false);
        teardown.state_dir = directory.clone();
        teardown.download_dir = directory.join("download");
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
        for mutation in 0..5 {
            let mut invalid = record.clone();
            match mutation {
                0 => invalid.run = "../outside".into(),
                1 => invalid.teardown.state_dir = dir.0.clone(),
                2 => invalid.teardown.download_dir = dir.0.clone(),
                3 => invalid.teardown.delete.push("unexpected".into()),
                _ => invalid.teardown.sandbox = "factory-other".into(),
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
        let e = prepare(&plan, &[]).await.unwrap_err().to_string();
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
        prepare(&plan, &["factory-claude".into()]).await.unwrap();
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
        assert!(prepare(&plan, &[]).await.is_err());
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
        prepare(&plan, &[]).await.unwrap();
        std::fs::write(cwd.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let teardown = Teardown::of(&plan, &cwd, false);
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
        prepare(&plan, &[]).await.unwrap();
        let outside = dir.0.join("host.txt");
        std::fs::write(&outside, "host data").unwrap();
        std::os::unix::fs::symlink(&outside, cwd.join("result.txt")).unwrap();
        let teardown = Teardown::of(&plan, &cwd, false);
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
}
