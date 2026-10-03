//! `sandbox: openshell` (`#218`): what an agent's `openshell:` block says,
//! and -- pure, with nothing run -- every command and file a run inside an
//! NVIDIA OpenShell sandbox needs. The daemon's `openshell` module is the
//! half that runs them; keeping the shapes here is what lets the tests pin
//! the exact argv, the environment, the providers and the policy without an
//! `openshell` binary, and pin that no secret ever reaches a command line.
//!
//! The shape of a run, end to end:
//!
//! 1. Preflight: the CLI answers, the gateway is connected, every named
//!    provider exists. Any of these failing fails the run with that reason;
//!    nothing ever falls back to starting the harness on the host.
//! 2. `sandbox create --detach` from the image, with the policy file, the
//!    providers, and labels naming the instance and the run.
//! 3. Uploads: the run's working directory to `/sandbox/work/<name>` (git's
//!    ignore rules applied, as OpenShell does by default), its `.git`
//!    directory separately (OpenShell's filtered upload leaves it out), and
//!    a staging directory of the run's own files -- the guide, the hook
//!    settings, the prompt, an env file and the launcher -- to
//!    `/sandbox/.factory-run`.
//! 4. The herdr pane runs `sh '<host launcher>'`, a short line, whose one
//!    command is `openshell sandbox exec --tty` of the in-sandbox launcher.
//!    The pane is still where the run is visible, and what `factory task
//!    output` reads.
//! 5. When the run ends: optionally download the working tree back (never
//!    `.git`), optionally fast-forward the host checkout to its upstream,
//!    then delete the sandbox.

use crate::adapter::agent::{LaunchKind, LaunchSpec};
use crate::error::{FactoryError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The image's `factory` CLI when the block does not say otherwise --
/// where `examples/openshell/` installs it.
pub const DEFAULT_FACTORY_BIN: &str = "/usr/local/bin/factory";

/// How the sandbox names the host it runs on. OpenShell's supervisor
/// resolves it to the gateway host's loopback for the VM driver, which is
/// what lets a policy rule reach a daemon bound to `127.0.0.1`.
pub const HOST_ALIAS: &str = "host.openshell.internal";

/// Where the run's working directory goes: `/sandbox/work/<its name>`.
pub const SANDBOX_WORK_PARENT: &str = "/sandbox/work";

/// Where the run's own files go: the guide, hook settings, prompt, env file
/// and launcher. Its basename is the staging directory's, since an upload
/// keeps the basename of what it uploads.
pub const SANDBOX_RUN_DIR: &str = "/sandbox/.factory-run";
const RUN_DIR_NAME: &str = ".factory-run";

/// The network rule Factory adds so the run can report, unless the policy
/// already has a rule by this name.
pub const CALLBACK_RULE: &str = "factory_callback";

/// Labels on every sandbox Factory makes, so reconcile can find its own and
/// nobody else's.
pub const LABEL_INSTANCE: &str = "factory.instance";
pub const LABEL_RUN: &str = "factory.run";

/// An agent declaration's `openshell:` block.
///
/// ```yaml
/// sandbox: openshell
/// openshell:
///   image: ghcr.io/example/claude-agent:1.0
///   providers: [factory-claude, factory-github]
///   upload: workdir
///   download: none
///   fast_forward: true
///   policy:
///     version: 1
///     network_policies: { ... }
/// ```
///
/// `deny_unknown_fields`, like every other block: a misspelt key would
/// otherwise parse clean and quietly do nothing, which for a sandbox is the
/// worst kind of quiet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenshellConfig {
    /// What the sandbox is made from: an OCI image reference the gateway's
    /// driver can pull or find locally, or a rootfs tar (`.tar`, `.tar.gz`,
    /// `.tgz`) for the VM driver. Passed to `--from` as written.
    pub image: String,
    /// OpenShell providers attached by name with `--provider`. They are
    /// created once on the host (`openshell provider create`); their
    /// credentials live in OpenShell's store and the agent only ever sees
    /// placeholders. Never a secret here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    /// Whether the run's working directory is copied into the sandbox.
    #[serde(default = "Transfer::workdir")]
    pub upload: Transfer,
    /// Whether the working tree is copied back when the run ends. Never
    /// `.git`: git state comes back through the remote (`fast_forward`).
    #[serde(default = "Transfer::none")]
    pub download: Transfer,
    /// After the run, fast-forward the host checkout to its upstream
    /// (`git fetch` and `git merge --ff-only @{u}`). For a run whose work
    /// lands by being pushed and merged, so the host checkout ends level
    /// with what was published. Never forced; a refusal is journaled.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fast_forward: bool,
    /// The `factory` CLI inside the image -- a Linux build; the host's is not.
    #[serde(default = "default_factory_bin")]
    pub factory_bin: String,
    /// How the sandbox reaches the daemon's http interface. Defaults to
    /// `http://host.openshell.internal:<port>` for a daemon bound to
    /// loopback or every address, and to the bound address itself for one
    /// bound to a single other address (a LAN address, say).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback: Option<String>,
    /// Which OpenShell gateway, by the name `openshell gateway add` gave it.
    /// Absent is the CLI's active gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    /// The `openshell` CLI. Absent finds it on the daemon's PATH, then in
    /// Homebrew's and `/usr/local`'s `bin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli: Option<String>,
    /// The OpenShell sandbox policy, verbatim -- what `--policy` reads.
    /// `version: 1` is added if absent, and so is a `factory_callback` rule
    /// letting `factory_bin` reach `callback`, unless the policy names a
    /// rule of that name itself.
    pub policy: serde_yaml_ng::Value,
}

fn default_factory_bin() -> String {
    DEFAULT_FACTORY_BIN.to_string()
}

/// What crosses into or out of the sandbox. A closed list on purpose: the
/// one thing that can cross is the run's working directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    Workdir,
    None,
}

impl Transfer {
    fn workdir() -> Self {
        Self::Workdir
    }
    fn none() -> Self {
        Self::None
    }
}

impl OpenshellConfig {
    /// Everything wrong with this block, as phrases that finish "an
    /// openshell: block that ...". Empty means a sandbox can be made from it.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.image.trim().is_empty() {
            out.push("names no image".to_string());
        }
        for provider in &self.providers {
            if provider.trim().is_empty() || provider.starts_with('-') || provider.contains(char::is_whitespace) {
                out.push(format!("names a provider {provider:?}, which is not a provider name"));
            }
        }
        if !self.factory_bin.starts_with('/') {
            out.push(format!("gives factory_bin {:?}, which is not an absolute path in the image", self.factory_bin));
        }
        if let Some(callback) = &self.callback {
            if !callback.starts_with("http://") {
                out.push(format!("gives callback {callback:?}, which is not a plain http:// address"));
            }
        }
        if let Some(gateway) = &self.gateway {
            if gateway.trim().is_empty() || gateway.starts_with('-') {
                out.push(format!("names a gateway {gateway:?}, which is not a gateway name"));
            }
        }
        match &self.policy {
            serde_yaml_ng::Value::Mapping(map) => {
                match map.get("version") {
                    None => {}
                    Some(v) if v.as_u64() == Some(1) => {}
                    Some(v) => out.push(format!(
                        "gives a policy with version {}, and OpenShell's policy schema is version 1",
                        serde_yaml_ng::to_string(v).unwrap_or_default().trim()
                    )),
                }
                if let Some(rules) = map.get("network_policies") {
                    if !rules.is_mapping() {
                        out.push("gives a policy whose network_policies is not a map of named rules".to_string());
                    }
                }
            }
            _ => out.push("gives a policy that is not a map; it is OpenShell's sandbox policy, verbatim".to_string()),
        }
        out
    }

    /// The policy as written to `--policy`: `version: 1` first, and the
    /// callback rule added when the policy does not name one itself.
    pub fn policy_yaml(&self, callback: &CallbackTarget) -> Result<String> {
        let serde_yaml_ng::Value::Mapping(written) = &self.policy else {
            return Err(FactoryError::BadRequest("the openshell policy is not a map".into()));
        };
        let mut policy = serde_yaml_ng::Mapping::new();
        policy.insert("version".into(), 1.into());
        for (k, v) in written {
            if k.as_str() != Some("version") {
                policy.insert(k.clone(), v.clone());
            }
        }
        let key: serde_yaml_ng::Value = "network_policies".into();
        let mut rules = match policy.remove(&key) {
            Some(serde_yaml_ng::Value::Mapping(rules)) => rules,
            _ => serde_yaml_ng::Mapping::new(),
        };
        if !rules.contains_key(CALLBACK_RULE) {
            let rule = serde_json::json!({
                "endpoints": [{ "host": callback.host, "port": callback.port }],
                "binaries": [{ "path": self.factory_bin }],
            });
            let rule: serde_yaml_ng::Value = serde_yaml_ng::to_value(rule)
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("encoding the callback rule: {e}")))?;
            rules.insert(CALLBACK_RULE.into(), rule);
        }
        policy.insert(key, serde_yaml_ng::Value::Mapping(rules));
        serde_yaml_ng::to_string(&serde_yaml_ng::Value::Mapping(policy))
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("encoding the openshell policy: {e}")))
    }

    /// The callback URL the sandbox is given, from `callback` or the daemon's
    /// own http bind.
    pub fn callback_target(&self, http_bind: Option<&str>) -> Result<CallbackTarget> {
        if let Some(url) = &self.callback {
            return CallbackTarget::parse(url);
        }
        let Some(bind) = http_bind else {
            return Err(FactoryError::BadRequest(
                "sandbox: openshell reports back over the daemon's http interface, and this instance runs none; \
                 add an http interface to daemon.interfaces or set openshell.callback"
                    .into(),
            ));
        };
        let (host, port) = bind
            .rsplit_once(':')
            .and_then(|(h, p)| Some((h, p.parse::<u16>().ok()?)))
            .ok_or_else(|| FactoryError::BadRequest(format!("the http interface's bind {bind:?} names no port")))?;
        // A daemon on loopback or every address is reached through the alias
        // OpenShell maps to the gateway host's loopback. One bound to a
        // single other address -- a LAN address, say -- is not listening on
        // loopback at all, so the sandbox is pointed at that address itself.
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let via_alias = host.is_empty()
            || host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .map(|ip| ip.is_loopback() || ip.is_unspecified())
                .unwrap_or(false);
        let host = if via_alias { HOST_ALIAS.to_string() } else { host.to_string() };
        Ok(CallbackTarget { host, port })
    }
}

/// Where the sandboxed `factory` CLI sends its requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackTarget {
    pub host: String,
    pub port: u16,
}

impl CallbackTarget {
    fn parse(url: &str) -> Result<Self> {
        let bad = || FactoryError::BadRequest(format!("openshell.callback {url:?} is not http://host:port"));
        let rest = url.strip_prefix("http://").ok_or_else(bad)?;
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        let (host, port) = rest.rsplit_once(':').ok_or_else(bad)?;
        let port = port.parse::<u16>().map_err(|_| bad())?;
        if host.is_empty() || host.contains(['/', '@']) {
            return Err(bad());
        }
        Ok(Self { host: host.to_string(), port })
    }

    pub fn url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

/// The sandbox's name for a run: `factory-` and the run id's first eight
/// characters, lowercased -- OpenShell allows lowercase letters, digits and
/// hyphens, nineteen at most.
pub fn sandbox_name(run_id: &str) -> String {
    let short: String = run_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("factory-{short}")
}

/// Everything one run needs, decided before anything is run.
#[derive(Debug, Clone)]
pub struct Plan {
    pub sandbox: String,
    /// `openshell` plus the global `-g <gateway>` when one is named; every
    /// command below starts with these words.
    pub base: Vec<String>,
    pub create: Vec<String>,
    /// In order: the working directory, its `.git`, the run's own files.
    pub uploads: Vec<Vec<String>>,
    /// The policy file `create` reads, and what to write in it.
    pub policy_path: PathBuf,
    pub policy: String,
    /// The staging directory uploaded as `/sandbox/.factory-run`, and the
    /// files to write into it first (`(path, contents, mode)`).
    pub stage_dir: PathBuf,
    pub stage_files: Vec<(PathBuf, String, u32)>,
    /// The script the pane runs, and the short line that runs it.
    pub host_launcher: PathBuf,
    pub host_launcher_script: String,
    pub pane_command: String,
    /// The sandbox's working directory.
    pub workdir: String,
    /// The launch the runtime is handed instead of the harness's own.
    pub launch: LaunchSpec,
    /// Run end.
    pub download: Option<Vec<String>>,
    pub download_dir: PathBuf,
    pub delete: Vec<String>,
}

/// What `plan` needs to know about the run.
pub struct PlanInput<'a> {
    pub config: &'a OpenshellConfig,
    /// The `openshell` CLI, resolved.
    pub cli: &'a str,
    pub instance_id: &'a str,
    pub run_id: &'a str,
    pub task_id: &'a str,
    /// The run's working directory on the host.
    pub cwd: &'a Path,
    /// Where the daemon writes run files (`Factory::guides_dir`) -- every
    /// host path under it in the launch is mapped into the sandbox.
    pub guides_dir: &'a Path,
    /// This run's own host directory for openshell state, under the
    /// instance's `.factory/`, never inside a scope.
    pub state_dir: &'a Path,
    /// The harness's launch as its adapter built it, with `ctx.factory_bin`
    /// and `ctx.callback_url` already the in-sandbox ones.
    pub launch: &'a LaunchSpec,
    /// What would have been submitted after start. Delivered at launch
    /// instead: a multi-line prompt typed into a TUI through a pty submits
    /// at its first newline.
    pub prompt: &'a str,
    pub callback: &'a CallbackTarget,
}

/// Where the run's working directory is inside the sandbox: the host
/// directory's name under `/sandbox/work` when it is uploaded (an upload
/// keeps its basename), `/sandbox/work` itself when nothing is. What the
/// agent is told its working directory is, since the host path names
/// nothing in there.
pub fn workdir_for(cfg: &OpenshellConfig, host_cwd: &Path) -> Result<String> {
    match cfg.upload {
        Transfer::None => Ok(SANDBOX_WORK_PARENT.to_string()),
        Transfer::Workdir => {
            let name = host_cwd
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|n| !n.is_empty())
                .ok_or_else(|| {
                    FactoryError::BadRequest(format!(
                        "the run's working directory {} has no name to upload it under",
                        host_cwd.display()
                    ))
                })?;
            Ok(format!("{SANDBOX_WORK_PARENT}/{name}"))
        }
    }
}

/// Everything a run inside OpenShell needs, as data. Refuses what this
/// version cannot carry faithfully rather than half-carrying it.
pub fn plan(input: &PlanInput<'_>) -> Result<Plan> {
    let cfg = input.config;
    if let Some(problem) = cfg.problems().into_iter().next() {
        return Err(FactoryError::BadRequest(format!("the agent's openshell: block {problem}")));
    }
    let sandbox = sandbox_name(input.run_id);
    let mut base = vec![input.cli.to_string()];
    if let Some(gateway) = &cfg.gateway {
        base.push("-g".into());
        base.push(gateway.clone());
    }
    let cmd = |words: &[&str]| -> Vec<String> {
        let mut out = base.clone();
        out.extend(words.iter().map(|w| w.to_string()));
        out
    };

    let workdir = workdir_for(cfg, input.cwd)?;

    // The working directory, then its `.git`.
    let mut uploads = Vec::new();
    if cfg.upload == Transfer::Workdir {
        let cwd = input.cwd.canonicalize()
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("resolving the working directory: {e}")))?;
        // A harness that needs no guide has not made this directory yet.
        // Resolve its existing ancestor without requiring it to exist.
        let guides = input.guides_dir.ancestors().find_map(|ancestor| {
            ancestor.canonicalize().ok().map(|resolved| resolved.join(input.guides_dir.strip_prefix(ancestor).unwrap()))
        }).ok_or_else(|| FactoryError::BadRequest("the guide directory has no resolvable ancestor".into()))?;
        if guides.starts_with(&cwd) {
            return Err(FactoryError::BadRequest(
                "OpenShell workdir upload would include daemon-owned runtime state; use a scope below the instance root, or openshell.upload: none".into(),
            ));
        }
        let dot_git = input.cwd.join(".git");
        if dot_git.is_file() {
            return Err(FactoryError::BadRequest(format!(
                "{} is a git worktree (its .git is a pointer into another repository), which an OpenShell sandbox \
                 cannot carry -- it has no bind mounts, and the repository the pointer names is not uploaded. \
                 Run this task in the scope itself (factory task create --no-worktree), or set openshell.upload: none",
                input.cwd.display()
            )));
        }
        uploads.push(cmd(&["sandbox", "upload", &sandbox, &path_str(input.cwd)?, SANDBOX_WORK_PARENT]));
        if dot_git.is_dir() {
            uploads.push(cmd(&["sandbox", "upload", &sandbox, &path_str(&dot_git)?, &workdir, "--no-git-ignore"]));
        }
    }

    // The run's own files: every host path under the guides directory that
    // the launch or the prompt names is copied into the staging directory
    // with the guides directory rewritten to its sandbox twin, and named by
    // that twin in the launch.
    let stage_dir = input.state_dir.join(RUN_DIR_NAME);
    let guides = path_str(input.guides_dir)?;
    let guides = guides.trim_end_matches('/').to_string();
    let map = |text: &str| text.replace(&format!("{guides}/"), &format!("{SANDBOX_RUN_DIR}/"));
    let mut referenced: Vec<PathBuf> = Vec::new();
    let mut note = |text: &str| {
        for path in host_paths_under(text, &guides) {
            if !referenced.contains(&path) {
                referenced.push(path);
            }
        }
    };
    for arg in &input.launch.args {
        note(arg);
    }
    for value in input.launch.env.values() {
        note(value);
    }
    note(input.prompt);
    let mut stage_files = Vec::new();
    for host in &referenced {
        let rel = host.strip_prefix(&guides).map_err(|_| FactoryError::BadRequest(format!("{} is not under {guides}", host.display())))?;
        if rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(FactoryError::BadRequest(format!("{} is not a confined guide-file path", host.display())));
        }
        let root = input.guides_dir.canonicalize()
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("resolving the guides directory: {e}")))?;
        let resolved = host.canonicalize()
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("resolving {}: {e}", host.display())))?;
        if !resolved.starts_with(&root) || !resolved.is_file() {
            return Err(FactoryError::BadRequest(format!("{} does not resolve to a file inside the guides directory", host.display())));
        }
        let contents = std::fs::read_to_string(&resolved)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("reading {} to carry it into the sandbox: {e}", host.display())))?;
        stage_files.push((stage_dir.join(rel), map(&contents), 0o644));
    }

    // The environment the harness starts with: the launch's own, mapped, in
    // a file the launcher sources -- never on a command line, where a token
    // would sit in the host's process table.
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in &input.launch.env {
        if !is_env_name(k) || k.starts_with("OPENSHELL_") {
            return Err(FactoryError::BadRequest(format!("the launch sets {k:?}, which cannot be passed into an OpenShell sandbox")));
        }
        env.insert(k.clone(), map(v));
    }
    env.remove("FACTORY_SOCKET");
    env.insert("FACTORY_URL".into(), input.callback.url());
    let env_file: String = env.iter().map(|(k, v)| format!("{k}={}\n", sh_quote(v))).collect();
    stage_files.push((stage_dir.join("env"), env_file, 0o600));

    // The in-sandbox launcher.
    let args: Vec<String> = input.launch.args.iter().map(|a| map(a)).collect();
    let prompt = map(input.prompt);
    let body = match &input.launch.kind {
        LaunchKind::Named(harness) if harness == "claude" => {
            stage_files.push((stage_dir.join("prompt.md"), prompt, 0o644));
            let words: Vec<String> = std::iter::once(harness.clone()).chain(args).map(|w| sh_quote(&w)).collect();
            format!("exec {} \"$(cat {SANDBOX_RUN_DIR}/prompt.md)\"\n", words.join(" "))
        }
        LaunchKind::Named(harness) => {
            return Err(FactoryError::BadRequest(format!(
                "sandbox: openshell carries the claude-code and shell agents in this version, not {harness:?}: \
                 each harness takes its first prompt differently, and only those two are proven inside a sandbox"
            )));
        }
        // The shell agent: nothing to start, and its "prompt" is the line
        // that sources its wrapper script -- run it as the session.
        LaunchKind::Command(cmd) if cmd.is_empty() && args.is_empty() => format!("{prompt}\n"),
        LaunchKind::Command(_) => {
            return Err(FactoryError::BadRequest(
                "sandbox: openshell cannot carry a plugin agent's own command line in this version".into(),
            ));
        }
    };
    let launcher = format!(
        "# Generated by Factory for run {run}; runs inside OpenShell sandbox {sandbox}.\n\
         set -a\n. {SANDBOX_RUN_DIR}/env\nset +a\n\
         mkdir -p {wd} && cd {wd} || exit 1\n\
         {body}",
        run = input.run_id,
        wd = sh_quote(&workdir),
    );
    stage_files.push((stage_dir.join("launch.sh"), launcher, 0o644));
    uploads.push(cmd(&["sandbox", "upload", &sandbox, &path_str(&stage_dir)?, "/sandbox", "--no-git-ignore"]));

    // The host side: one short line typed into the pane.
    let exec = cmd(&[
        "sandbox", "exec", "-n", &sandbox, "--tty", "--no-login-shell", "--workdir", "/sandbox", "--", "bash",
        &format!("{SANDBOX_RUN_DIR}/launch.sh"),
    ]);
    let host_launcher = input.state_dir.join("pane.sh");
    let host_launcher_script = format!(
        "# Generated by Factory for run {run}: the harness runs inside OpenShell sandbox {sandbox}.\n\
         exec {}\n",
        exec.iter().map(|w| sh_quote(w)).collect::<Vec<_>>().join(" "),
        run = input.run_id,
    );
    let pane_command = format!("sh {}", sh_quote(&path_str(&host_launcher)?));
    let launch = LaunchSpec {
        kind: LaunchKind::Command(vec![pane_command.clone()]),
        args: Vec::new(),
        // The pane's own environment needs nothing: the sandbox gets its
        // environment from the env file, and the host shell runs only
        // `openshell`.
        env: BTreeMap::new(),
    };

    let policy_path = input.state_dir.join("policy.yaml");
    let mut create = cmd(&[
        "sandbox", "create", "--name", &sandbox, "--from", &cfg.image, "--policy", &path_str(&policy_path)?,
    ]);
    for provider in &cfg.providers {
        create.push("--provider".into());
        create.push(provider.clone());
    }
    create.extend(
        [
            "--label".to_string(),
            format!("{LABEL_INSTANCE}={}", label_value(input.instance_id)),
            "--label".to_string(),
            format!("{LABEL_RUN}={}", label_value(input.run_id)),
            "--label".to_string(),
            format!("factory.task={}", label_value(input.task_id)),
            "--no-auto-providers".to_string(),
            "--detach".to_string(),
            "-o".to_string(),
            "json".to_string(),
        ],
    );

    let download_dir = input.state_dir.join("download");
    let download = match (cfg.download, cfg.upload) {
        (Transfer::Workdir, Transfer::Workdir) => Some(cmd(&["sandbox", "download", &sandbox, &workdir, &path_str(&download_dir)?])),
        (Transfer::Workdir, Transfer::None) => Some(cmd(&["sandbox", "download", &sandbox, SANDBOX_WORK_PARENT, &path_str(&download_dir)?])),
        _ => None,
    };

    Ok(Plan {
        delete: cmd(&["sandbox", "delete", &sandbox]),
        sandbox,
        base,
        create,
        uploads,
        policy_path,
        policy: cfg.policy_yaml(input.callback)?,
        stage_dir,
        stage_files,
        host_launcher,
        host_launcher_script,
        pane_command,
        workdir,
        launch,
        download,
        download_dir,
    })
}

/// Every absolute path in `text` that starts with `dir/`, up to the first
/// character that cannot be part of one of Factory's own generated paths.
fn host_paths_under(text: &str, dir: &str) -> Vec<PathBuf> {
    let needle = format!("{dir}/");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(&needle) {
        let tail = &rest[at..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '`' | ';' | ')' | '(' | '|' | '&' | '<' | '>'))
            .unwrap_or(tail.len());
        let path = &tail[..end];
        if path.len() > needle.len() {
            out.push(PathBuf::from(path));
        }
        rest = &tail[end..];
    }
    out
}

fn path_str(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| FactoryError::BadRequest(format!("{} is not valid UTF-8", path.display())))
}

fn is_env_name(k: &str) -> bool {
    let mut chars = k.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A label value OpenShell accepts: alphanumerics, `-`, `_`, `.`.
fn label_value(v: &str) -> String {
    v.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .take(63)
        .collect()
}

/// Single-quoted for `sh`, always.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = "\
image: ghcr.io/example/claude-agent:1.0
providers: [factory-claude, factory-github]
fast_forward: true
policy:
  network_policies:
    github_read:
      endpoints:
        - { host: api.github.com, port: 443 }
      binaries: [{ path: /usr/bin/gh }]
";

    fn config() -> OpenshellConfig {
        serde_yaml_ng::from_str(BLOCK).unwrap()
    }

    #[test]
    fn a_block_parses_with_its_defaults() {
        let c = config();
        assert_eq!(c.upload, Transfer::Workdir, "the working directory goes in unless told otherwise");
        assert_eq!(c.download, Transfer::None, "and nothing comes back unless told otherwise");
        assert!(c.fast_forward);
        assert_eq!(c.factory_bin, DEFAULT_FACTORY_BIN);
        assert!(c.problems().is_empty(), "{:?}", c.problems());
    }

    #[test]
    fn an_unknown_key_or_a_missing_image_or_policy_is_refused_by_the_parser() {
        let typo = BLOCK.replace("providers:", "provider:");
        let e = serde_yaml_ng::from_str::<OpenshellConfig>(&typo).unwrap_err().to_string();
        assert!(e.contains("provider"), "{e}");
        let no_image = BLOCK.replace("image: ghcr.io/example/claude-agent:1.0\n", "");
        assert!(serde_yaml_ng::from_str::<OpenshellConfig>(&no_image).unwrap_err().to_string().contains("image"));
        let no_policy = BLOCK[..BLOCK.find("policy:").unwrap()].to_string();
        assert!(serde_yaml_ng::from_str::<OpenshellConfig>(&no_policy).unwrap_err().to_string().contains("policy"));
        let bad_transfer = format!("{BLOCK}download: everything\n");
        assert!(serde_yaml_ng::from_str::<OpenshellConfig>(&bad_transfer).is_err());
    }

    #[test]
    fn a_block_a_sandbox_cannot_be_made_from_says_why() {
        let mut c = config();
        c.image = " ".into();
        c.factory_bin = "factory".into();
        c.callback = Some("https://x:1".into());
        c.policy = serde_yaml_ng::from_str("version: 2").unwrap();
        let problems = c.problems().join("; ");
        for expected in ["no image", "absolute path", "plain http", "version 1"] {
            assert!(problems.contains(expected), "{problems}");
        }
        c.policy = serde_yaml_ng::Value::Sequence(Vec::new());
        assert!(c.problems().join(";").contains("not a map"));
    }

    #[test]
    fn the_policy_gains_its_version_and_the_callback_rule_but_keeps_everything_else() {
        let c = config();
        let target = c.callback_target(Some("127.0.0.1:8787")).unwrap();
        assert_eq!(target.url(), "http://host.openshell.internal:8787");
        let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&c.policy_yaml(&target).unwrap()).unwrap();
        assert_eq!(yaml["version"], serde_yaml_ng::Value::from(1));
        assert_eq!(yaml["network_policies"]["github_read"]["binaries"][0]["path"], serde_yaml_ng::Value::from("/usr/bin/gh"));
        let callback = &yaml["network_policies"][CALLBACK_RULE];
        assert_eq!(callback["endpoints"][0]["host"], serde_yaml_ng::Value::from(HOST_ALIAS));
        assert_eq!(callback["endpoints"][0]["port"], serde_yaml_ng::Value::from(8787));
        assert_eq!(callback["binaries"][0]["path"], serde_yaml_ng::Value::from(DEFAULT_FACTORY_BIN));

        // A policy that writes its own callback rule keeps it as written.
        let mut own = c.clone();
        own.policy = serde_yaml_ng::from_str(
            "network_policies:\n  factory_callback:\n    endpoints: [{ host: example, port: 1 }]\n    binaries: [{ path: /x }]\n",
        )
        .unwrap();
        let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&own.policy_yaml(&target).unwrap()).unwrap();
        assert_eq!(yaml["network_policies"][CALLBACK_RULE]["endpoints"][0]["host"], serde_yaml_ng::Value::from("example"));
    }

    #[test]
    fn a_daemon_bound_to_one_lan_address_is_called_back_there_not_on_loopback() {
        let c = config();
        for bind in ["127.0.0.1:8787", "0.0.0.0:8787", "[::]:8787", "localhost:8787", "[::1]:8787"] {
            assert_eq!(c.callback_target(Some(bind)).unwrap().host, HOST_ALIAS, "{bind}");
        }
        assert_eq!(
            c.callback_target(Some("192.168.188.92:8791")).unwrap(),
            CallbackTarget { host: "192.168.188.92".into(), port: 8791 }
        );
    }

    #[test]
    fn no_http_interface_and_no_callback_is_a_reason_not_a_guess() {
        let e = config().callback_target(None).unwrap_err().to_string();
        assert!(e.contains("http interface"), "{e}");
        let mut c = config();
        c.callback = Some("http://10.0.0.2:9000".into());
        assert_eq!(c.callback_target(None).unwrap(), CallbackTarget { host: "10.0.0.2".into(), port: 9000 });
    }

    #[test]
    fn a_sandbox_name_fits_openshells_rules() {
        let name = sandbox_name("9F1A2B3C-0000-4000-8000-000000000000");
        assert_eq!(name, "factory-9f1a2b3c");
        assert!(name.len() <= 19 && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    }

    struct Fixture {
        _dir: tempdir::Dir,
        cwd: PathBuf,
        guides: PathBuf,
        state: PathBuf,
    }

    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn fixture(git: Option<&str>) -> Fixture {
        let root = std::env::temp_dir().join(format!("factory-openshell-plan-{}", uuid::Uuid::new_v4()));
        let cwd = root.join("projects/awesome-herdr");
        let guides = root.join(".factory/guides");
        let state = root.join(".factory/openshell/run-1");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&guides).unwrap();
        match git {
            Some("dir") => std::fs::create_dir_all(cwd.join(".git")).unwrap(),
            Some("file") => std::fs::write(cwd.join(".git"), "gitdir: /elsewhere\n").unwrap(),
            _ => {}
        }
        std::fs::write(guides.join("run-t1.md"), "the guide\n").unwrap();
        std::fs::write(
            guides.join("hooks-r1.json"),
            "{\"hooks\":{\"Stop\":[{\"hooks\":[{\"command\":\"'/usr/local/bin/factory' task turn-ended 't1' --event stop\"}]}]}}",
        )
        .unwrap();
        Fixture { _dir: tempdir::Dir(root), cwd, guides, state }
    }

    fn claude_launch(f: &Fixture) -> LaunchSpec {
        LaunchSpec {
            kind: LaunchKind::Named("claude".into()),
            args: vec![
                "--append-system-prompt-file".into(),
                f.guides.join("run-t1.md").display().to_string(),
                "--settings".into(),
                f.guides.join("hooks-r1.json").display().to_string(),
                "--model".into(),
                "opus".into(),
            ],
            env: BTreeMap::from([
                ("FACTORY_TOKEN".to_string(), "SECRET-RUN-TOKEN".to_string()),
                ("FACTORY_TASK_TOKEN".to_string(), "SECRET-RUN-TOKEN".to_string()),
                ("FACTORY_TASK_ID".to_string(), "t1".to_string()),
                ("FACTORY_URL".to_string(), "http://host.openshell.internal:8787".to_string()),
                ("DISABLE_AUTOUPDATER".to_string(), "1".to_string()),
            ]),
        }
    }

    fn plan_for(f: &Fixture, launch: &LaunchSpec, prompt: &str, config: &OpenshellConfig) -> Result<Plan> {
        let target = CallbackTarget { host: HOST_ALIAS.into(), port: 8787 };
        plan(&PlanInput {
            config,
            cli: "/opt/homebrew/bin/openshell",
            instance_id: "inst-1",
            run_id: "0a1b2c3d-run",
            task_id: "t1",
            cwd: &f.cwd,
            guides_dir: &f.guides,
            state_dir: &f.state,
            launch,
            prompt,
            callback: &target,
        })
    }

    #[test]
    fn a_claude_run_is_created_with_its_image_policy_providers_and_labels() {
        let f = fixture(Some("dir"));
        let p = plan_for(&f, &claude_launch(&f), "Do the audit.\nThen report.", &config()).unwrap();
        assert_eq!(p.sandbox, "factory-0a1b2c3d");
        let create = p.create.join(" ");
        assert!(create.starts_with("/opt/homebrew/bin/openshell sandbox create --name factory-0a1b2c3d --from ghcr.io/example/claude-agent:1.0 --policy "), "{create}");
        assert!(create.contains("--provider factory-claude --provider factory-github"), "{create}");
        assert!(create.contains("--label factory.instance=inst-1 --label factory.run=0a1b2c3d-run"), "{create}");
        assert!(create.contains("--no-auto-providers --detach"), "a missing provider is an error, never a prompt: {create}");
        assert!(p.policy_path.starts_with(&f.state), "the policy file is daemon state, never in the scope");
    }

    #[test]
    fn the_working_directory_and_its_git_and_the_run_files_are_uploaded_in_that_order() {
        let f = fixture(Some("dir"));
        let p = plan_for(&f, &claude_launch(&f), "go", &config()).unwrap();
        assert_eq!(p.workdir, "/sandbox/work/awesome-herdr");
        assert_eq!(p.uploads.len(), 3);
        assert_eq!(p.uploads[0][1..], ["sandbox", "upload", "factory-0a1b2c3d", &f.cwd.display().to_string(), "/sandbox/work"]);
        assert_eq!(
            p.uploads[1][1..],
            ["sandbox", "upload", "factory-0a1b2c3d", &f.cwd.join(".git").display().to_string(), "/sandbox/work/awesome-herdr", "--no-git-ignore"]
        );
        assert_eq!(p.uploads[2][4..], [p.stage_dir.display().to_string(), "/sandbox".into(), "--no-git-ignore".into()]);
        assert!(p.stage_dir.ends_with(".factory-run"), "an upload keeps its basename: {}", p.stage_dir.display());
    }

    #[test]
    fn a_worktree_is_refused_with_the_way_out() {
        let f = fixture(Some("file"));
        let e = plan_for(&f, &claude_launch(&f), "go", &config()).unwrap_err().to_string();
        assert!(e.contains("git worktree") && e.contains("--no-worktree"), "{e}");
    }

    #[test]
    fn host_paths_in_the_launch_become_sandbox_paths_and_their_files_are_staged() {
        let f = fixture(None);
        let p = plan_for(&f, &claude_launch(&f), "go", &config()).unwrap();
        let launcher = &p.stage_files.iter().find(|(path, _, _)| path.ends_with("launch.sh")).unwrap().1;
        assert!(launcher.contains("'--append-system-prompt-file' '/sandbox/.factory-run/run-t1.md'"), "{launcher}");
        assert!(launcher.contains("'--settings' '/sandbox/.factory-run/hooks-r1.json'"), "{launcher}");
        assert!(launcher.contains("'--model' 'opus'"), "declared args survive: {launcher}");
        assert!(launcher.contains("\"$(cat /sandbox/.factory-run/prompt.md)\""), "the prompt goes in at launch: {launcher}");
        assert!(launcher.contains("cd '/sandbox/work/awesome-herdr'"), "{launcher}");
        assert!(!launcher.contains(&f.guides.display().to_string()), "no host path survives: {launcher}");
        let staged: Vec<String> = p.stage_files.iter().map(|(path, _, _)| path.file_name().unwrap().to_string_lossy().to_string()).collect();
        for name in ["run-t1.md", "hooks-r1.json", "env", "prompt.md", "launch.sh"] {
            assert!(staged.contains(&name.to_string()), "{name} missing from {staged:?}");
        }
        let hooks = &p.stage_files.iter().find(|(path, _, _)| path.ends_with("hooks-r1.json")).unwrap().1;
        assert!(hooks.contains("'/usr/local/bin/factory' task turn-ended"), "the hook calls the image's factory: {hooks}");
    }

    #[test]
    fn the_token_never_reaches_a_command_line_only_the_env_file() {
        let f = fixture(Some("dir"));
        let p = plan_for(&f, &claude_launch(&f), "go", &config()).unwrap();
        let mut argv: Vec<&Vec<String>> = vec![&p.create, &p.delete];
        argv.extend(p.uploads.iter());
        if let Some(d) = &p.download {
            argv.push(d);
        }
        for words in argv {
            assert!(!words.join(" ").contains("SECRET-RUN-TOKEN"), "{words:?}");
        }
        assert!(!p.host_launcher_script.contains("SECRET-RUN-TOKEN"), "{}", p.host_launcher_script);
        assert!(!p.pane_command.contains("SECRET-RUN-TOKEN"));
        assert!(p.launch.env.is_empty(), "the pane's own environment carries nothing");
        let launcher = &p.stage_files.iter().find(|(path, _, _)| path.ends_with("launch.sh")).unwrap().1;
        assert!(!launcher.contains("SECRET-RUN-TOKEN"), "{launcher}");
        let (_, env, mode) = p.stage_files.iter().find(|(path, _, _)| path.ends_with("env")).unwrap();
        assert!(env.contains("FACTORY_TOKEN='SECRET-RUN-TOKEN'"), "{env}");
        assert!(env.contains("FACTORY_URL='http://host.openshell.internal:8787'"), "{env}");
        assert!(!env.contains("FACTORY_SOCKET"), "a host socket path means nothing in there: {env}");
        assert_eq!(*mode, 0o600);
    }

    #[test]
    fn the_pane_types_one_short_line_that_execs_into_the_sandbox() {
        let f = fixture(None);
        let p = plan_for(&f, &claude_launch(&f), "go", &config()).unwrap();
        assert_eq!(p.launch.kind, LaunchKind::Command(vec![p.pane_command.clone()]));
        assert!(p.pane_command.starts_with("sh '") && p.pane_command.len() < 400, "{}", p.pane_command);
        assert!(
            p.host_launcher_script.contains(
                "exec '/opt/homebrew/bin/openshell' 'sandbox' 'exec' '-n' 'factory-0a1b2c3d' '--tty' '--no-login-shell' '--workdir' '/sandbox' '--' 'bash' '/sandbox/.factory-run/launch.sh'"
            ),
            "{}",
            p.host_launcher_script
        );
    }

    #[test]
    fn a_named_gateway_leads_every_command() {
        let f = fixture(None);
        let mut c = config();
        c.gateway = Some("lab".into());
        let p = plan_for(&f, &claude_launch(&f), "go", &c).unwrap();
        for words in [&p.create, &p.delete, &p.uploads[0]] {
            assert_eq!(words[..3], ["/opt/homebrew/bin/openshell", "-g", "lab"], "{words:?}");
        }
    }

    #[test]
    fn download_is_planned_only_when_asked_for_and_never_into_the_scope_directly() {
        let f = fixture(None);
        assert!(plan_for(&f, &claude_launch(&f), "go", &config()).unwrap().download.is_none());
        let mut c = config();
        c.download = Transfer::Workdir;
        let p = plan_for(&f, &claude_launch(&f), "go", &c).unwrap();
        let download = p.download.unwrap();
        assert_eq!(download[1..4], ["sandbox", "download", "factory-0a1b2c3d"]);
        assert_eq!(download[4], "/sandbox/work/awesome-herdr");
        assert!(PathBuf::from(&download[5]).starts_with(&f.state), "downloaded to staging, then copied without .git");
    }

    #[test]
    fn the_shell_agents_script_line_becomes_the_session_with_its_paths_mapped() {
        let f = fixture(None);
        let script = f.guides.join("shell-r1.sh");
        std::fs::write(&script, format!("_factory_bin='/usr/local/bin/factory'\nexport FACTORY_UPSTREAM_FILE='{}/upstream-t1.json'\necho hi\n", f.guides.display())).unwrap();
        let launch = LaunchSpec { kind: LaunchKind::Command(Vec::new()), args: Vec::new(), env: BTreeMap::new() };
        let prompt = format!(". '{}'", script.display());
        let p = plan_for(&f, &launch, &prompt, &config()).unwrap();
        let launcher = &p.stage_files.iter().find(|(path, _, _)| path.ends_with("launch.sh")).unwrap().1;
        assert!(launcher.ends_with(". '/sandbox/.factory-run/shell-r1.sh'\n"), "{launcher}");
        let staged = &p.stage_files.iter().find(|(path, _, _)| path.ends_with("shell-r1.sh")).unwrap().1;
        assert!(staged.contains("FACTORY_UPSTREAM_FILE='/sandbox/.factory-run/upstream-t1.json'"), "{staged}");
    }

    #[test]
    fn a_harness_not_proven_in_a_sandbox_is_refused_rather_than_guessed_at() {
        let f = fixture(None);
        let mut launch = claude_launch(&f);
        launch.kind = LaunchKind::Named("codex".into());
        let e = plan_for(&f, &launch, "go", &config()).unwrap_err().to_string();
        assert!(e.contains("codex"), "{e}");
    }

    /// The awesome-herdr block the README and the PR hand to a person to
    /// paste is a block this build loads -- not just one that looks right.
    #[test]
    fn the_shipped_awesome_herdr_example_loads_and_validates() {
        let doc: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(include_str!("../../../examples/openshell/awesome-herdr.config.yaml")).unwrap();
        let mut scope: crate::config::Scope = serde_yaml_ng::from_value(doc["scope"].clone()).unwrap();
        scope.path = PathBuf::from("projects/awesome-herdr");
        let agent = scope.declared_agents().remove(0);
        assert_eq!(agent.name(), "awesome-herdr-curator");
        assert_eq!(agent.sandbox, crate::config::Sandbox::Openshell);
        crate::config::refuse_bad_openshell(&scope, &agent).unwrap();
        let block = agent.openshell.unwrap();
        assert!(block.fast_forward && block.download == Transfer::None && block.upload == Transfer::Workdir);
        assert_eq!(block.providers, ["factory-claude", "factory-github"]);
        let target = block.callback_target(Some("192.168.188.92:8791")).unwrap();
        let policy: serde_yaml_ng::Value = serde_yaml_ng::from_str(&block.policy_yaml(&target).unwrap()).unwrap();
        for rule in ["github_read", "awesome_herdr_publish", "sources", CALLBACK_RULE] {
            assert!(policy["network_policies"].get(rule).is_some(), "{rule} missing");
        }
        assert_eq!(policy["network_policies"][CALLBACK_RULE]["endpoints"][0]["host"], serde_yaml_ng::Value::from("192.168.188.92"));
    }

    #[test]
    fn paths_are_found_inside_quotes_and_flags() {
        let found = host_paths_under("a '/g/x.sh' and --f=/g/y.json /other/z /g/", "/g");
        assert_eq!(found, vec![PathBuf::from("/g/x.sh"), PathBuf::from("/g/y.json")]);
    }

    #[test]
    fn a_prompt_cannot_stage_host_files_by_traversing_out_of_guides() {
        let f = fixture(None);
        let outside = f.guides.parent().unwrap().join("host-secret.txt");
        std::fs::write(&outside, "private host data").unwrap();
        let prompt = format!("read {}/../host-secret.txt", f.guides.display());
        assert!(plan_for(&f, &claude_launch(&f), &prompt, &config()).is_err());
    }

    #[test]
    fn a_guide_symlink_cannot_stage_a_host_file_outside_guides() {
        let f = fixture(None);
        let outside = f.guides.parent().unwrap().join("host-secret.txt");
        std::fs::write(&outside, "private host data").unwrap();
        std::os::unix::fs::symlink(&outside, f.guides.join("linked-secret.txt")).unwrap();
        let prompt = format!("read {}/linked-secret.txt", f.guides.display());
        assert!(plan_for(&f, &claude_launch(&f), &prompt, &config()).is_err());
    }

    #[test]
    fn uploading_a_workdir_that_contains_instance_runtime_state_is_refused() {
        let mut f = fixture(None);
        f.cwd = f.guides.parent().unwrap().to_path_buf();
        let error = plan_for(&f, &claude_launch(&f), "go", &config()).unwrap_err().to_string();
        assert!(error.contains("daemon-owned runtime state"), "{error}");
    }
}
