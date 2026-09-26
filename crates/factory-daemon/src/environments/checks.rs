//! Running one health check, bounded (`#185`).
//!
//! Every check ends in a [`Sample`], whatever happens: a check that hangs is
//! killed at its timeout with its whole process group, one that cannot start
//! says why, and the caller runs each in a task of its own, so a panic is a
//! failed sample rather than a dead loop. Nothing a check does can take the
//! daemon down -- the same rules `harness_health::run_probe` keeps.
//!
//! `http` asks `curl`, found on the daemon's `PATH`: the daemon carries no
//! HTTP client, and the environments worth checking are behind TLS.

use chrono::Utc;
use factory_core::environments::{CheckDecl, CheckKind, Sample};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// How much of a body is kept for an `http` check's `body` match.
const BODY_LIMIT: usize = 64 * 1024;

/// Run `check` of `environment` once. `env_url` is the environment's own
/// `url`, `dir` the scope's directory, where a `command` check runs.
pub async fn run(environment: &str, env_url: Option<&str>, dir: &Path, check: &CheckDecl) -> Sample {
    let started = Instant::now();
    let timeout = Duration::from_secs(check.timeout_seconds());
    let outcome = match check.kind {
        CheckKind::Http => http(check, env_url, timeout).await,
        CheckKind::Tcp => tcp(check, timeout).await,
        CheckKind::Command => command(check, dir, timeout).await,
    };
    let (ok, detail) = match outcome {
        Ok(detail) => (true, detail),
        Err(detail) => (false, Some(detail)),
    };
    Sample {
        environment: environment.to_string(),
        check: check.display_name(),
        at: Utc::now(),
        ok,
        latency_ms: started.elapsed().as_millis() as u64,
        detail,
    }
}

async fn http(check: &CheckDecl, env_url: Option<&str>, timeout: Duration) -> Result<Option<String>, String> {
    let url = check.http_url(env_url).ok_or("no URL to ask")?;
    let curl = crate::harness_health::resolve("curl").ok_or("curl is not on the daemon's PATH")?;
    let expect = check.expect.unwrap_or(200);
    let args = vec![
        "-sS".to_string(),
        "-o".into(),
        "-".into(),
        "-w".into(),
        "\n%{http_code}".into(),
        "--max-time".into(),
        timeout.as_secs().to_string(),
        url,
    ];
    // curl keeps its own deadline; ours is the backstop for a curl that
    // does not.
    let out = bounded(curl, args, None, timeout + Duration::from_secs(2)).await?;
    let text = out.stdout.clone();
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", text.as_ref()));
    let code: u16 = code.trim().parse().unwrap_or(0);
    if !out.success || code == 0 {
        return Err(first_line(&out.stderr).unwrap_or_else(|| "no answer".to_string()));
    }
    if code != expect {
        return Err(format!("expected {expect}, got {code}"));
    }
    if let Some(want) = &check.body {
        let body: String = body.chars().take(BODY_LIMIT).collect();
        if !body.contains(want.as_str()) {
            return Err(format!("{code}, but the body does not contain {want:?}"));
        }
    }
    Ok(Some(code.to_string()))
}

async fn tcp(check: &CheckDecl, timeout: Duration) -> Result<Option<String>, String> {
    let host = check.host.clone().unwrap_or_default();
    let port = check.port.unwrap_or_default();
    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host.as_str(), port))).await {
        Ok(Ok(_)) => Ok(Some("connected".into())),
        Ok(Err(e)) => Err(format!("{host}:{port}: {e}")),
        Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
    }
}

async fn command(check: &CheckDecl, dir: &Path, timeout: Duration) -> Result<Option<String>, String> {
    let script = check.command.clone().unwrap_or_default();
    let out = bounded(PathBuf::from("/bin/sh"), vec!["-c".into(), script], Some(dir), timeout).await?;
    if out.success {
        Ok(first_line(&out.stdout))
    } else {
        let said = first_line(&out.stderr).or_else(|| first_line(&out.stdout));
        Err(match said {
            Some(said) => format!("{}: {said}", out.status),
            None => out.status,
        })
    }
}

struct Output {
    success: bool,
    status: String,
    stdout: String,
    stderr: String,
}

/// Run a program with no stdin in its own process group, and kill the
/// group at `timeout`.
async fn bounded(program: PathBuf, args: Vec<String>, dir: Option<&Path>, timeout: Duration) -> Result<Output, String> {
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    #[cfg(unix)]
    cmd.process_group(0);
    let child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", program.display()))?;
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => Ok(Output {
            success: out.status.success(),
            status: match out.status.code() {
                Some(code) => format!("exit {code}"),
                None => "killed by a signal".into(),
            },
            stdout: String::from_utf8_lossy(&out.stdout[..out.stdout.len().min(BODY_LIMIT + 16)]).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr[..out.stderr.len().min(4096)]).into_owned(),
        }),
        Ok(Err(e)) => Err(format!("{}: {e}", program.display())),
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
                // SAFETY: killpg with a pgid we created and a constant signal.
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            #[cfg(not(unix))]
            let _ = pid;
            Err(format!("timed out after {}s", timeout.as_secs()))
        }
    }
}

fn first_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| l.chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(yaml: &str) -> CheckDecl {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    #[tokio::test]
    async fn a_command_that_exits_zero_is_healthy_and_one_that_does_not_says_why() {
        let dir = std::env::temp_dir();
        let ok = run("e", None, &dir, &check("{ kind: command, command: 'echo fine' }")).await;
        assert!(ok.ok);
        assert_eq!(ok.detail.as_deref(), Some("fine"));
        assert_eq!(ok.check, "echo fine");
        let bad = run("e", None, &dir, &check("{ kind: command, command: 'echo broken >&2; exit 3' }")).await;
        assert!(!bad.ok);
        assert_eq!(bad.detail.as_deref(), Some("exit 3: broken"));
    }

    #[tokio::test]
    async fn a_check_that_hangs_is_killed_at_its_timeout_and_is_a_failed_sample() {
        let started = Instant::now();
        let hung = run(
            "e",
            None,
            &std::env::temp_dir(),
            &check("{ kind: command, command: 'sleep 30', timeout: 1s, every: 5s }"),
        )
        .await;
        assert!(!hung.ok);
        assert_eq!(hung.detail.as_deref(), Some("timed out after 1s"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn tcp_connects_or_says_it_could_not() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let up = run("e", None, Path::new("/"), &check(&format!("{{ kind: tcp, host: 127.0.0.1, port: {port} }}"))).await;
        assert!(up.ok, "{:?}", up.detail);

        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let down = run("e", None, Path::new("/"), &check(&format!("{{ kind: tcp, host: 127.0.0.1, port: {port} }}"))).await;
        assert!(!down.ok, "{:?}", down.detail);
    }

    /// A tiny HTTP server answering one fixed response, so `http` is
    /// exercised end to end through curl.
    async fn serve(response: &'static str) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(response.as_bytes()).await;
            }
        });
        port
    }

    #[tokio::test]
    async fn http_checks_the_status_and_the_body() {
        let port = serve("HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}").await;
        let url = format!("http://127.0.0.1:{port}");
        let ok = run("e", Some(&url), Path::new("/"), &check("{ kind: http, path: /api/status, body: '\"ok\"' }")).await;
        assert!(ok.ok, "{:?}", ok.detail);
        assert_eq!(ok.detail.as_deref(), Some("200"));
        let wrong = run("e", Some(&url), Path::new("/"), &check("{ kind: http, path: /, expect: 204 }")).await;
        assert_eq!(wrong.detail.as_deref(), Some("expected 204, got 200"));
        let missing = run("e", Some(&url), Path::new("/"), &check("{ kind: http, path: /, body: nope }")).await;
        assert!(!missing.ok);

        let broken = serve("HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        let bad = run("e", Some(&format!("http://127.0.0.1:{broken}")), Path::new("/"), &check("{ kind: http, path: / }")).await;
        assert_eq!(bad.detail.as_deref(), Some("expected 200, got 502"));

        // Nothing listening: curl's own words.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let closed = listener.local_addr().unwrap().port();
        drop(listener);
        let gone = run("e", Some(&format!("http://127.0.0.1:{closed}")), Path::new("/"), &check("{ kind: http, path: / }")).await;
        assert!(!gone.ok);
        assert!(gone.detail.unwrap().contains("curl"), "curl says why");
    }
}
