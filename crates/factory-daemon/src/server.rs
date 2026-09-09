//! The blocking Unix-domain-socket server (crate docs decision 3): blocking
//! `accept`, one thread per connection, no async runtime.
//!
//! [`Daemon::start`] is the only place the installation lock and the socket
//! bind meet, and the order between them is load-bearing — see the `lock`
//! and `socket` module docs. [`Daemon::serve`] is the transport loop: it
//! reads one newline-delimited JSON request, dispatches it to a
//! caller-supplied [`Handler`], and writes back one newline-delimited JSON
//! response, repeating until the connection closes.
//!
//! This module defines `Handler` and never implements it. Station 10 builds
//! the transport, not the supervisor loop that will eventually dispatch
//! commands into the store, the registry, or an adapter — see the crate
//! docs' decision 8.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::envelope::{self, CommandRequest, ErrorBody, QueryRequest, Request};
use crate::lock::{InstallationLock, LockError};
use crate::socket::{self, SocketError};

/// The productive half of a [`Handler`] call: what ADR 0003 §4 calls the
/// result and the event cursor it was read at (or committed through, for a
/// command).
#[derive(Debug, Clone)]
pub struct HandlerSuccess {
    pub event_cursor: u64,
    pub result: Value,
}

/// What a [`Handler`] returns for one command or query.
pub type HandlerOutcome = Result<HandlerSuccess, ErrorBody>;

/// Dispatches one parsed request to whatever owns domain state.
///
/// This crate never implements `Handler` itself; it only defines the trait
/// [`Daemon::serve`] dispatches through. A caller — the daemon binary, once
/// it exists, or a test — supplies the real one.
pub trait Handler: Send + Sync {
    fn handle_command(&self, request: CommandRequest) -> HandlerOutcome;
    fn handle_query(&self, request: QueryRequest) -> HandlerOutcome;
}

/// A bound daemon: the installation lock is held and the socket is
/// listening. Dropping this releases both, in that order (fields drop in
/// declaration order — `listener` first, then `lock` — which is also the
/// harmless order: the socket file being briefly unremovable while a new
/// daemon starts is not a correctness problem the way a socket steal is).
#[derive(Debug)]
pub struct Daemon {
    listener: UnixListener,
    lock: InstallationLock,
    socket_path: PathBuf,
}

impl Daemon {
    /// Take the installation lock, then bind `.factory/factory.sock` beneath
    /// `instance_root`, creating `.factory/` if needed.
    ///
    /// The order between the two calls below is the crate docs' decision 4:
    /// [`InstallationLock::acquire`] runs first and short-circuits this
    /// function on failure via `?`, so [`socket::bind`] is never reached
    /// unless this call is the only daemon. `socket::bind` also cannot be
    /// named without an [`InstallationLock`] value already in hand — see its
    /// module docs — so the ordering is enforced twice over: once by this
    /// function's control flow, once by the type system.
    pub fn start(instance_root: impl AsRef<Path>) -> Result<Self, DaemonError> {
        let instance_root = instance_root.as_ref();
        let lock = InstallationLock::acquire(crate::lock_path(instance_root))?;
        let socket_path = crate::socket_path(instance_root);
        let listener = socket::bind(&lock, &socket_path)?;

        Ok(Self {
            listener,
            lock,
            socket_path,
        })
    }

    /// The socket this daemon is listening on.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The installation lock file this daemon holds.
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        self.lock.path()
    }

    /// Accept connections forever, one thread per connection (crate docs
    /// decision 3). Returns only if `accept` itself fails unrecoverably; a
    /// failure handling one connection never stops the loop.
    pub fn serve(&self, handler: Arc<dyn Handler>) -> std::io::Result<()> {
        loop {
            let (stream, _address) = self.listener.accept()?;
            let handler = Arc::clone(&handler);
            std::thread::spawn(move || {
                let _ = serve_connection(&stream, handler.as_ref());
            });
        }
    }
}

/// Serve one connection: read newline-delimited requests and write back
/// newline-delimited responses until the peer closes it.
fn serve_connection(stream: &UnixStream, handler: &dyn Handler) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(()); // peer closed the connection
        }

        let response_line = dispatch_line(&line, handler);
        writer.write_all(response_line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
}

/// Parse one request line, dispatch it if it parsed, and serialize the
/// resulting envelope back to one line. This never fails outward: a request
/// that cannot be parsed becomes an error envelope, not a dropped
/// connection.
fn dispatch_line(line: &str, handler: &dyn Handler) -> String {
    let response = match envelope::parse_request(line) {
        Ok(Request::Command(request)) => {
            respond(request.request_id, || handler.handle_command(request))
        }
        Ok(Request::Query(request)) => {
            respond(request.request_id, || handler.handle_query(request))
        }
        Err(failure) => envelope::Response::Error(envelope::ErrorEnvelope {
            request_id: failure.request_id,
            error: ErrorBody {
                code: failure.code,
                message: failure.message,
                retryable: false,
                details: serde_json::json!({}),
            },
        }),
    };

    // `Response`'s only free-form field is `result`/`details`, both already
    // `serde_json::Value` — there is no input here that can make encoding a
    // `Response` we built ourselves fail (mirrors the same reasoning in
    // `client::call` for encoding a `Request`).
    serde_json::to_string(&response)
        .expect("Response always serializes: no field can produce a serde error")
}

fn respond(request_id: uuid::Uuid, call: impl FnOnce() -> HandlerOutcome) -> envelope::Response {
    match call() {
        Ok(success) => envelope::Response::Success(envelope::SuccessResponse {
            request_id,
            event_cursor: success.event_cursor,
            result: success.result,
        }),
        Err(error) => envelope::Response::Error(envelope::ErrorEnvelope {
            request_id: Some(request_id),
            error,
        }),
    }
}

/// Everything that can go wrong starting the daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Socket(#[from] SocketError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Response;
    struct EchoHandler;

    impl Handler for EchoHandler {
        fn handle_command(&self, request: CommandRequest) -> HandlerOutcome {
            if request.command == "test.fail" {
                return Err(ErrorBody {
                    code: "conflict.revision_mismatch".to_string(),
                    message: "the task changed after it was read".to_string(),
                    retryable: false,
                    details: serde_json::json!({}),
                });
            }
            Ok(HandlerSuccess {
                event_cursor: 1,
                result: request.payload,
            })
        }

        fn handle_query(&self, request: QueryRequest) -> HandlerOutcome {
            if request.query == "test.not_found" {
                return Err(ErrorBody {
                    code: "not_found.task".to_string(),
                    message: "no such task".to_string(),
                    retryable: false,
                    details: serde_json::json!({}),
                });
            }
            Ok(HandlerSuccess {
                event_cursor: 2,
                result: request.payload,
            })
        }
    }

    fn spawn_test_daemon() -> (Daemon, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        // Leak the tempdir so its contents outlive the test's use of the
        // socket path; the OS cleans /tmp eventually and the daemon test
        // process is short-lived regardless.
        let dir = Box::leak(Box::new(dir));
        let daemon = Daemon::start(dir.path()).unwrap();
        let socket_path = daemon.socket_path().to_path_buf();
        (daemon, socket_path)
    }

    #[test]
    fn command_round_trip_returns_the_handlers_result() {
        let (daemon, socket_path) = spawn_test_daemon();
        std::thread::spawn(move || {
            let _ = daemon.serve(Arc::new(EchoHandler));
        });

        let request = Request::Command(CommandRequest {
            request_id: uuid::Uuid::from_bytes([1; 16]),
            scope_id: uuid::Uuid::from_bytes([2; 16]),
            command: "task.create".to_string(),
            payload: serde_json::json!({"title": "hello"}),
            expected_revision: None,
        });

        let response = crate::client::call(&socket_path, &request).unwrap();
        assert_eq!(response.request_id, uuid::Uuid::from_bytes([1; 16]));
        assert_eq!(response.event_cursor, 1);
        assert_eq!(response.result, serde_json::json!({"title": "hello"}));
    }

    #[test]
    fn one_connection_carries_multiple_sequential_requests() {
        let (daemon, socket_path) = spawn_test_daemon();
        std::thread::spawn(move || {
            let _ = daemon.serve(Arc::new(EchoHandler));
        });

        let mut stream = UnixStream::connect(&socket_path).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());

        for n in 0..3u8 {
            let request = Request::Query(QueryRequest {
                request_id: uuid::Uuid::from_bytes([n; 16]),
                scope_id: uuid::Uuid::from_bytes([9; 16]),
                query: "task.get".to_string(),
                payload: serde_json::json!({"n": n}),
            });
            let line = serde_json::to_string(&request).unwrap();
            writeln!(stream, "{line}").unwrap();

            let mut response_line = String::new();
            reader.read_line(&mut response_line).unwrap();
            let response = envelope::parse_response(&response_line).unwrap();
            match response {
                Response::Success(success) => {
                    assert_eq!(success.request_id, uuid::Uuid::from_bytes([n; 16]));
                    assert_eq!(success.result, serde_json::json!({"n": n}));
                }
                Response::Error(error) => panic!("unexpected error: {error:?}"),
            }
        }
    }

    #[test]
    fn malformed_line_gets_a_validation_error_without_killing_the_connection() {
        let (daemon, socket_path) = spawn_test_daemon();
        std::thread::spawn(move || {
            let _ = daemon.serve(Arc::new(EchoHandler));
        });

        let mut stream = UnixStream::connect(&socket_path).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());

        writeln!(stream, "not json").unwrap();
        let mut response_line = String::new();
        reader.read_line(&mut response_line).unwrap();
        let response = envelope::parse_response(&response_line).unwrap();
        match response {
            Response::Error(error) => {
                assert_eq!(error.error.code, "validation.malformed_json");
                assert_eq!(error.request_id, None);
            }
            Response::Success(success) => panic!("expected an error, got {success:?}"),
        }

        // The connection must still be usable for a well-formed request.
        let request = Request::Query(QueryRequest {
            request_id: uuid::Uuid::from_bytes([7; 16]),
            scope_id: uuid::Uuid::from_bytes([8; 16]),
            query: "task.get".to_string(),
            payload: serde_json::json!({}),
        });
        writeln!(stream, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        response_line.clear();
        reader.read_line(&mut response_line).unwrap();
        let response = envelope::parse_response(&response_line).unwrap();
        assert!(matches!(response, Response::Success(_)));
    }

    #[test]
    fn handler_error_codes_surface_with_distinct_families_on_the_client() {
        let (daemon, socket_path) = spawn_test_daemon();
        std::thread::spawn(move || {
            let _ = daemon.serve(Arc::new(EchoHandler));
        });

        let conflict_request = Request::Command(CommandRequest {
            request_id: uuid::Uuid::from_bytes([1; 16]),
            scope_id: uuid::Uuid::from_bytes([2; 16]),
            command: "test.fail".to_string(),
            payload: serde_json::json!({}),
            expected_revision: None,
        });
        let conflict_err = crate::client::call(&socket_path, &conflict_request).unwrap_err();
        match conflict_err {
            crate::client::ClientError::Remote(remote) => {
                assert_eq!(remote.code, "conflict.revision_mismatch");
                assert_eq!(remote.family, crate::client::ErrorFamily::Conflict);
            }
            other => panic!("expected a remote error, got {other:?}"),
        }

        let not_found_request = Request::Query(QueryRequest {
            request_id: uuid::Uuid::from_bytes([3; 16]),
            scope_id: uuid::Uuid::from_bytes([4; 16]),
            query: "test.not_found".to_string(),
            payload: serde_json::json!({}),
        });
        let not_found_err = crate::client::call(&socket_path, &not_found_request).unwrap_err();
        match not_found_err {
            crate::client::ClientError::Remote(remote) => {
                assert_eq!(remote.code, "not_found.task");
                assert_eq!(remote.family, crate::client::ErrorFamily::NotFound);
            }
            other => panic!("expected a remote error, got {other:?}"),
        }
    }
}
