//! The full `Adapter` lifecycle — `start`, `send`, `observe`, `interrupt`,
//! `stop` — against a **real Herdr and a real `pi` process**.
//!
//! Marked `#[ignore]` on purpose: it needs a running Herdr and a working `pi`
//! install, and it takes as long as a model takes to answer. It creates its
//! own Herdr workspace via `Adapter::start` and closes it via `Adapter::stop`
//! before returning; it never touches a pane it did not create.
//!
//! ```text
//! cargo test -p factory-adapter --test live_pi -- --ignored --nocapture
//! ```
//!
//! Two rules learned the hard way elsewhere in this codebase (see
//! `factory-e2e/tests/live_agent.rs`), both honoured here:
//!
//! - The token this drill asks the agent to echo is derived from the clock at
//!   run time and asserted on directly — a fixed word would pass on a
//!   *previous* run's answer still sitting in the pane's scrollback.
//! - Submission is confirmed through `send`'s own `--wait --until working`
//!   path, and completion through a real `herdr agent wait --until done`,
//!   never by matching a pre-prompt idle state. A wall-clock floor at the end
//!   guards against a drill that "passes" without ever having waited on a
//!   model.

use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use factory_adapter::{Adapter, HerdrCli, PiAdapter, StartRequest};
use factory_paths::CanonicalPath;

#[test]
#[ignore = "needs a running Herdr and a working `pi` install; creates and closes its own workspace"]
fn full_lifecycle_against_a_real_pi_session() {
    let adapter = PiAdapter::new(HerdrCli::new("herdr"));

    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = CanonicalPath::resolve(dir.path()).expect("resolve workspace");

    let session_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-00000000f101").expect("uuid");
    let scope_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-00000000f102").expect("uuid");
    let task_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-00000000f103").expect("uuid");

    let clock = Instant::now();

    // 1. start: creates a fresh Herdr workspace/pane and starts `pi` in it.
    let started = adapter
        .start(&StartRequest {
            scope_id,
            session_id,
            workspace,
            generated_context: String::new(),
            model: None,
        })
        .expect("start must succeed against a real Herdr with `pi` installed");
    println!(
        "STARTED pane={:?} confidence={:?} harness_session_id={:?}",
        started.pane, started.confidence, started.harness_session_id
    );

    // A token unique to this run.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let token = format!("ACK-{:x}", nanos & 0xffff_ffff_ffff);
    println!("TOKEN {token}");

    // 2. send: submits and confirms the harness started working — never a
    // fire-and-forget write to the PTY.
    adapter
        .send(
            &started.pane,
            task_id,
            &format!("Reply with exactly the word {token} and nothing else."),
        )
        .expect("send must confirm the harness started working on this prompt");

    // `send` only confirms *submission*; wait for the turn itself to finish
    // before reading the answer back.
    let waited = Command::new("herdr")
        .args([
            "agent",
            "wait",
            &started.pane.0,
            "--until",
            "done",
            "--until",
            "idle",
            "--timeout",
            "300000",
        ])
        .output()
        .expect("herdr agent wait");
    assert!(waited.status.success(), "the agent never finished its turn");

    // 3. observe: reads the state back through the adapter, not the raw CLI.
    let after = adapter
        .observe(&started.pane)
        .expect("observe must succeed after the turn finished");
    println!(
        "AFTER status={:?} signal={:?}",
        after.harness_state, after.task_signal
    );
    assert_ne!(
        after.harness_state, "working",
        "the agent must have finished before its result is checked"
    );

    // A drill that goes green in under a second did not run.
    assert!(
        clock.elapsed().as_secs() >= 1,
        "this drill finished suspiciously fast ({:?}) — it likely never waited on a real model",
        clock.elapsed()
    );

    let transcript = Command::new("herdr")
        .args(["agent", "read", &started.pane.0])
        .output()
        .expect("herdr agent read");
    let screen = String::from_utf8_lossy(&transcript.stdout);
    assert!(
        screen.contains(&token),
        "the agent's answer to *this* run's task must be on the pane; looked for {token}"
    );

    // 4. interrupt: succeeds even against a session that has already gone
    // idle — it must not require an active turn to be meaningful to call.
    adapter
        .interrupt(&started.pane)
        .expect("interrupt must succeed against a live session");

    // 5. stop: closes the pane, and Herdr closes the workspace along with
    // its last pane (measured: closing the only pane in a freshly created
    // workspace leaves `herdr workspace close` reporting `workspace_not_found`
    // immediately afterward) — so this alone is this drill's whole teardown.
    adapter
        .stop(&started.pane)
        .expect("stop must succeed against a live session");

    let gone = adapter.observe(&started.pane).expect(
        "observe must still answer for a pane whose session was stopped, even if only to \
                 say it is gone",
    );
    assert!(
        !gone.session_alive,
        "a stopped session must no longer be reported alive"
    );
}
