//! The installation lock: an exclusive `flock` on `.factory/factory.lock`,
//! held for the daemon's lifetime (ADR 0012 decision 3, ADR 0014
//! consequences). A second daemon fails to start rather than quietly
//! competing with the first over the same SQLite database and socket.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// An exclusive hold on the installation lock file, released when dropped.
///
/// This is the *only* proof [`crate::Daemon::start`] (via the private
/// `socket` module) accepts that no other daemon can be holding
/// `.factory/factory.sock`: binding the socket requires a `&InstallationLock`
/// argument, which can only be produced by [`InstallationLock::acquire`]
/// succeeding first.
#[derive(Debug)]
pub struct InstallationLock {
    // `fd_lock::RwLockWriteGuard` borrows the `RwLock` it locks, which makes
    // holding both in one struct self-referential. `unsafe_code` is forbidden
    // in this workspace, so the usual self-referential-struct tricks (a raw
    // pointer, `ouroboros`) are not available here. Leaking the `RwLock` onto
    // a `'static` reference sidesteps the problem safely: a daemon holds this
    // lock for its entire lifetime by design, so the one allocation (and the
    // one file descriptor inside it) had nowhere to be freed to anyway.
    //
    // The honest limit of that argument: the leak happens before `try_write`,
    // so a *failed* acquisition leaks too. For the daemon that is one
    // allocation on the path to exiting with an error. For a test that
    // deliberately contends the lock it is one per attempt, which is bounded
    // and was measured as such rather than assumed. Removing the leak means
    // giving the caller the `RwLock` to own and handing `acquire` a `&mut` to
    // it — worth doing if anything ever acquires this in a loop.
    // Never read: it is held purely for its `Drop` effect (releasing the
    // `flock`) when this `InstallationLock` goes out of scope.
    #[allow(dead_code)]
    guard: fd_lock::RwLockWriteGuard<'static, File>,
    path: PathBuf,
}

impl InstallationLock {
    /// Acquire the exclusive lock at `lock_path`, creating the file if
    /// needed. Fails immediately — never blocks — if another daemon already
    /// holds it.
    pub fn acquire(lock_path: impl AsRef<Path>) -> Result<Self, LockError> {
        let path = lock_path.as_ref().to_path_buf();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| LockError::Io {
                path: path.clone(),
                source,
            })?;
        }

        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| LockError::Io {
                path: path.clone(),
                source,
            })?;

        let rw_lock: &'static mut fd_lock::RwLock<File> =
            Box::leak(Box::new(fd_lock::RwLock::new(file)));

        match rw_lock.try_write() {
            Ok(mut guard) => {
                record_holder_pid(&mut guard).map_err(|source| LockError::Io {
                    path: path.clone(),
                    source,
                })?;
                Ok(Self { guard, path })
            }
            Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                Err(LockError::AlreadyLocked {
                    holder_pid: read_holder_pid(&path),
                    path,
                })
            }
            Err(source) => Err(LockError::Io { path, source }),
        }
    }

    /// The lock file this hold is on.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Overwrite the lock file with this process's PID, so a daemon that fails to
/// acquire the lock can name which process holds it in its error message.
/// Best-effort in the sense that a contender reads this file *without* a
/// lock of its own (the file is exclusively locked by us at this point), so a
/// concurrent reader can observe a partial write; that reader treats an
/// unparseable result as "unknown holder" rather than erroring.
fn record_holder_pid(file: &mut File) -> std::io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    write!(file, "{}", std::process::id())?;
    file.flush()
}

fn read_holder_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Everything that can go wrong acquiring the installation lock.
#[derive(Debug)]
pub enum LockError {
    /// Another daemon holds the lock right now.
    AlreadyLocked {
        path: PathBuf,
        /// The PID the current holder recorded, when readable. `None` means
        /// either the holder hasn't written it yet (a narrow race right after
        /// it acquired the lock) or the file could not be read at all.
        holder_pid: Option<u32>,
    },
    /// The lock file itself could not be created, opened, or written.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::AlreadyLocked { path, holder_pid } => {
                let path = path.display();
                match holder_pid {
                    Some(pid) => write!(
                        f,
                        "another factory daemon (pid {pid}) already holds the installation lock at {path}\n  \
                         help: check whether pid {pid} is still running (`ps -p {pid}`); if it is, this instance \
                         already has a daemon and none of its state should be touched by hand. If it is not, the \
                         lock releases automatically — nothing to delete — and starting again will succeed"
                    ),
                    None => write!(
                        f,
                        "another factory daemon already holds the installation lock at {path}\n  \
                         help: its pid could not be read from the lock file; find it independently (e.g. `lsof {path}`) \
                         before assuming it is safe to act. If it has exited, the lock releases automatically and \
                         starting again will succeed"
                    ),
                }
            }
            LockError::Io { path, source } => {
                write!(f, "cannot use lock file {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for LockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LockError::Io { source, .. } => Some(source),
            LockError::AlreadyLocked { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_creates_missing_parent_directories_and_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("factory.lock");

        let lock = InstallationLock::acquire(&path).unwrap();
        assert_eq!(lock.path(), path);
        assert!(path.exists());
    }

    #[test]
    fn second_acquire_fails_while_the_first_is_still_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("factory.lock");

        let first = InstallationLock::acquire(&path).unwrap();

        let second = InstallationLock::acquire(&path);
        match second {
            Err(LockError::AlreadyLocked { holder_pid, .. }) => {
                assert_eq!(
                    holder_pid,
                    Some(std::process::id()),
                    "the holder pid recorded in the lock file should be this process's own pid"
                );
            }
            other => panic!("expected AlreadyLocked, got {other:?}"),
        }

        drop(first);
    }

    #[test]
    fn lock_releases_on_drop_and_can_be_reacquired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("factory.lock");

        let first = InstallationLock::acquire(&path).unwrap();
        assert!(InstallationLock::acquire(&path).is_err());
        drop(first);

        let third = InstallationLock::acquire(&path);
        assert!(
            third.is_ok(),
            "lock must be reacquirable once the prior holder drops it: {third:?}"
        );
    }

    #[test]
    fn already_locked_display_names_the_holder_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("factory.lock");
        let _first = InstallationLock::acquire(&path).unwrap();

        let err = InstallationLock::acquire(&path).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains(&std::process::id().to_string()),
            "error message should name the holder's pid: {message}"
        );
        assert!(
            message.contains("help:"),
            "message should be actionable: {message}"
        );
    }
}
