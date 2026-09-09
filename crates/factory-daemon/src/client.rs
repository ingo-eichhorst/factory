//! The client half of the socket ADR 0014 requires: connect, send one
//! request, read one response, and turn a `factory.error/v1` envelope into a
//! typed Rust error.
//!
//! [`call`] takes a raw socket path rather than an instance root
//! deliberately — see [`crate::socket_path`] for the one place that layout
//! is decided — so a test can point it at a fake server that never went
//! through [`crate::Daemon::start`].

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::envelope::{self, ErrorBody, Request, Response, SuccessResponse};

/// Connect to the daemon at `socket_path`, send `request`, and return its
/// result — or a typed reason it could not be obtained.
pub fn call(
    socket_path: impl AsRef<Path>,
    request: &Request,
) -> Result<SuccessResponse, ClientError> {
    let socket_path = socket_path.as_ref();

    let stream =
        UnixStream::connect(socket_path).map_err(|source| classify_connect(socket_path, source))?;

    let mut writer = stream.try_clone().map_err(ClientError::Io)?;
    // `Request` is our own well-typed enum of plain JSON-representable
    // fields (UUIDs bridged through `uuid_str`, `payload` already a
    // `Value`); there is no input here that can make serialization fail.
    let line = serde_json::to_string(request)
        .expect("Request always serializes: no field can produce a serde error");
    writer.write_all(line.as_bytes()).map_err(ClientError::Io)?;
    writer.write_all(b"\n").map_err(ClientError::Io)?;
    writer.flush().map_err(ClientError::Io)?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    let read = reader
        .read_line(&mut response_line)
        .map_err(ClientError::Io)?;
    if read == 0 {
        return Err(ClientError::Protocol(
            "the daemon closed the connection without sending a response".to_string(),
        ));
    }

    let response = envelope::parse_response(&response_line).map_err(ClientError::Protocol)?;

    match response {
        Response::Success(success) => {
            if success.request_id != request.request_id() {
                return Err(ClientError::Protocol(format!(
                    "response request_id {} does not match the request's request_id {}",
                    success.request_id,
                    request.request_id()
                )));
            }
            Ok(success)
        }
        Response::Error(error_envelope) => Err(ClientError::Remote(RemoteError::from_wire(
            error_envelope.error,
        ))),
    }
}

/// `ConnectionRefused` covers a stale socket *file* left behind by a killed
/// daemon (see the `socket` module docs) as much as `NotFound` covers no
/// file at all — both mean the same thing to a caller: there is no daemon to
/// talk to right now.
fn classify_connect(socket_path: &Path, source: std::io::Error) -> ClientError {
    match source.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
            ClientError::NotRunning(socket_path.to_path_buf())
        }
        _ => ClientError::Io(source),
    }
}

/// Everything that can go wrong calling the daemon.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(
        "the factory daemon is not running: no listener at {0}\n  \
         help: start the daemon before retrying this command"
    )]
    NotRunning(PathBuf),

    #[error("i/o error talking to the factory daemon: {0}")]
    Io(#[source] std::io::Error),

    #[error("malformed response from the factory daemon: {0}")]
    Protocol(String),

    #[error(transparent)]
    Remote(#[from] RemoteError),
}

/// A `factory.error/v1` envelope, turned into a typed Rust error.
///
/// `code` and `message` and `details` are the wire body verbatim; `family`
/// is derived from `code`'s leading dotted segment (ADR 0003 §5's code
/// families) so a caller can match on it without parsing the string itself.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct RemoteError {
    pub code: String,
    pub family: ErrorFamily,
    pub message: String,
    pub retryable: bool,
    pub details: Value,
}

impl RemoteError {
    fn from_wire(body: ErrorBody) -> Self {
        Self {
            family: ErrorFamily::from_code(&body.code),
            code: body.code,
            message: body.message,
            retryable: body.retryable,
            details: body.details,
        }
    }
}

/// ADR 0003 §5's initial error code families, plus `Other` for a code this
/// crate does not recognize (a forward-compatible daemon must not make an
/// older client panic on a new family).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorFamily {
    Validation,
    Authentication,
    Authorization,
    NotFound,
    Conflict,
    Unavailable,
    Internal,
    Other,
}

impl ErrorFamily {
    fn from_code(code: &str) -> Self {
        match code.split('.').next().unwrap_or_default() {
            "validation" => Self::Validation,
            "authentication" => Self::Authentication,
            "authorization" => Self::Authorization,
            "not_found" => Self::NotFound,
            "conflict" => Self::Conflict,
            "unavailable" => Self::Unavailable,
            "internal" => Self::Internal,
            _ => Self::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{CommandRequest, ErrorEnvelope, QueryRequest};
    use std::os::unix::net::UnixListener;
    use uuid::Uuid;

    fn uuid(seed: u8) -> Uuid {
        Uuid::from_bytes([seed; 16])
    }

    fn sample_command() -> Request {
        Request::Command(CommandRequest {
            request_id: uuid(1),
            scope_id: uuid(2),
            command: "task.create".to_string(),
            payload: serde_json::json!({}),
            expected_revision: None,
        })
    }

    #[test]
    fn daemon_not_running_when_no_socket_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("factory.sock");

        let err = call(&socket_path, &sample_command()).unwrap_err();
        assert!(
            matches!(err, ClientError::NotRunning(ref path) if path == &socket_path),
            "expected NotRunning({socket_path:?}), got {err:?}"
        );
    }

    #[test]
    fn daemon_not_running_when_socket_file_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("factory.sock");
        {
            let leftover = UnixListener::bind(&socket_path).unwrap();
            drop(leftover);
        }

        let err = call(&socket_path, &sample_command()).unwrap_err();
        assert!(
            matches!(err, ClientError::NotRunning(_)),
            "a stale socket file must be reported the same way as no daemon: {err:?}"
        );
    }

    /// A minimal fake server: accept one connection, read one line, discard
    /// it, and write back `response_line`. Lets the client be tested against
    /// exact wire bytes without a real [`crate::Daemon`] or [`Handler`].
    fn fake_server_once(response_line: String) -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let dir = Box::leak(Box::new(dir));
        let socket_path = dir.path().join("factory.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                let _ = writeln!(stream, "{response_line}");
            }
        });

        socket_path
    }

    #[test]
    fn client_surfaces_the_remote_errors_code_and_family_distinctly_per_family() {
        // Two different families from two different fake servers: a client
        // that ignored `error.code` and reported a constant family would
        // pass a test that checked only one of these.
        let conflict_envelope = Response::Error(ErrorEnvelope {
            request_id: Some(uuid(1)),
            error: ErrorBody {
                code: "conflict.revision_mismatch".to_string(),
                message: "the task changed after it was read".to_string(),
                retryable: false,
                details: serde_json::json!({"expected_revision": 7, "actual_revision": 8}),
            },
        });
        let socket_path = fake_server_once(serde_json::to_string(&conflict_envelope).unwrap());
        let err = call(&socket_path, &sample_command()).unwrap_err();
        match err {
            ClientError::Remote(remote) => {
                assert_eq!(remote.code, "conflict.revision_mismatch");
                assert_eq!(remote.family, ErrorFamily::Conflict);
            }
            other => panic!("expected ClientError::Remote, got {other:?}"),
        }

        let not_found_envelope = Response::Error(ErrorEnvelope {
            request_id: Some(uuid(1)),
            error: ErrorBody {
                code: "not_found.task".to_string(),
                message: "no such task".to_string(),
                retryable: false,
                details: serde_json::json!({}),
            },
        });
        let socket_path = fake_server_once(serde_json::to_string(&not_found_envelope).unwrap());
        let err = call(&socket_path, &sample_command()).unwrap_err();
        match err {
            ClientError::Remote(remote) => {
                assert_eq!(remote.code, "not_found.task");
                assert_eq!(remote.family, ErrorFamily::NotFound);
            }
            other => panic!("expected ClientError::Remote, got {other:?}"),
        }
    }

    #[test]
    fn unrecognized_error_family_becomes_other_rather_than_panicking() {
        let envelope = Response::Error(ErrorEnvelope {
            request_id: Some(uuid(1)),
            error: ErrorBody {
                code: "something_new.surprising".to_string(),
                message: "a future daemon speaks a code this client does not know".to_string(),
                retryable: true,
                details: serde_json::json!({}),
            },
        });
        let socket_path = fake_server_once(serde_json::to_string(&envelope).unwrap());
        let err = call(&socket_path, &sample_command()).unwrap_err();
        match err {
            ClientError::Remote(remote) => {
                assert_eq!(remote.family, ErrorFamily::Other);
                assert!(remote.retryable);
            }
            other => panic!("expected ClientError::Remote, got {other:?}"),
        }
    }

    #[test]
    fn response_request_id_mismatch_is_a_protocol_error() {
        let response = Response::Success(SuccessResponse {
            request_id: uuid(99), // does not match sample_command()'s request_id
            event_cursor: 1,
            result: serde_json::json!({}),
        });
        let socket_path = fake_server_once(serde_json::to_string(&response).unwrap());
        let err = call(&socket_path, &sample_command()).unwrap_err();
        assert!(
            matches!(err, ClientError::Protocol(_)),
            "a response for a different request_id must not be accepted silently: {err:?}"
        );
    }

    #[test]
    fn malformed_response_bytes_are_a_protocol_error_not_a_panic() {
        let socket_path = fake_server_once("not json at all".to_string());
        let err = call(&socket_path, &sample_command()).unwrap_err();
        assert!(matches!(err, ClientError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn query_request_id_accessor_matches_the_request() {
        let request = Request::Query(QueryRequest {
            request_id: uuid(5),
            scope_id: uuid(6),
            query: "task.get".to_string(),
            payload: serde_json::json!({}),
        });
        assert_eq!(request.request_id(), uuid(5));
    }
}
