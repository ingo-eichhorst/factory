//! Process exit codes this binary uses, beyond the two clap already owns:
//! `0` for `--help`/`--version`, `2` for a usage error clap itself detects
//! (an unknown subcommand, a missing required argument). Everything below is
//! this crate's own, and every one of them is documented so a caller can act
//! on the number alone, not just the message.

/// The command did what it was asked.
pub const OK: i32 = 0;

/// Something failed in a way none of the more specific codes below name —
/// an I/O error, a malformed local file, a bug. The message always says
/// what happened; this code just says "not one of the others."
pub const GENERIC_ERROR: i32 = 1;

/// The daemon is not running. Distinct from [`GENERIC_ERROR`] because this is
/// a routine condition, not a crash — the message always names how to start
/// it (`factory start --root <root>`).
pub const DAEMON_NOT_RUNNING: i32 = 3;

/// The daemon answered but rejected the request (a `factory.error/v1`
/// envelope) or the transport itself misbehaved after connecting. The
/// message carries the daemon's own error code.
pub const REMOTE_ERROR: i32 = 4;

/// `factory doctor` ran to completion and found at least one
/// [`factory_doctor::Finding`] — binding decision 3: a finding means a
/// non-zero exit, so this command is usable from a check.
pub const DOCTOR_FINDINGS: i32 = 5;

/// `factory doctor` could not even open the database (missing or
/// unopenable) — distinct from [`DOCTOR_FINDINGS`] because there is no
/// report to read, only a reason diagnosis itself did not run.
pub const DOCTOR_CANT_DIAGNOSE: i32 = 6;

/// `factory task send --wait` (or a chunked continuation of it) passed its
/// deadline before the task reached a terminal or `blocked` state. The task
/// itself is untouched — waiting is a read (see `factory_daemon`'s own
/// `task.wait` docs) — and the message names the task id so the caller can
/// ask again with `factory task show`.
pub const TASK_WAIT_TIMEOUT: i32 = 7;

/// `factory daemon run` could not bind the socket or take the installation
/// lock — most commonly, another daemon already holds it. The message is
/// [`factory_daemon::DaemonError`]'s own, which already names the holder's
/// pid when it can.
pub const DAEMON_START_FAILED: i32 = 8;

/// `factory stop` signalled the daemon but it was still answering after the
/// bounded wait. Distinct from [`GENERIC_ERROR`] because the operator's next
/// move is specific: check the pid, consider `kill -9`.
pub const STOP_TIMED_OUT: i32 = 9;
