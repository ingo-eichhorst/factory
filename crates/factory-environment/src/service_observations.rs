//! L2's immutable enforcement evidence and live OpenShell log read. No task
//! store, policy evaluator, scanner or credential source is consulted here.
use crate::sandbox_runtime::Teardown;
use chrono::{DateTime, Utc};
use factory_kernel::error::{FactoryError, Result};
use factory_kernel::{
    AccessDisposition, ObservedServiceAccess, ObservedTransport, SandboxServiceCapture,
    SandboxServiceEvidenceFact,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_CAPTURES: usize = 50;
const MAX_ACCESSES: usize = 2000;
const MAX_LIVE: usize = 8;
const SOURCE: &str = "OpenShell supervisor/proxy OCSF";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureContext {
    pub root: PathBuf,
    pub instance: String,
    pub scope: String,
    pub agent: String,
    pub task: String,
    pub run: String,
    pub base: Vec<String>,
}

impl CaptureContext {
    pub fn valid(&self, teardown: &Teardown) -> bool {
        let mut delete = self.base.clone();
        delete.extend(["sandbox".into(), "delete".into(), teardown.sandbox.clone()]);
        self.root.is_absolute()
            && safe_id(&self.run)
            && !self.base.is_empty()
            && !self.base[0].is_empty()
            && [&self.scope, &self.agent, &self.task, &self.instance]
                .into_iter()
                .all(|s| safe_text(s, 512))
            && teardown.state_dir == self.root.join(".factory/openshell").join(&self.run)
            && teardown.sandbox
                == format!("factory-{}", self.run.chars().take(8).collect::<String>())
            && teardown.delete == delete
    }
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn safe_text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

/// Human log timestamps use decimal Unix seconds. Integer parsing avoids
/// rounding the event's millisecond identity through a floating point number.
fn timestamp(value: &str) -> Option<DateTime<Utc>> {
    let (seconds, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 9 || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let nanos = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u32>().ok()? * 10u32.pow(9 - fraction.len() as u32)
    };
    DateTime::from_timestamp(seconds.parse().ok()?, nanos)
}

fn bracket<'a>(text: &mut &'a str) -> Option<&'a str> {
    let rest = text.trim_start().strip_prefix('[')?;
    let (value, next) = rest.split_once(']')?;
    *text = next.trim_start();
    Some(value.trim())
}

/// Exact authority only. Never retain URL userinfo, paths, queries, fragments
/// or arbitrary log text. IPv6 and non-default ports retain their identity.
fn authority(value: &str) -> Option<String> {
    let value = value
        .strip_suffix("/tcp")
        .or_else(|| value.strip_suffix("/udp"))
        .unwrap_or(value);
    let (host, port) = value.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    if port == 0 || host.is_empty() || host.len() > 253 {
        return None;
    }
    if host.starts_with('[') {
        host.strip_prefix('[')?
            .strip_suffix(']')?
            .parse::<std::net::Ipv6Addr>()
            .ok()?;
    } else if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return None;
    }
    Some(format!("{}:{port}", host.to_ascii_lowercase()))
}

fn http_target(value: &str) -> Option<String> {
    let (scheme, rest) = value.split_once("://")?;
    let port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let host = rest.split(['/', '?', '#']).next()?;
    if host.contains('@') {
        return None;
    }
    authority(host).or_else(|| authority(&format!("{host}:{port}")))
}

/// Parse only the supervisor's native OCSF security records received over
/// gRPC. A terminal line, standard tracing event, configured listener, relay
/// without an endpoint, or Landlock path declaration proves no service use.
pub fn parse(logs: &str) -> Vec<ObservedServiceAccess> {
    let mut rows = Vec::new();
    for mut text in logs.lines().filter(|line| line.len() <= 8192) {
        let Some(at) = bracket(&mut text).and_then(timestamp) else {
            continue;
        };
        if bracket(&mut text) != Some("sandbox")
            || bracket(&mut text) != Some("OCSF")
            || bracket(&mut text) != Some("ocsf")
        {
            continue;
        }
        let Some((event, rest)) = text.split_once(' ') else {
            continue;
        };
        let mut rest = rest.trim_start();
        if bracket(&mut rest).is_none() {
            continue;
        }
        let Some((action, detail)) = rest.split_once(' ') else {
            continue;
        };
        let disposition = match action {
            "ALLOWED" => AccessDisposition::Allowed,
            "DENIED" => AccessDisposition::Denied,
            "BLOCKED" => AccessDisposition::Blocked,
            _ => continue,
        };
        let (process, target) = match event {
            "NET:OPEN" | "NET:REFUSE" => {
                let (process, endpoint) = match detail.split_once(" -> ") {
                    Some((process, endpoint)) => (
                        safe_text(process, 512).then(|| process.to_string()),
                        endpoint,
                    ),
                    // Some control-mode decisions have a destination but no
                    // binary identity. Keep that absence unknown; do not use
                    // a nearby process event to fill it in.
                    None => (None, detail),
                };
                let Some(target) = endpoint.split_whitespace().next().and_then(authority) else {
                    continue;
                };
                (process, target)
            }
            event if event.starts_with("HTTP:") => {
                let (process, request) = match detail.split_once(" -> ") {
                    Some((process, request)) => (
                        safe_text(process, 512).then(|| process.to_string()),
                        request,
                    ),
                    None => (None, detail),
                };
                let mut words = request.split_whitespace();
                let Some(method) = words.next() else {
                    continue;
                };
                if event.strip_prefix("HTTP:") != Some(method) {
                    continue;
                }
                let Some(target) = words.next().and_then(http_target) else {
                    continue;
                };
                (process, target)
            }
            _ => continue,
        };
        let policy = detail
            .split_once("[policy:")
            .and_then(|(_, value)| value.split([' ', ']']).next())
            .filter(|value| safe_text(value, 256))
            .map(str::to_string);
        rows.push(ObservedServiceAccess {
            at,
            transport: ObservedTransport::Network,
            target,
            disposition,
            process,
            policy,
        });
    }
    normalize(&mut rows);
    rows
}

fn normalize(rows: &mut Vec<ObservedServiceAccess>) {
    rows.sort_by_cached_key(|row| (row.at, serde_json::to_string(row).unwrap_or_default()));
    rows.dedup();
}

/// Keep draining both pipes even past the cap, but retain bounded memory.
async fn drain(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok((bytes, truncated));
        }
        let keep = read.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
}

async fn cli(argv: &[String]) -> std::result::Result<String, ()> {
    let (program, args) = argv.split_first().ok_or(())?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|_| ())?;
    let pid = child.id();
    let stdout = child.stdout.take().ok_or(())?;
    let stderr = child.stderr.take().ok_or(())?;
    let work = async { tokio::try_join!(child.wait(), drain(stdout, MAX_BYTES), drain(stderr, 0)) };
    match tokio::time::timeout(Duration::from_secs(5), work).await {
        Ok(Ok((status, (bytes, truncated), _))) if status.success() && !truncated => {
            String::from_utf8(bytes).map_err(|_| ())
        }
        _ => {
            #[cfg(unix)]
            if let Some(pid) = pid.and_then(|id| i32::try_from(id).ok()) {
                // SAFETY: this is the process group created for this exact child.
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            Err(())
        }
    }
}

pub async fn collect(context: &CaptureContext, teardown: &Teardown) -> SandboxServiceCapture {
    let mut capture = SandboxServiceCapture {
        id: String::new(),
        run_id: context.run.clone(),
        task_id: context.task.clone(),
        agent: context.agent.clone(),
        sandbox: teardown.sandbox.clone(),
        captured_at: Utc::now(),
        source: SOURCE.into(),
        partial: true,
        accesses: Vec::new(),
        issue: None,
    };
    let result = async {
        if !context.valid(teardown) {
            return Err("Invalid sandbox evidence provenance");
        }
        let mut get = context.base.clone();
        get.extend(["sandbox", "get", &teardown.sandbox, "-o", "json"].map(String::from));
        let raw = cli(&get)
            .await
            .map_err(|_| "Sandbox ownership could not be verified")?;
        let object: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| "Sandbox ownership could not be verified")?;
        if object["labels"]["factory.instance"] != context.instance
            || object["labels"]["factory.run"] != context.run
        {
            return Err("Sandbox ownership did not match; no logs accepted");
        }
        let mut logs = context.base.clone();
        logs.extend(
            [
                "logs",
                &teardown.sandbox,
                "--source",
                "sandbox",
                "-n",
                "1000",
                "--color",
                "never",
            ]
            .map(String::from),
        );
        let raw = cli(&logs)
            .await
            .map_err(|_| "Sandbox enforcement log was unavailable or exceeded its bound")?;
        capture.accesses = parse(&raw);
        Ok(())
    }
    .await;
    capture.issue = result.err().map(str::to_string);
    capture.id = capture_identity(&context.instance, &context.scope, &capture);
    capture
}

fn capture_identity(instance: &str, scope: &str, capture: &SandboxServiceCapture) -> String {
    let identity = serde_json::to_vec(&(
        instance,
        scope,
        &capture.run_id,
        &capture.accesses,
        &capture.issue,
    ))
    .unwrap_or_default();
    format!("{:x}", Sha256::digest(identity))
}

/// The report callback can beat log forwarding. Give final records a short
/// bounded drain window before deleting their only source; union the windows.
pub async fn collect_final(context: &CaptureContext, teardown: &Teardown) -> SandboxServiceCapture {
    let mut capture = collect(context, teardown).await;
    for _ in 0..2 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let next = collect(context, teardown).await;
        capture.accesses.extend(next.accesses);
        normalize(&mut capture.accesses);
        if capture.accesses.len() > 1000 {
            capture.accesses.drain(..capture.accesses.len() - 1000);
        }
        capture.captured_at = next.captured_at;
        capture.issue = next.issue;
    }
    capture.id = capture_identity(&context.instance, &context.scope, &capture);
    capture
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    instance: String,
    scope: String,
    capture: SandboxServiceCapture,
}

pub fn save(
    context: &CaptureContext,
    teardown: &Teardown,
    capture: &SandboxServiceCapture,
) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if !context.valid(teardown)
        || capture.id != capture_identity(&context.instance, &context.scope, capture)
        || capture.run_id != context.run
        || capture.task_id != context.task
        || capture.agent != context.agent
        || capture.sandbox != teardown.sandbox
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid evidence provenance",
        ));
    }
    let logs = context.root.join(".factory/logs");
    if std::fs::symlink_metadata(&logs).is_ok_and(|metadata| !metadata.is_dir()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "logs directory is unsafe",
        ));
    }
    let directory = context.root.join(".factory/logs/service-evidence");
    std::fs::create_dir_all(&directory)?;
    if std::fs::symlink_metadata(&directory)?
        .file_type()
        .is_symlink()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "evidence directory is a symlink",
        ));
    }
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let bytes = serde_json::to_vec(&Stored {
        version: 1,
        instance: context.instance.clone(),
        scope: context.scope.clone(),
        capture: capture.clone(),
    })?;
    // A fresh immutable receipt per collection; the content identity below
    // deduplicates repeated snapshots on read. Publish only a complete file.
    let name = uuid::Uuid::new_v4().to_string();
    let temporary = directory.join(format!(".{name}.tmp"));
    let path = directory.join(format!("{name}.json"));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(directory)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn archived(
    root: &Path,
    instance: &str,
    scope: &str,
) -> std::io::Result<SandboxServiceEvidenceFact> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut fact = SandboxServiceEvidenceFact {
        scope: scope.into(),
        captures: Vec::new(),
        findings: Vec::new(),
    };
    if std::fs::symlink_metadata(root.join(".factory/logs"))
        .is_ok_and(|metadata| !metadata.is_dir())
    {
        fact.findings
            .push("Stored sandbox evidence directory could not be read safely".into());
        return Ok(fact);
    }
    let directory = root.join(".factory/logs/service-evidence");
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(fact),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
        _ => {
            fact.findings
                .push("Stored sandbox evidence directory could not be read safely".into());
            return Ok(fact);
        }
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => {
            fact.findings
                .push("Stored sandbox evidence directory could not be read safely".into());
            return Ok(fact);
        }
    };
    for (index, entry) in entries.enumerate() {
        if index >= 5000 {
            fact.findings
                .push("Stored sandbox evidence walk reached its bound".into());
            break;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let read = || -> std::io::Result<Option<SandboxServiceCapture>> {
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(&path)?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_BYTES as u64 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unsafe evidence file",
                ));
            }
            let mut bytes = Vec::new();
            std::io::Read::by_ref(&mut file)
                .take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            let stored: Stored = serde_json::from_slice(&bytes)?;
            if stored.version != 1
                || path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    .is_none()
                || stored.capture.source != SOURCE
                || !safe_id(&stored.capture.run_id)
                || stored.capture.id
                    != capture_identity(&stored.instance, &stored.scope, &stored.capture)
                || !stored.capture.partial
                || [&stored.capture.task_id, &stored.capture.agent]
                    .into_iter()
                    .any(|value| !safe_text(value, 512))
                || stored.capture.sandbox
                    != format!(
                        "factory-{}",
                        stored.capture.run_id.chars().take(8).collect::<String>()
                    )
                || stored
                    .capture
                    .issue
                    .as_ref()
                    .is_some_and(|value| !safe_text(value, 256))
                || stored.capture.accesses.len() > 1000
                || stored.capture.accesses.iter().any(|row| {
                    row.transport != ObservedTransport::Network
                        || authority(&row.target).as_ref() != Some(&row.target)
                        || row
                            .process
                            .as_ref()
                            .is_some_and(|value| !safe_text(value, 512))
                        || row
                            .policy
                            .as_ref()
                            .is_some_and(|value| !safe_text(value, 256))
                })
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid evidence file",
                ));
            }
            Ok((stored.instance == instance && stored.scope == scope).then_some(stored.capture))
        };
        match read() {
            Ok(Some(capture)) => retain_capture(&mut fact, capture),
            Ok(None) => (),
            Err(_) => fact
                .findings
                .push("A stored sandbox evidence record could not be read safely".into()),
        }
    }
    fact.findings.sort();
    fact.findings.dedup();
    Ok(fact)
}

// Bound memory while walking receipts, not only after reading the entire
// archive. Keep the newest distinct captures, with stable content identities.
fn retain_capture(fact: &mut SandboxServiceEvidenceFact, capture: SandboxServiceCapture) {
    if let Some(existing) = fact.captures.iter_mut().find(|row| row.id == capture.id) {
        if capture.captured_at < existing.captured_at {
            *existing = capture;
        }
    } else {
        fact.captures.push(capture);
    }
    fact.captures
        .sort_by(|a, b| b.captured_at.cmp(&a.captured_at).then(a.id.cmp(&b.id)));
    if fact.captures.len() > MAX_CAPTURES {
        fact.captures.truncate(MAX_CAPTURES);
        if !fact
            .findings
            .iter()
            .any(|row| row == "Sandbox evidence report reached its capture bound")
        {
            fact.findings
                .push("Sandbox evidence report reached its capture bound".into());
        }
    }
}

/// A live L2 read from owned archive/cleanup capabilities and plain scope identities.
pub struct Provider {
    pub root: PathBuf,
    pub instance: String,
    pub scopes: factory_kernel::ScopeTree,
}
impl factory_kernel::FactProvider for Provider {
    type Level = factory_kernel::L2;
}
#[async_trait::async_trait]
impl factory_kernel::Provide<SandboxServiceEvidenceFact> for Provider {
    type Query = String;
    type Value = SandboxServiceEvidenceFact;
    type Error = FactoryError;
    async fn get(&self, query: &String) -> Result<SandboxServiceEvidenceFact> {
        let scope = self.scopes.scope(query)?.name.clone();
        let root = self.root.clone();
        let instance = self.instance.clone();
        let (mut fact, records) = tokio::task::spawn_blocking(move || {
            let mut fact = archived(&root, &instance, &scope).map_err(|_| {
                FactoryError::BadRequest("Stored sandbox service evidence could not be read".into())
            })?;
            let records =
                crate::sandbox_runtime::pending(&root.join(".factory/openshell"), &instance)
                    .unwrap_or_else(|_| {
                        fact.findings
                            .push("Live sandbox service evidence could not be read".into());
                        Vec::new()
                    });
            Ok::<_, FactoryError>((fact, records))
        })
        .await
        .map_err(|_| {
            FactoryError::BadRequest("Sandbox service evidence walk did not finish".into())
        })??;
        let mut live = tokio::task::JoinSet::new();
        for record in records {
            let Some(context) = record.teardown.service_evidence.clone() else {
                continue;
            };
            if context.scope != fact.scope {
                continue;
            }
            if live.len() >= MAX_LIVE {
                fact.findings
                    .push("Live sandbox evidence collection reached its bound".into());
                break;
            }
            live.spawn(async move { collect(&context, &record.teardown).await });
        }
        while let Some(result) = live.join_next().await {
            match result {
                Ok(capture) => retain_capture(&mut fact, capture),
                Err(_) => fact
                    .findings
                    .push("A live sandbox evidence read did not finish".into()),
            }
        }
        let mut remaining = MAX_ACCESSES;
        for capture in &mut fact.captures {
            if capture.accesses.len() > remaining {
                capture.accesses.truncate(remaining);
                capture.partial = true;
                fact.findings
                    .push("Sandbox evidence report reached its access bound".into());
            }
            remaining -= capture.accesses.len();
        }
        fact.findings.sort();
        fact.findings.dedup();
        Ok(fact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_kernel::Provide;
    use std::os::unix::fs::PermissionsExt;

    const LOGS: &str = "[1775014132.118] [sandbox] [OCSF ] [ocsf] NET:OPEN [INFO] ALLOWED /usr/bin/curl(58) -> api.github.com:443 [policy:github_api engine:opa]\n\
        [1775014132.690] [sandbox] [OCSF ] [ocsf] NET:OPEN [MED] DENIED /usr/bin/curl(64) -> httpbin.org:443 [policy:- engine:opa] [reason:no matching policy]\n\
        [1775014133.000] [sandbox] [OCSF ] [ocsf] HTTP:GET [INFO] ALLOWED curl(88) -> GET https://api.github.com/private?token=SECRET_PAYLOAD#fragment [policy:github_api engine:opa]\n";

    struct Rig {
        root: PathBuf,
        context: CaptureContext,
        teardown: Teardown,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Rig {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("factory-service-evidence-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let cli = root.join("openshell");
            let base = vec![cli.display().to_string()];
            let run = "aaaabbbb-run";
            let state_dir = root.join(".factory/openshell").join(run);
            std::fs::create_dir_all(&state_dir).unwrap();
            let teardown = Teardown {
                sandbox: "factory-aaaabbbb".into(),
                state_dir: state_dir.clone(),
                cwd: root.clone(),
                download: None,
                download_dir: state_dir.join("download"),
                fast_forward: false,
                delete: vec![
                    base[0].clone(),
                    "sandbox".into(),
                    "delete".into(),
                    "factory-aaaabbbb".into(),
                ],
                service_evidence: None,
            };
            let context = CaptureContext {
                root: root.clone(),
                instance: "inst".into(),
                scope: "demo".into(),
                agent: "curator".into(),
                task: "task".into(),
                run: run.into(),
                base,
            };
            let rig = Self {
                root,
                context,
                teardown,
            };
            rig.cli("inst", LOGS, false);
            rig
        }

        fn cli(&self, instance: &str, logs: &str, failure: bool) {
            std::fs::write(self.root.join("logs"), logs).unwrap();
            let labels = serde_json::json!({"labels": {"factory.instance": instance, "factory.run": self.context.run}});
            let script = format!("#!/bin/sh\necho \"$*\" >> '{}/calls'\ncase \"$1 $2\" in\n'sandbox get') printf '%s\\n' '{}' ;;\n'logs factory-aaaabbbb') if [ {} = 1 ]; then echo SECRET_ERROR >&2; exit 1; fi; /bin/cat '{}/logs'; echo log-read >> '{}/calls' ;;\n'sandbox delete') test -d '{}/.factory/logs/service-evidence' ;;\nesac\n", self.root.display(), labels,
                if failure { 1 } else { 0 }, self.root.display(), self.root.display(), self.root.display());
            std::fs::write(&self.context.base[0], script).unwrap();
            std::fs::set_permissions(
                &self.context.base[0],
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        fn provider(&self) -> Provider {
            Provider {
                root: self.root.clone(),
                instance: "inst".into(),
                scopes: factory_kernel::ScopeTree {
                    scopes: vec![
                        factory_kernel::ScopeNode {
                            name: "demo".into(),
                            path: ".".into(),
                        },
                        factory_kernel::ScopeNode {
                            name: "other".into(),
                            path: "other".into(),
                        },
                    ],
                },
            }
        }

        fn pending(&self) {
            let mut teardown = self.teardown.clone();
            teardown.service_evidence = Some(self.context.clone());
            crate::sandbox_runtime::Pending {
                instance: self.context.instance.clone(),
                run: self.context.run.clone(),
                task: self.context.task.clone(),
                base: self.context.base.clone(),
                teardown,
            }
            .save()
            .unwrap();
        }
    }

    #[test]
    fn native_access_records_preserve_times_and_dispositions_but_not_payloads() {
        let rows = parse(LOGS);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].at.timestamp_millis(), 1775014132118);
        assert_eq!(rows[0].target, "api.github.com:443");
        assert_eq!(rows[1].disposition, AccessDisposition::Denied);
        assert_eq!(rows[2].target, "api.github.com:443");
        assert!(!serde_json::to_string(&rows)
            .unwrap()
            .contains("SECRET_PAYLOAD"));
        assert!(!serde_json::to_string(&rows).unwrap().contains("private"));
        assert_eq!(parse(&format!("{LOGS}{LOGS}")), rows);
    }

    #[test]
    fn startup_configured_paths_relays_listeners_and_terminal_text_are_not_access() {
        let logs = "[1775014132.1] [sandbox] [INFO ] [ocsf] NET:OPEN [INFO] ALLOWED curl(1) -> host:443\n\
            [1775014132.1] [gateway] [OCSF ] [ocsf] NET:OPEN [INFO] ALLOWED curl(1) -> host:443\n\
            [1775014132.1] [sandbox] [OCSF ] [terminal] NET:OPEN [INFO] ALLOWED curl(1) -> host:443\n\
            [1775014132.1] [sandbox] [OCSF ] [ocsf] CONFIG:ENABLED [INFO] Applying Landlock filesystem sandbox [path:/data]\n\
            [1775014132.1] [sandbox] [OCSF ] [ocsf] NET:LISTEN [INFO] ALLOWED curl(1) -> host:443\n\
            [1775014132.1] [sandbox] [OCSF ] [ocsf] NET:OPEN [INFO] [msg:relay open (channel_id=ch-42)]\n\
            [1775014132.1] [sandbox] [OCSF ] [ocsf] HTTP:GET [INFO] ALLOWED GET https://user:SECRET@host/path\n\
            [bad] [sandbox] [OCSF ] [ocsf] NET:OPEN [INFO] ALLOWED curl(1) -> host:443\n";
        assert!(parse(logs).is_empty());
        assert_eq!(
            authority("[2001:db8::1]:8443/tcp"),
            Some("[2001:db8::1]:8443".into())
        );
        assert_eq!(authority("host:0"), None);
        assert_eq!(authority("host:443?SECRET"), None);
        assert_eq!(
            http_target("http://HOST:8080/path?SECRET"),
            Some("host:8080".into())
        );
    }

    #[tokio::test]
    async fn ownership_is_checked_before_logs_and_errors_never_disclose_cli_output() {
        let rig = Rig::new();
        rig.cli("foreign", LOGS, false);
        let capture = collect(&rig.context, &rig.teardown).await;
        assert!(capture.accesses.is_empty());
        assert!(capture.issue.unwrap().contains("ownership did not match"));
        assert!(!std::fs::read_to_string(rig.root.join("calls"))
            .unwrap()
            .contains("logs "));
        rig.cli("inst", LOGS, true);
        let capture = collect(&rig.context, &rig.teardown).await;
        assert!(capture.accesses.is_empty());
        assert!(!serde_json::to_string(&capture)
            .unwrap()
            .contains("SECRET_ERROR"));
        let mut invalid = rig.context.clone();
        invalid.root = rig.root.join("wrong");
        assert!(!invalid.valid(&rig.teardown));
        assert!(save(&invalid, &rig.teardown, &capture).is_err());
        assert!(!invalid.root.exists());
    }

    #[tokio::test]
    async fn live_port_reads_changes_without_events_and_archive_survives_teardown() {
        let rig = Rig::new();
        rig.pending();
        let provider = rig.provider();
        let facts = factory_kernel::Facts::<factory_kernel::People>::new();
        let live = facts
            .get::<SandboxServiceEvidenceFact, _>(&provider, &"demo".into())
            .await
            .unwrap();
        assert_eq!(live.captures.len(), 1);
        assert_eq!(live.captures[0].accesses.len(), 3);
        assert!(live.captures[0].partial);
        assert!(facts
            .get::<SandboxServiceEvidenceFact, _>(&provider, &"other".into())
            .await
            .unwrap()
            .captures
            .is_empty());
        assert!(facts
            .get::<SandboxServiceEvidenceFact, _>(&provider, &"missing".into())
            .await
            .is_err());
        std::fs::write(rig.root.join("logs"), "").unwrap();
        assert!(facts
            .get::<SandboxServiceEvidenceFact, _>(&provider, &"demo".into())
            .await
            .unwrap()
            .captures[0]
            .accesses
            .is_empty());
        std::fs::write(rig.root.join("logs"), LOGS).unwrap();
        let mut teardown = rig.teardown.clone();
        teardown.service_evidence = Some(rig.context.clone());
        let notes = crate::sandbox_runtime::finish(&teardown).await;
        assert!(
            notes.iter().any(|line| line.starts_with("deleted sandbox")),
            "{notes:?}"
        );
        assert!(!teardown.state_dir.exists());
        let restarted = rig.provider();
        let after = factory_kernel::Facts::<factory_kernel::People>::new()
            .get::<SandboxServiceEvidenceFact, _>(&restarted, &"demo".into())
            .await
            .unwrap();
        assert_eq!(after.captures[0].accesses, live.captures[0].accesses);
        let file = std::fs::read_dir(rig.root.join(".factory/logs/service-evidence"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn evidence_is_append_only_and_unsafe_files_do_not_become_observations() {
        use std::os::unix::fs::symlink;
        let rig = Rig::new();
        let capture = collect(&rig.context, &rig.teardown).await;
        save(&rig.context, &rig.teardown, &capture).unwrap();
        save(&rig.context, &rig.teardown, &capture).unwrap();
        let directory = rig.root.join(".factory/logs/service-evidence");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        symlink(rig.root.join("logs"), directory.join("symlink.json")).unwrap();
        std::fs::write(directory.join("bad.json"), "SECRET_ERROR").unwrap();
        let fact = rig.provider().get(&"demo".into()).await.unwrap();
        assert_eq!(fact.captures.len(), 1);
        assert!(!fact.findings.is_empty());
        assert!(!serde_json::to_string(&fact)
            .unwrap()
            .contains("SECRET_ERROR"));
    }

    #[tokio::test]
    async fn archive_memory_and_response_are_bounded_without_hiding_partial_coverage() {
        let rig = Rig::new();
        let capture = collect(&rig.context, &rig.teardown).await;
        for index in 0..MAX_CAPTURES + 5 {
            let mut row = capture.clone();
            row.captured_at += chrono::Duration::seconds(index as i64);
            row.accesses[0].at += chrono::Duration::seconds(index as i64);
            row.accesses = vec![row.accesses[0].clone(); 1000];
            row.id = capture_identity(&rig.context.instance, &rig.context.scope, &row);
            save(&rig.context, &rig.teardown, &row).unwrap();
        }
        let archive = archived(&rig.root, "inst", "demo").unwrap();
        assert_eq!(archive.captures.len(), MAX_CAPTURES);
        assert_eq!(
            archive.captures[0].captured_at,
            capture.captured_at + chrono::Duration::seconds((MAX_CAPTURES + 4) as i64)
        );
        let report = rig.provider().get(&"demo".into()).await.unwrap();
        assert_eq!(
            report
                .captures
                .iter()
                .map(|row| row.accesses.len())
                .sum::<usize>(),
            MAX_ACCESSES
        );
        assert!(report
            .findings
            .iter()
            .any(|row| row.contains("capture bound")));
        assert!(report
            .findings
            .iter()
            .any(|row| row.contains("access bound")));
        assert!(report.captures.iter().all(|row| row.partial));
        assert!(archived(&rig.root, "inst", "other")
            .unwrap()
            .captures
            .is_empty());
    }

    #[tokio::test]
    async fn symlinked_log_parent_cannot_redirect_evidence_writes_or_reads() {
        let rig = Rig::new();
        let capture = collect(&rig.context, &rig.teardown).await;
        let outside = rig.root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, rig.root.join(".factory/logs")).unwrap();
        assert!(save(&rig.context, &rig.teardown, &capture).is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        let fact = archived(&rig.root, "inst", "demo").unwrap();
        assert!(fact.captures.is_empty() && !fact.findings.is_empty());
    }

    #[tokio::test]
    async fn final_capture_drains_late_records_without_losing_the_earlier_window() {
        let rig = Rig::new();
        let context = rig.context.clone();
        let teardown = rig.teardown.clone();
        let final_read = tokio::spawn(async move { collect_final(&context, &teardown).await });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(rig.root.join("calls"))
            .unwrap_or_default()
            .contains("log-read")
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "first log window was not read"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        std::fs::write(rig.root.join("logs"), "[1791116839.109] [sandbox] [OCSF ] [ocsf] NET:REFUSE [MED] DENIED /usr/bin/curl(0) -> 198.18.0.2:1/tcp [policy:bypass engine:nftables]\n").unwrap();
        let capture = final_read.await.unwrap();
        assert_eq!(capture.accesses.len(), 4);
        assert!(capture
            .accesses
            .iter()
            .any(|row| row.target == "198.18.0.2:1"));
        assert!(capture
            .accesses
            .iter()
            .any(|row| row.target == "api.github.com:443"));
        save(&rig.context, &rig.teardown, &capture).unwrap();
        assert_eq!(
            archived(&rig.root, "inst", "demo").unwrap().captures[0].accesses,
            capture.accesses
        );
    }

    #[test]
    fn destination_only_decisions_keep_unknown_process_and_do_not_infer_dns_names() {
        let rows = parse("[1791116839.108] [sandbox] [OCSF ] [ocsf] NET:OPEN [MED] DENIED host.openshell.internal:18958 [reason:policy generation is stale]\n\
            [1791116839.109] [sandbox] [OCSF ] [ocsf] NET:REFUSE [MED] DENIED /usr/bin/curl(0) -> 198.18.0.2:1/tcp [policy:bypass engine:nftables]\n\
            [1791116839.131] [sandbox] [OCSF ] [ocsf] NET:REFUSE [MED] DENIED factory-52abbb0c [reason:policy_dns_ineligible]\n");
        assert_eq!(rows.len(), 2);
        assert!(rows[0].process.is_none());
        assert_eq!(rows[0].target, "host.openshell.internal:18958");
        assert_eq!(rows[1].target, "198.18.0.2:1");
        assert!(rows
            .iter()
            .all(|row| row.disposition == AccessDisposition::Denied));
    }
}
