//! A thin client for the daemon. One request, one response -- over the unix
//! socket (one JSON line each way), or, for an agent that cannot reach the
//! socket at all, over the daemon's `http` interface (`POST /api/rpc`).
//!
//! The HTTP path exists for a run inside an OpenShell sandbox (`#218`): the
//! agent runs on Linux inside a VM, where `.factory/factory.sock` on the host
//! is not a path at all, but the sandbox's network policy can let one binary
//! reach the daemon's http port as `host.openshell.internal`. Both transports
//! send the very same `Envelope` -- request and token -- to the same
//! `Engine::handle`, so roles and grants apply exactly as they do over the
//! socket; nothing here decides anything about who may do what.

use anyhow::{anyhow, bail, Context, Result};
use factory_core::config::Factory;
use factory_core::protocol::{Envelope, Payload, Request, Response};
use std::fmt;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UnixStream};

/// Where the daemon is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Socket(PathBuf),
    Http(HttpUrl),
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(path) => write!(f, "{}", path.display()),
            Self::Http(url) => write!(f, "{url}"),
        }
    }
}

/// A plain `http://host:port` the daemon's http interface answers on. Only
/// plain HTTP: the interface itself serves nothing else, and a sandbox's
/// proxy relays a plain connection to the host untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpUrl {
    pub host: String,
    pub port: u16,
}

impl fmt::Display for HttpUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "http://{}:{}", self.host, self.port)
    }
}

impl HttpUrl {
    /// `http://host[:port][/]`. Anything else -- `https://`, a path, a
    /// query, credentials in the authority -- is refused rather than
    /// guessed at: the token travels in the body, and a URL that looked
    /// like it carried one somewhere else would be a lie about where it went.
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        let rest = raw
            .strip_prefix("http://")
            .ok_or_else(|| anyhow!("FACTORY_URL/--url must be a plain http:// address, got {raw:?}"))?;
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
            bail!("FACTORY_URL/--url must be http://host:port with nothing after it, got {raw:?}");
        }
        let (host, port) = match authority.rsplit_once(':') {
            // An IPv6 literal is bracketed; its own colons are not a port.
            Some((host, port)) if !port.contains(']') => {
                let port: u16 = port
                    .parse()
                    .map_err(|_| anyhow!("FACTORY_URL/--url has a port that is not a number: {raw:?}"))?;
                (host.to_string(), port)
            }
            _ => (authority.to_string(), 80),
        };
        if host.is_empty() {
            bail!("FACTORY_URL/--url names no host: {raw:?}");
        }
        Ok(Self { host, port })
    }

    fn host_header(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    fn connect_target(&self) -> (String, u16) {
        (
            self.host.trim_start_matches('[').trim_end_matches(']').to_string(),
            self.port,
        )
    }
}

pub struct Client {
    pub endpoint: Endpoint,
    /// Which agent is calling. Absent means the person at the keyboard.
    pub token: Option<String>,
}

impl Client {
    /// `--socket`, then `--url`, then `FACTORY_URL`, then `FACTORY_SOCKET`,
    /// then the nearest instance's own socket.
    ///
    /// `FACTORY_URL` beats `FACTORY_SOCKET` because the only session that
    /// sets it is one that cannot use the socket -- a sandboxed run, where a
    /// socket path inherited from the host would name nothing.
    pub fn locate(
        explicit: Option<PathBuf>,
        url: Option<String>,
        root: Option<PathBuf>,
        token: Option<String>,
    ) -> Result<Self> {
        // Factory sets FACTORY_TOKEN in every session it opens, so an agent
        // identifies itself without being told to.
        let token = token
            .or_else(|| std::env::var("FACTORY_TOKEN").ok())
            .or_else(|| std::env::var("FACTORY_TASK_TOKEN").ok())
            .filter(|t| !t.is_empty());
        let env_url = std::env::var("FACTORY_URL").ok().filter(|u| !u.is_empty());
        let env_socket = std::env::var("FACTORY_SOCKET").ok().filter(|s| !s.is_empty());
        let endpoint = Self::choose(explicit, url, env_url, env_socket, || {
            let root = match root {
                Some(r) => r,
                None => Factory::discover(&std::env::current_dir()?)?.ok_or_else(|| {
                    anyhow!("no .factory/ here or in any parent, and no --socket or --url given")
                })?,
            };
            Ok(Factory::load(&root)?.socket_path())
        })?;
        Ok(Self { endpoint, token })
    }

    /// The precedence `locate` documents, without touching the environment
    /// or the disk, so it can be tested on its own.
    fn choose(
        explicit: Option<PathBuf>,
        url: Option<String>,
        env_url: Option<String>,
        env_socket: Option<String>,
        discover: impl FnOnce() -> Result<PathBuf>,
    ) -> Result<Endpoint> {
        if let Some(p) = explicit {
            return Ok(Endpoint::Socket(p));
        }
        if let Some(u) = url.or(env_url) {
            return Ok(Endpoint::Http(HttpUrl::parse(&u)?));
        }
        if let Some(p) = env_socket {
            return Ok(Endpoint::Socket(PathBuf::from(p)));
        }
        Ok(Endpoint::Socket(discover()?))
    }

    async fn connect(&self, socket: &Path) -> Result<UnixStream> {
        UnixStream::connect(socket).await.with_context(|| {
            format!(
                "no daemon on {}. Start one with `factory-daemon run`.",
                socket.display()
            )
        })
    }

    pub async fn send(&self, request: Request) -> Result<Payload> {
        let envelope = Envelope {
            request,
            token: self.token.clone(),
        };
        let response = match &self.endpoint {
            Endpoint::Socket(socket) => self.send_socket(socket, &envelope).await?,
            Endpoint::Http(url) => send_http(url, &envelope).await?,
        };
        match response {
            Response::Ok { data } => Ok(data),
            Response::Error { code, message } => Err(anyhow!("{message} [{code}]")),
        }
    }

    async fn send_socket(&self, socket: &Path, envelope: &Envelope) -> Result<Response> {
        let stream = self.connect(socket).await?;
        let (read_half, mut write_half) = stream.into_split();
        let line = format!("{}\n", serde_json::to_string(envelope)?);
        write_half.write_all(line.as_bytes()).await?;
        write_half.flush().await?;

        let mut lines = BufReader::new(read_half).lines();
        let reply = lines
            .next_line()
            .await?
            .ok_or_else(|| anyhow!("the daemon closed the connection without answering"))?;
        Ok(serde_json::from_str::<Response>(&reply)?)
    }

    /// Stream events until the connection drops or `on_event` says to stop.
    /// Socket only: the http interface streams over a WebSocket, which this
    /// client does not speak, and nothing a sandboxed run does needs it.
    pub async fn subscribe<F>(&self, mut on_event: F) -> Result<()>
    where
        F: FnMut(Response) -> bool,
    {
        let socket = match &self.endpoint {
            Endpoint::Socket(socket) => socket,
            Endpoint::Http(url) => bail!(
                "watching events needs the control socket; {url} is the http interface, which this client only sends single requests to"
            ),
        };
        let stream = self.connect(socket).await?;
        let (read_half, mut write_half) = stream.into_split();
        let line = format!(
            "{}\n",
            serde_json::to_string(&Envelope {
                request: Request::Subscribe,
                token: self.token.clone(),
            })?
        );
        write_half.write_all(line.as_bytes()).await?;
        write_half.flush().await?;

        let mut lines = BufReader::new(read_half).lines();
        // The first line is the acknowledgement, not an event.
        let _ = lines.next_line().await?;
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let msg: Response = serde_json::from_str(&line)?;
            if !on_event(msg) {
                break;
            }
        }
        Ok(())
    }

    /// Where requests go, for a status line to name.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

/// One `POST /api/rpc`, HTTP/1.1 with `Connection: close`, read to the end.
/// Hand-rolled rather than a dependency: the CLI is cross-built as a static
/// Linux binary for sandbox images, and this is the entire HTTP it needs.
async fn send_http(url: &HttpUrl, envelope: &Envelope) -> Result<Response> {
    let body = serde_json::to_vec(envelope)?;
    let mut stream = TcpStream::connect(url.connect_target())
        .await
        .with_context(|| format!("no daemon answering on {url} (its http interface)"))?;
    let head = format!(
        "POST /api/rpc HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\nUser-Agent: factory-cli\r\n\r\n",
        url.host_header(),
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .with_context(|| format!("reading the daemon's answer from {url}"))?;
    parse_http_response(&raw).with_context(|| format!("the answer from {url}"))
}

/// The daemon's answer out of a raw HTTP/1.1 response. Every status the
/// interface sends for a protocol answer -- 200, 400, 403, 404, 500 -- carries
/// a `Response` body, so the body is what decides; a body that is not one
/// (a proxy's own refusal, say) is reported with its status line.
fn parse_http_response(raw: &[u8]) -> Result<Response> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("the connection closed before a complete HTTP response arrived"))?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut body = raw[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default().to_string();
    let mut chunked = false;
    let mut length: Option<usize> = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if name.eq_ignore_ascii_case("transfer-encoding") && value.eq_ignore_ascii_case("chunked") {
            chunked = true;
        } else if name.eq_ignore_ascii_case("content-length") {
            length = value.parse().ok();
        }
    }
    if chunked {
        body = dechunk(&body)?;
    } else if let Some(n) = length {
        body.truncate(n);
    }
    match serde_json::from_slice::<Response>(&body) {
        Ok(response) => Ok(response),
        Err(_) => {
            let text = String::from_utf8_lossy(&body);
            let shown: String = text.trim().chars().take(200).collect();
            Err(anyhow!("{status_line}: {shown}"))
        }
    }
}

fn dechunk(mut body: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| anyhow!("a chunked body ended mid-chunk"))?;
        let size_text = String::from_utf8_lossy(&body[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or_default().trim().to_string();
        let size = usize::from_str_radix(&size_text, 16)
            .map_err(|_| anyhow!("a chunked body has a bad chunk size {size_text:?}"))?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if body.len() < size {
            bail!("a chunked body ended mid-chunk");
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size..];
        body = body.strip_prefix(b"\r\n").unwrap_or(body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn a_url_parses_to_host_and_port_and_refuses_anything_it_would_have_to_guess_at() {
        assert_eq!(
            HttpUrl::parse("http://host.openshell.internal:8787").unwrap(),
            HttpUrl { host: "host.openshell.internal".into(), port: 8787 }
        );
        assert_eq!(HttpUrl::parse("http://127.0.0.1:9/").unwrap().port, 9);
        assert_eq!(HttpUrl::parse("http://daemon").unwrap().port, 80);
        assert_eq!(HttpUrl::parse("http://[::1]:8787").unwrap().connect_target(), ("::1".to_string(), 8787));
        for bad in [
            "https://host:1",
            "host:1",
            "http://host:1/api/rpc",
            "http://user:pw@host:1",
            "http://host:port",
            "http://",
        ] {
            assert!(HttpUrl::parse(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn the_socket_flag_wins_then_the_url_then_factory_url_then_factory_socket_then_discovery() {
        let never = || -> Result<PathBuf> { panic!("discovery must not run") };
        let url = Some("http://h:1".to_string());
        assert_eq!(
            Client::choose(Some("/s".into()), url.clone(), url.clone(), Some("/e".into()), never).unwrap(),
            Endpoint::Socket("/s".into())
        );
        assert_eq!(
            Client::choose(None, Some("http://flag:2".into()), url.clone(), Some("/e".into()), never).unwrap(),
            Endpoint::Http(HttpUrl { host: "flag".into(), port: 2 })
        );
        // A sandboxed run carries FACTORY_URL; an inherited FACTORY_SOCKET
        // would name a path that does not exist in there.
        assert_eq!(
            Client::choose(None, None, url, Some("/e".into()), never).unwrap(),
            Endpoint::Http(HttpUrl { host: "h".into(), port: 1 })
        );
        assert_eq!(
            Client::choose(None, None, None, Some("/e".into()), never).unwrap(),
            Endpoint::Socket("/e".into())
        );
        assert_eq!(
            Client::choose(None, None, None, None, || Ok(PathBuf::from("/found"))).unwrap(),
            Endpoint::Socket("/found".into())
        );
    }

    /// A one-shot fake daemon: accepts one connection, hands the raw
    /// request to the test, and answers with `reply`.
    async fn fake_daemon(reply: String) -> (HttpUrl, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            // Read the head, then exactly Content-Length bytes of body.
            loop {
                let n = stream.read(&mut buf).await.unwrap();
                seen.extend_from_slice(&buf[..n]);
                if let Some(split) = seen.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&seen[..split]).to_string();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if seen.len() >= split + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            stream.write_all(reply.as_bytes()).await.unwrap();
            stream.shutdown().await.ok();
            String::from_utf8_lossy(&seen).to_string()
        });
        (HttpUrl { host: "127.0.0.1".into(), port }, handle)
    }

    fn reply(body: &str, status: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    const OK: &str = r#"{"status":"ok","data":{"kind":"ok"}}"#;

    fn body_of(seen: &str) -> serde_json::Value {
        serde_json::from_str(&seen[seen.find("\r\n\r\n").unwrap() + 4..]).unwrap()
    }

    #[tokio::test]
    async fn a_report_goes_over_http_as_the_same_envelope_the_socket_carries() {
        let (url, seen) = fake_daemon(reply(OK, "200 OK")).await;
        let client = Client { endpoint: Endpoint::Http(url), token: Some("run-token".into()) };
        let report = serde_json::from_value(serde_json::json!({"status": "done", "result": "ok", "token": "run-token"})).unwrap();
        let payload = client.send(Request::TaskReport { id: "t1".into(), report }).await;
        let seen = seen.await.unwrap();
        assert!(seen.starts_with("POST /api/rpc HTTP/1.1\r\n"), "{seen}");
        let envelope = body_of(&seen);
        // Byte for byte what the socket would have carried.
        let socket_line = serde_json::to_value(Envelope {
            request: Request::TaskReport {
                id: "t1".into(),
                report: serde_json::from_value(serde_json::json!({"status": "done", "result": "ok", "token": "run-token"})).unwrap(),
            },
            token: Some("run-token".into()),
        })
        .unwrap();
        assert_eq!(envelope, socket_line);
        assert_eq!(envelope["op"], "task.report", "{envelope}");
        assert!(matches!(payload, Ok(Payload::Ok)), "{payload:?}");
    }

    #[tokio::test]
    async fn turn_ended_goes_over_http_with_the_run_token() {
        let (url, seen) = fake_daemon(reply(OK, "200 OK")).await;
        let client = Client { endpoint: Endpoint::Http(url), token: None };
        let turn = serde_json::from_value(serde_json::json!({"event": "stop", "token": "run-token"})).unwrap();
        client.send(Request::TaskTurnEnded { id: "t1".into(), turn }).await.unwrap();
        let envelope = body_of(&seen.await.unwrap());
        assert_eq!(envelope["op"], "task.turn_ended", "{envelope}");
        assert_eq!(envelope["params"]["turn"]["token"], "run-token", "{envelope}");
    }

    #[tokio::test]
    async fn a_refusal_over_http_is_the_daemons_own_error_not_the_status_code() {
        let (url, _) = fake_daemon(reply(
            r#"{"status":"error","code":"denied","message":"the run token does not match"}"#,
            "403 Forbidden",
        ))
        .await;
        let client = Client { endpoint: Endpoint::Http(url), token: Some("stale".into()) };
        let e = client.send(Request::Status).await.unwrap_err().to_string();
        assert!(e.contains("the run token does not match") && e.contains("[denied]"), "{e}");
    }

    #[tokio::test]
    async fn something_that_is_not_the_daemon_is_reported_with_its_status_line() {
        let (url, _) = fake_daemon(reply("blocked by policy", "403 Forbidden")).await;
        let client = Client { endpoint: Endpoint::Http(url), token: None };
        let e = format!("{:#}", client.send(Request::Status).await.unwrap_err());
        assert!(e.contains("403 Forbidden") && e.contains("blocked by policy"), "{e}");
    }

    #[tokio::test]
    async fn nobody_listening_says_which_url_it_tried() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let client = Client { endpoint: Endpoint::Http(HttpUrl { host: "127.0.0.1".into(), port }), token: None };
        let e = format!("{:#}", client.send(Request::Status).await.unwrap_err());
        assert!(e.contains(&format!("http://127.0.0.1:{port}")), "{e}");
    }

    #[test]
    fn a_chunked_answer_is_reassembled() {
        let raw = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n9\r\n{\"status\"\r\n1b\r\n:\"ok\",\"data\":{\"kind\":\"ok\"}}\r\n0\r\n\r\n";
        assert!(matches!(parse_http_response(raw).unwrap(), Response::Ok { .. }));
    }

    #[tokio::test]
    async fn watching_over_http_is_refused_plainly() {
        let client = Client { endpoint: Endpoint::Http(HttpUrl::parse("http://h:1").unwrap()), token: None };
        let e = client.subscribe(|_| true).await.unwrap_err().to_string();
        assert!(e.contains("control socket"), "{e}");
    }
}
