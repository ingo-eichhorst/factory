//! The HTTP transport (ADR 0002 decision 2's second transport, ADR 0023):
//! the same [`crate::Handler`] the Unix socket serves, reachable from a
//! browser, plus the one page that uses it.
//!
//! This module adds no protocol of its own. `POST /api` carries exactly ADR
//! 0003's envelope — the same JSON line the socket accepts — and answers with
//! exactly ADR 0003's response envelope, because it hands the body straight to
//! [`crate::server::dispatch_line`]. A request that works against the socket
//! works here byte for byte, which is what keeps a second transport from
//! becoming a second API.
//!
//! # Shape
//!
//! [`crate::server::Daemon::serve`]'s shape, with a different listener:
//! blocking `accept`, one thread per connection, no async runtime. One
//! request per connection (`Connection: close`) — keep-alive would buy
//! nothing for a page that issues a handful of requests every five seconds,
//! and costs a parser that has to agree with the client about framing.
//!
//! # Why it is safe to bind a TCP port at all
//!
//! The socket transport is reachable only by a process that can open a file
//! in `.factory/`. A TCP port is reachable by anything that can make a
//! request to `127.0.0.1` — and on a developer's machine, that includes
//! **every page in their browser**. A local daemon that can start agents and
//! send tasks is exactly the kind of thing a hostile page would like to
//! reach, so three guards stand in front of [`handle_api`], and all three
//! must pass:
//!
//! 1. **The listener binds loopback only.** [`Ui::bind`] refuses any address
//!    whose IP is not a loopback address, so the port cannot be exposed to a
//!    network by configuration accident.
//! 2. **`Host` must name the loopback.** A page can point a DNS name it
//!    controls at `127.0.0.1` (DNS rebinding) and reach this port with its own
//!    origin. The browser sends the attacker's name in `Host`, and this
//!    module refuses anything but `127.0.0.1`, `[::1]` or `localhost` at the
//!    port it bound.
//! 3. **`Origin`, when present, must be this server.** A same-origin `fetch`
//!    from the served page sends no `Origin` on same-origin requests in some
//!    browsers and its own origin in others; a cross-origin one always sends
//!    a foreign origin. Refusing a foreign `Origin` refuses the cross-site
//!    request even when the browser would have sent it.
//!
//! A fourth guard falls out of the content type: `POST /api` requires
//! `application/json`, which is not one of the three types a cross-origin
//! form or `fetch` may send without a preflight. The preflight is an
//! `OPTIONS` this module answers with `405`, so the real request is never
//! made.
//!
//! None of this is authentication, and this module does not pretend
//! otherwise: anything that can already run code as this user can reach the
//! Unix socket too. The guards exist to close the one gap a TCP port opens
//! that the socket did not — *the browser* acting on a hostile page's behalf.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

use crate::server::Handler;

/// The page itself, compiled in. There is no directory to serve and no path
/// to traverse: this transport answers exactly two routes, and one of them is
/// this constant.
const INDEX_HTML: &str = include_str!("../ui/index.html");

/// The environment variable that overrides where — or whether — the UI is
/// served. `off` disables it; anything else is parsed as a socket address.
pub const ADDR_ENV: &str = "FACTORY_UI_ADDR";

/// Where the UI is served when nothing says otherwise.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7373";

/// The largest request body this transport will read. An ADR 0003 envelope is
/// a few hundred bytes; a task prompt can be long, but not this long. The cap
/// exists so a malformed `Content-Length` cannot make this process allocate on
/// a peer's say-so.
const MAX_BODY: usize = 1 << 20;

/// The largest request line or header line. Same reasoning as [`MAX_BODY`].
const MAX_LINE: u64 = 8 * 1024;

/// A bound UI listener.
#[derive(Debug)]
pub struct Ui {
    listener: TcpListener,
    addr: SocketAddr,
}

impl Ui {
    /// Bind `addr`, which must be a loopback address.
    ///
    /// # Errors
    ///
    /// [`UiError::NotLoopback`] if the address is routable, and
    /// [`UiError::Bind`] for whatever the bind itself returned — most often
    /// the port already being in use, which callers are expected to report
    /// and carry on from rather than treat as fatal.
    pub fn bind(addr: SocketAddr) -> Result<Self, UiError> {
        if !is_loopback(addr.ip()) {
            return Err(UiError::NotLoopback(addr));
        }
        let listener = TcpListener::bind(addr).map_err(|source| UiError::Bind { addr, source })?;
        let addr = listener
            .local_addr()
            .map_err(|source| UiError::Bind { addr, source })?;
        Ok(Self { listener, addr })
    }

    /// Resolve the configured address: [`DEFAULT_ADDR`], unless [`ADDR_ENV`]
    /// says otherwise. `Ok(None)` means the operator turned the UI off.
    ///
    /// # Errors
    ///
    /// [`UiError::BadAddr`] when the variable is set to something that is
    /// neither `off` nor a socket address — a typo that silently fell back to
    /// the default would be worse than a refusal to start.
    pub fn configured_addr() -> Result<Option<SocketAddr>, UiError> {
        let raw = match std::env::var(ADDR_ENV) {
            Ok(value) => value,
            Err(_) => DEFAULT_ADDR.to_string(),
        };
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("off") {
            return Ok(None);
        }
        trimmed
            .parse::<SocketAddr>()
            .map(Some)
            .map_err(|_| UiError::BadAddr(raw))
    }

    /// The address actually bound — never the requested one, so that a
    /// caller which asked for port 0 learns the port it got.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Accept connections forever, one thread per connection. Returns only if
    /// `accept` itself fails; a failure serving one connection never stops the
    /// loop, exactly as on the socket transport.
    pub fn serve(&self, handler: Arc<dyn Handler>) -> std::io::Result<()> {
        let port = self.addr.port();
        loop {
            let (stream, _peer) = self.listener.accept()?;
            let handler = Arc::clone(&handler);
            std::thread::spawn(move || {
                let _ = serve_connection(stream, handler.as_ref(), port);
            });
        }
    }

    /// Serve on a thread, so a caller can keep the socket transport on the
    /// main one. The returned handle is dropped by callers that serve until
    /// the process ends.
    pub fn spawn(self, handler: Arc<dyn Handler>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let _ = self.serve(handler);
        })
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Everything that can go wrong binding or configuring the UI listener.
#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error(
        "the UI listener refuses to bind {0}: only a loopback address is allowed\n  help: the \
         daemon serves a page that can start agents; binding it to a routable address would \
         publish that to the network"
    )]
    NotLoopback(SocketAddr),

    #[error("could not bind the UI listener at {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },

    #[error("{ADDR_ENV} is `{0}`, which is neither `off` nor an address like `127.0.0.1:7373`")]
    BadAddr(String),
}

/// One request, then close. See the module docs for why there is no
/// keep-alive.
fn serve_connection(stream: TcpStream, handler: &dyn Handler, port: u16) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    let response = match read_request(&mut reader, port) {
        Ok(request) => route(&request, handler),
        Err(response) => response,
    };

    writer.write_all(&response.render())?;
    writer.flush()
}

/// What a parsed request carries. Only the fields this transport actually
/// decides on: nothing here is kept for a caller that might want it later.
struct Request {
    method: String,
    path: String,
    host_ok: bool,
    origin_ok: bool,
    json: bool,
    body: String,
}

/// Read one line, refusing a line longer than [`MAX_LINE`] rather than
/// growing a buffer on a peer's say-so. Returns the line including its
/// terminator, or an empty string at end of input.
fn read_line_limited(reader: &mut BufReader<TcpStream>) -> std::io::Result<String> {
    let mut line = String::new();
    let mut limited = reader.take(MAX_LINE);
    limited.read_line(&mut line)?;
    Ok(line)
}

fn read_request(reader: &mut BufReader<TcpStream>, port: u16) -> Result<Request, Response> {
    let line =
        read_line_limited(reader).map_err(|_| Response::text(400, "malformed request line"))?;

    let mut parts = line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| Response::text(400, "malformed request line"))?
        .to_string();
    // The request target, with any query string cut off: `/?x=1` and `/#a`
    // are the same page as `/`, and a browser adds both without asking. A
    // router that compared the whole target would 404 a link someone shared.
    let target = parts
        .next()
        .ok_or_else(|| Response::text(400, "malformed request line"))?;
    let path = target
        .split(['?', '#'])
        .next()
        .unwrap_or(target)
        .to_string();

    let mut host_ok = false;
    let mut origin_ok = true;
    let mut json = false;
    let mut length: usize = 0;

    loop {
        let header =
            read_line_limited(reader).map_err(|_| Response::text(400, "malformed header"))?;
        if header.trim().is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        match name.as_str() {
            "host" => host_ok = host_is_loopback(&value, port),
            "origin" => origin_ok = origin_is_self(&value, port),
            "content-type" => {
                json = value.split(';').next().unwrap_or("").trim() == "application/json";
            }
            "content-length" => {
                length = value
                    .parse()
                    .map_err(|_| Response::text(400, "malformed content-length"))?;
                if length > MAX_BODY {
                    return Err(Response::text(413, "request body too large"));
                }
            }
            _ => {}
        }
    }

    let mut body = vec![0u8; length];
    if length > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|_| Response::text(400, "request body shorter than content-length"))?;
    }
    let body =
        String::from_utf8(body).map_err(|_| Response::text(400, "request body is not UTF-8"))?;

    Ok(Request {
        method,
        path,
        host_ok,
        origin_ok,
        json,
        body,
    })
}

/// `Host` must name a loopback at the port this listener bound. See the
/// module docs' guard 2.
fn host_is_loopback(value: &str, port: u16) -> bool {
    let (host, given_port) = match value.rsplit_once(':') {
        // `[::1]:7373` splits correctly; a bare `[::1]` does not, and is
        // handled by the fallback below.
        Some((host, port)) if !host.ends_with('[') => (host, port.parse::<u16>().ok()),
        _ => (value, None),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let name_ok =
        host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok_and(is_loopback);
    name_ok && given_port.is_some_and(|p| p == port)
}

/// `Origin`, when the browser sends one, must be this server. See the module
/// docs' guard 3.
fn origin_is_self(value: &str, port: u16) -> bool {
    if value == "null" {
        return false;
    }
    let Some(rest) = value.strip_prefix("http://") else {
        return false;
    };
    host_is_loopback(rest, port)
}

fn route(request: &Request, handler: &dyn Handler) -> Response {
    if !request.host_ok {
        return Response::text(
            421,
            "this daemon answers only on the loopback address it bound",
        );
    }
    if !request.origin_ok {
        return Response::text(403, "cross-origin requests are refused");
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") | ("HEAD", "/") => Response::html(INDEX_HTML),
        ("POST", "/api") => handle_api(request, handler),
        ("GET", _) | ("HEAD", _) => Response::text(404, "no such page"),
        _ => Response::text(405, "method not allowed"),
    }
}

/// The one endpoint. The body is ADR 0003's request envelope and the answer
/// is ADR 0003's response envelope — this function adds framing and a content
/// type, and decides nothing about the request itself.
fn handle_api(request: &Request, handler: &dyn Handler) -> Response {
    if !request.json {
        return Response::text(415, "POST /api requires `Content-Type: application/json`");
    }
    Response::json(200, crate::server::dispatch_line(&request.body, handler))
}

/// A response, still in pieces.
struct Response {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl Response {
    fn html(body: &str) -> Self {
        Self {
            status: 200,
            content_type: "text/html; charset=utf-8",
            body: body.to_string(),
        }
    }

    fn json(status: u16, body: String) -> Self {
        Self {
            status,
            content_type: "application/json",
            body,
        }
    }

    fn text(status: u16, message: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: format!("{message}\n"),
        }
    }

    fn render(&self) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            421 => "Misdirected Request",
            _ => "Error",
        };
        let mut out = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
             Connection: close\r\n\r\n",
            self.status,
            self.content_type,
            self.body.len()
        )
        .into_bytes();
        out.extend_from_slice(self.body.as_bytes());
        out
    }
}
