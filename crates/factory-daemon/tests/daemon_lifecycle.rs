//! Integration coverage for the installation lock and the socket bind order,
//! run as genuine concurrency (two `Daemon`s alive on separate threads at the
//! same time), per the acceptance standard: a second daemon started after
//! the first has already exited proves nothing about real contention.
//!
//! Every test here uses a fresh `tempfile::TempDir` as the instance root —
//! never the real `.factory/` directory — and every socket this file binds
//! lives inside that temp directory.

use std::sync::mpsc;
use std::time::Duration;

use factory_daemon::{Daemon, DaemonError, LockError};

const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// Start a daemon on a background thread and block it there (holding both
/// the installation lock and the socket) until the test tells it to stop.
/// Returns once the daemon has actually bound, so the caller never races a
/// second `Daemon::start` against one that is merely "about to" exist.
fn start_background_daemon(
    instance_root: std::path::PathBuf,
) -> (
    std::thread::JoinHandle<()>,
    mpsc::Sender<()>,
    std::path::PathBuf, // socket_path, reported back before the daemon is moved into the thread
) {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();

    let handle = std::thread::spawn(move || {
        let daemon = Daemon::start(&instance_root).expect("background daemon must start");
        ready_tx
            .send(daemon.socket_path().to_path_buf())
            .expect("test thread must still be waiting for readiness");
        // Hold the daemon (lock + listener) alive until told to stop. Do not
        // actually serve connections here — these tests are about the lock
        // and the socket file, not the request/response loop.
        let _ = stop_rx.recv();
        drop(daemon);
    });

    let socket_path = ready_rx
        .recv_timeout(READY_TIMEOUT)
        .expect("background daemon did not report ready in time");

    (handle, stop_tx, socket_path)
}

/// Guard 1 (crate docs decision 4 / ADR 0012 decision 3): the exclusive
/// installation lock. A second daemon must fail to start while a first one
/// is genuinely still running — not merely "was running a moment ago".
///
/// Mutation this catches: removing the `try_write` exclusivity check inside
/// `InstallationLock::acquire` (e.g. making it always succeed). If that
/// happened, `second` below would be `Ok`, and the first assertion fails.
#[test]
fn second_daemon_fails_to_start_while_first_is_running() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let (handle, stop_tx, _socket_path) = start_background_daemon(root.clone());

    let second = Daemon::start(&root);
    assert!(
        matches!(
            second,
            Err(DaemonError::Lock(LockError::AlreadyLocked { .. }))
        ),
        "a second daemon must fail with AlreadyLocked while the first is running, got {second:?}"
    );

    stop_tx.send(()).unwrap();
    handle.join().unwrap();

    // Prove the failure above was real contention, not a permanent break:
    // once the first daemon has genuinely exited, starting again succeeds.
    let third = Daemon::start(&root);
    assert!(
        third.is_ok(),
        "the instance must be startable again once the first daemon exits: {third:?}"
    );
}

/// Guard 2 (crate docs decision 4): "acquire the lock first, and only then
/// may the socket file be replaced." A failed second start must never touch
/// the first daemon's live socket file.
///
/// Mutation this catches: reordering `Daemon::start` to bind the socket
/// before acquiring the lock (or dropping the requirement that
/// `socket::bind` receive a held `InstallationLock`). Either would let the
/// second attempt's `socket::bind` unlink and recreate the first daemon's
/// socket file before its own lock acquisition failed — which the inode
/// comparison below detects even though the second `Daemon::start` call
/// still ultimately returns `Err`.
#[test]
fn live_daemon_socket_survives_a_failed_second_start() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let (handle, stop_tx, socket_path) = start_background_daemon(root.clone());

    let identity_before =
        factory_paths::FileId::of(&socket_path).expect("the first daemon's socket file must exist");

    let second = Daemon::start(&root);
    assert!(second.is_err(), "second start should fail: {second:?}");

    let identity_after = factory_paths::FileId::of(&socket_path)
        .expect("the first daemon's socket file must still exist after a failed second start");
    assert_eq!(
        identity_before, identity_after,
        "a failed second start must not unlink and recreate the first daemon's live socket file"
    );

    // And the first daemon's listener is still genuinely reachable, not just
    // a file that happens to still be there.
    assert!(
        std::os::unix::net::UnixStream::connect(&socket_path).is_ok(),
        "the first daemon's socket must still be accepting connections"
    );

    stop_tx.send(()).unwrap();
    handle.join().unwrap();
}

/// Guard 3: a socket file left behind by a daemon that was killed (not
/// stopped cleanly) must not block a restart — the exclusive lock, not the
/// socket file's mere existence, is what decides whether replacing it is
/// safe.
#[test]
fn stale_socket_file_does_not_block_restart() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let socket_path = factory_daemon::socket_path(&root);
    std::fs::create_dir_all(socket_path.parent().unwrap()).unwrap();

    // Simulate a killed daemon: bind, then drop without unlinking — exactly
    // what `std::os::unix::net::UnixListener` does on `Drop`, and exactly
    // what a `kill -9` leaves behind. No lock is held by anyone here.
    {
        let leftover = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        drop(leftover);
    }
    assert!(socket_path.exists(), "test setup sanity check");
    assert!(
        std::os::unix::net::UnixStream::connect(&socket_path).is_err(),
        "test setup sanity check: the leftover socket must be genuinely dead"
    );

    let daemon = Daemon::start(&root);
    assert!(
        daemon.is_ok(),
        "a stale socket file with no lock holder must not block a restart: {daemon:?}"
    );
}

/// A clean stop (lock and socket both released) also allows a normal
/// restart — the ordinary, non-crash case.
#[test]
fn restart_after_a_clean_stop_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let first = Daemon::start(&root).unwrap();
    let socket_path = first.socket_path().to_path_buf();
    drop(first);

    assert!(
        socket_path.exists(),
        "std does not unlink a UnixListener's socket file on Drop, so the file is expected to remain"
    );

    let second = Daemon::start(&root);
    assert!(second.is_ok(), "{second:?}");
}

/// A socket path too long for `AF_UNIX` is refused with the length, the
/// limit, and something to do about it — and refused *before* anything on
/// disk is touched.
///
/// Found by running the real `factory` binary from a deeply nested scratch
/// directory: the operator saw only "path must be shorter than SUN_LEN",
/// which names neither the path nor the limit. The nesting that produced it
/// was ordinary — a per-session temporary directory.
#[test]
fn a_socket_path_too_long_for_the_platform_is_refused_with_an_actionable_error() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Nest until `<root>/.factory/factory.sock` is past the limit.
    let mut root = dir.path().to_path_buf();
    while factory_daemon::socket_path(&root)
        .as_os_str()
        .as_encoded_bytes()
        .len()
        <= 103
    {
        root = root.join("nested-directory-name");
    }
    std::fs::create_dir_all(&root).expect("create the nested root");

    let err = factory_daemon::Daemon::start(&root).expect_err("this path cannot be bound");
    let rendered = err.to_string();

    assert!(
        rendered.contains("103"),
        "the message must name the platform's limit: {rendered}"
    );
    assert!(
        rendered.contains("shorter path"),
        "the message must say what an operator can do: {rendered}"
    );
    assert!(
        !factory_daemon::socket_path(&root).exists(),
        "a path that cannot be bound must not leave a socket file behind"
    );
}
