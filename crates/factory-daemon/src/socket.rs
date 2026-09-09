//! Binding `.factory/factory.sock`, and the one rule that protects a live
//! daemon's address: nothing here runs before the caller proves it holds the
//! installation lock.
//!
//! A daemon that was killed (not stopped cleanly) leaves its socket file
//! behind — `std::os::unix::net::UnixListener` does not unlink on `Drop`, and
//! neither does a `kill -9` give it the chance to. A restart must still be
//! able to bind that path, so [`bind`] removes whatever is there first. But
//! blindly removing a socket file is only safe when nothing live could be
//! listening on it, and the *only* way to know that is the installation
//! lock: if another daemon held it, [`crate::InstallationLock::acquire`]
//! would have failed and this function would never be called. That is why
//! `bind` takes `&InstallationLock` as a required parameter it never reads —
//! the type system, not a runtime check, is what enforces the order.

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use crate::lock::InstallationLock;

/// Bind the daemon's socket at `socket_path`, replacing a stale file left
/// behind by a prior daemon that did not exit cleanly.
///
/// `_lock` is not read; its presence in the signature is the guarantee. A
/// caller cannot construct an `InstallationLock` without successfully taking
/// the exclusive lock first, so a call to `bind` is proof, at the type level,
/// that no live daemon can own the file this is about to remove.
/// The longest socket path `AF_UNIX` accepts here.
///
/// `sockaddr_un.sun_path` is 104 bytes on macOS including its terminator, so
/// 103 bytes of path bind and 104 do not — measured directly rather than read
/// off a header, because the value differs across platforms (Linux allows
/// 107).
const MAX_SOCKET_PATH_BYTES: usize = 103;

pub(crate) fn bind(
    _lock: &InstallationLock,
    socket_path: impl AsRef<Path>,
) -> Result<UnixListener, SocketError> {
    let socket_path = socket_path.as_ref();

    // Checked before anything is created or removed: a path that cannot be
    // bound must not first cause a live daemon's socket file to be deleted.
    let length = socket_path.as_os_str().as_encoded_bytes().len();
    if length > MAX_SOCKET_PATH_BYTES {
        return Err(SocketError::PathTooLong {
            path: socket_path.to_path_buf(),
            length,
            limit: MAX_SOCKET_PATH_BYTES,
        });
    }

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SocketError::Io {
            path: socket_path.to_path_buf(),
            source,
        })?;
    }

    match std::fs::remove_file(socket_path) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(SocketError::StaleFileNotRemovable {
                path: socket_path.to_path_buf(),
                source,
            });
        }
    }

    UnixListener::bind(socket_path).map_err(|source| SocketError::Bind {
        path: socket_path.to_path_buf(),
        source,
    })
}

/// Everything that can go wrong binding the daemon's socket.
#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    #[error("cannot remove the stale socket file at {path}: {source}")]
    StaleFileNotRemovable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The socket path exceeds what `AF_UNIX` accepts.
    ///
    /// Measured on this platform on 2026-09-09: 103 bytes bind, 104 do not.
    /// The limit is `sun_path`'s size in `sockaddr_un`, not a Factory choice,
    /// and it is small enough to hit in ordinary use — a Factory instance
    /// under a deeply nested directory (a temp dir, a sandbox path) reaches
    /// it easily. Without this the operator sees only rusqlite's passthrough
    /// "path must be shorter than SUN_LEN", which names neither the path nor
    /// the limit nor anything to do about it.
    #[error(
        "the daemon socket path is {length} bytes, and this platform accepts at most {limit}: \
         {path}\n  help: the limit is the operating system's, not Factory's — move the instance \
         to a shorter path, or reach it through a shorter symlink"
    )]
    PathTooLong {
        path: PathBuf,
        length: usize,
        limit: usize,
    },

    #[error("cannot bind the factory daemon socket at {path}: {source}")]
    Bind {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot use {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::InstallationLock;

    #[test]
    fn binds_a_fresh_socket_when_nothing_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let lock = InstallationLock::acquire(dir.path().join("factory.lock")).unwrap();
        let socket_path = dir.path().join("factory.sock");

        let listener = bind(&lock, &socket_path);
        assert!(listener.is_ok(), "{listener:?}");
        assert!(socket_path.exists());
    }

    #[test]
    fn replaces_a_stale_socket_file_left_by_a_killed_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("factory.sock");

        // Simulate a daemon that was killed: bind, then drop without
        // unlinking (exactly what std does, and exactly what a `kill -9`
        // leaves behind).
        {
            let leftover = UnixListener::bind(&socket_path).unwrap();
            drop(leftover);
        }
        assert!(socket_path.exists());
        assert!(
            std::os::unix::net::UnixStream::connect(&socket_path).is_err(),
            "the leftover socket must be genuinely dead: nothing should be listening on it"
        );

        let lock = InstallationLock::acquire(dir.path().join("factory.lock")).unwrap();
        let listener = bind(&lock, &socket_path);
        assert!(
            listener.is_ok(),
            "a stale socket file must not block a restart: {listener:?}"
        );
    }
}
