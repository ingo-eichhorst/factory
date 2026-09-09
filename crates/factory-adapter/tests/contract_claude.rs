//! Runs backlog §10's contract suite against [`ClaudeAdapter`] — literally
//! the same `run_contract_suite` `tests/contract_pi.rs` runs against
//! `PiAdapter`, per that suite's own doc comment on why a second, hand
//! written copy of the same assertions is exactly the drift this crate
//! exists not to pay for again.
//!
//! Beyond the shared suite, this file adds three things specific to this
//! adapter:
//!
//! - a Claude-specific cleanup test mirroring `contract_pi.rs`'s own
//!   `a_failed_agent_start_closes_the_pane_start_created`;
//! - the pane-not-cwd join rule, exercised through the fixture rather than
//!   raw JSON (`src/claude.rs`'s own unit tests already cover the raw-JSON
//!   case) — two sessions started in one workspace directory must never be
//!   attributed to each other;
//! - a real, `#[ignore]`d drill against a live Herdr pane and the live
//!   Irrlicht daemon: the only proof in this crate that the measured command
//!   shapes `src/claude.rs` depends on still hold on this machine.

mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use factory_adapter::contract::{ContractFixture, run_contract_suite};
use factory_adapter::{
    Adapter, ClaudeAdapter, HerdrAccess, HerdrCli, IrrlichtAccess, IrrlichtHttp, PaneId,
};
use factory_paths::CanonicalPath;
use support::ClaudeFixture;

#[test]
fn claude_adapter_satisfies_the_full_contract_suite() {
    let fixture = ClaudeFixture::new();
    run_contract_suite(&fixture);
}

#[test]
fn a_failed_agent_start_closes_the_pane_start_created() {
    let fixture = ClaudeFixture::new();
    fixture.fail_next_agent_start();

    let req = fixture.startable();
    fixture
        .adapter()
        .start(&req)
        .expect_err("a fixture configured to fail agent_start must fail start");

    assert_eq!(
        fixture.pane_close_calls().len(),
        1,
        "the pane created before agent_start failed must be closed exactly once, or it leaks"
    );
}

// backlog §10's cwd rule (its 2026-09-09 resolution note, and ADR 0011/0017),
// exercised through the fixture rather than raw JSON: two sessions started
// in the *same* workspace directory must never be attributed to each other.
// Mutating `ClaudeAdapter::observe`'s join to fall back to `cwd` must turn
// this red.
#[test]
fn two_claude_sessions_in_one_workspace_are_never_confused() {
    let fixture = ClaudeFixture::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = CanonicalPath::resolve(dir.path()).expect("resolve tempdir");

    let req_a = fixture.startable_at(workspace.clone());
    let started_a = fixture
        .adapter()
        .start(&req_a)
        .expect("first session in the shared workspace must start");

    let req_b = fixture.startable_at(workspace.clone());
    let started_b = fixture
        .adapter()
        .start(&req_b)
        .expect("second session in the shared workspace must start");

    assert_ne!(
        started_a.pane, started_b.pane,
        "the fixture must hand out distinct panes even for one shared workspace"
    );

    let obs_a = fixture
        .adapter()
        .observe(&started_a.pane)
        .expect("observe must succeed for the first session's pane");
    let obs_b = fixture
        .adapter()
        .observe(&started_b.pane)
        .expect("observe must succeed for the second session's pane");

    assert_eq!(obs_a.pane, started_a.pane);
    assert_eq!(obs_b.pane, started_b.pane);

    // Compared against the fixture's own independent record of which
    // session it gave each pane — not against `started.harness_session_id`,
    // which comes from `start`'s own internal call to this same `observe`
    // and would move in lockstep with any join defect, passing
    // tautologically under exactly the mutation this test exists to catch.
    let expected_a = fixture
        .session_id_for(&started_a.pane)
        .expect("fixture must know the session id it gave the first pane");
    let expected_b = fixture
        .session_id_for(&started_b.pane)
        .expect("fixture must know the session id it gave the second pane");
    assert_ne!(
        expected_a, expected_b,
        "test setup error: the fixture must never hand out one session id for two panes"
    );

    assert_eq!(
        obs_a.harness_session_id.as_deref(),
        Some(expected_a.as_str()),
        "the first pane must resolve to its own session"
    );
    assert_eq!(
        obs_b.harness_session_id.as_deref(),
        Some(expected_b.as_str()),
        "the second pane must resolve to its own session, not the first's — this is the pane a \
         cwd-based join would get wrong, since both share one workspace directory and a \
         first-match-wins lookup would return the first pane's session here"
    );
}

// === Live drill =========================================================
//
// Everything above runs against a synchronous, in-memory double. This test
// is the one place in the crate that actually drives the real `herdr`
// binary and the live Irrlicht daemon, proving the command shapes
// `src/claude.rs` depends on (`herdr agent start --kind claude`, `herdr
// agent prompt --wait --until working`, Irrlicht's `launcher.herdr_pane_id`
// join key) still hold. `#[ignore]`d so `./check.sh` and a normal `cargo
// test` never pay for it, spawn a real Claude Code process, or depend on a
// running Irrlicht daemon.
//
// The crate's absolute prohibitions are honored throughout: this creates its
// own Herdr workspace (`herdr workspace create`) rather than touching any
// existing pane, and closes it via `PaneGuard` even if an assertion panics
// partway through.

/// Closes a real Herdr pane on drop — belt and suspenders alongside the
/// explicit `stop()` call below, so a panic between `start` and this test's
/// own cleanup does not leave a stray pane on the machine.
struct PaneGuard {
    herdr: HerdrCli,
    pane: PaneId,
}

impl Drop for PaneGuard {
    fn drop(&mut self) {
        let _ = self.herdr.pane_close(&self.pane);
    }
}

/// A per-run id, derived from the current time rather than `Uuid::new_v4`:
/// this workspace pins the `uuid` crate without the `v4` feature (see this
/// crate's other deterministic-uuid test helpers), so a live-only test
/// cannot reach for it either. Nanosecond resolution keeps two runs a human
/// might launch back to back from colliding.
fn per_run_task_id() -> uuid::Uuid {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .as_nanos();
    let low_48_bits = (nanos as u64) & 0xFFFF_FFFF_FFFF;
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{low_48_bits:012x}"))
        .expect("valid uuid")
}

/// This repository's own root — a directory Claude Code has certainly
/// already trusted, since this very test binary was built and is running
/// inside it. A fresh, never-before-seen directory would instead hit
/// Claude's own trust prompt, which nothing here could answer, and the
/// drill would hang rather than fail informatively. Overridable via
/// `FACTORY_ADAPTER_LIVE_DRILL_CWD` if this crate's position in the
/// workspace ever changes.
fn live_drill_workspace() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("FACTORY_ADAPTER_LIVE_DRILL_CWD") {
        return std::path::PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .and_then(Path::parent) // repo root
        .expect("factory-adapter is two directories below the repo root")
        .to_path_buf()
}

/// Whether `raw` (Irrlicht's `/api/v1/sessions` body) mentions `token`
/// anywhere near the entry whose `launcher.herdr_pane_id` is `pane` —
/// proving the real round trip landed in the real transcript Irrlicht
/// indexed, not just that `send` returned `Ok`. A window rather than a full
/// JSON parse: measured live, the fields that would carry `token`
/// (`task_summary`, `intent_headline`, `last_assistant_text`, all under
/// `metrics`) sit *before* `launcher` in the object Irrlicht emits, so the
/// window looks both ways around the pane-id marker.
fn payload_mentions_token_for_pane(raw: &str, pane: &str, token: &str) -> bool {
    let marker = format!("\"herdr_pane_id\":\"{pane}\"");
    let Some(pane_pos) = raw.find(&marker) else {
        return false;
    };
    let window_start = pane_pos.saturating_sub(6000);
    let window_end = (pane_pos + marker.len() + 500).min(raw.len());
    raw[window_start..window_end].contains(token)
}

fn poll_irrlicht_until(timeout: Duration, mut found: impl FnMut(&str) -> bool) -> bool {
    let irrlicht = IrrlichtHttp::new();
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(raw) = irrlicht.sessions() {
            if found(&raw) {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "drives the real `herdr` binary, a real Claude Code process, and the live Irrlicht \
            daemon. Creates its own Herdr workspace and closes it afterward — never touches an \
            existing pane. Run explicitly: \
            cargo test -p factory-adapter --test contract_claude -- --ignored --nocapture"]
fn live_claude_session_start_send_confirm_stop() {
    let clock = Instant::now();

    let herdr_for_adapter = HerdrCli::new("herdr");
    let herdr_for_cleanup = HerdrCli::new("herdr");
    let irrlicht = IrrlichtHttp::new();
    // A reachability probe, taken *before* this drill's own session exists,
    // so a hard assertion below can tell "Irrlicht is not running on this
    // machine" (ADR 0011 decision 3: Factory degrades without it, and this
    // drill must not fail for that unrelated reason) apart from "Irrlicht is
    // running but never saw our pane" (a real, reportable defect).
    let irrlicht_is_reachable = irrlicht.sessions().is_ok();
    let adapter = ClaudeAdapter::new(herdr_for_adapter, irrlicht);

    let workspace = CanonicalPath::resolve(live_drill_workspace())
        .expect("resolve the live drill workspace (see FACTORY_ADAPTER_LIVE_DRILL_CWD)");

    let scope_id = per_run_task_id();
    let session_id = per_run_task_id();
    let task_id = per_run_task_id();
    // A token unique to this run, mirroring `live_pi.rs`'s identical rule: a
    // fixed word would pass on a *previous* run's answer still sitting in
    // the pane's scrollback.
    let token = format!("ACK-{}", &task_id.simple().to_string()[24..]);

    let started = adapter
        .start(&factory_adapter::StartRequest {
            scope_id,
            session_id,
            workspace,
            generated_context: String::new(),
        })
        .expect("start must succeed against a real, freshly created Herdr pane");
    println!(
        "STARTED pane={:?} confidence={:?} harness_session_id={:?}",
        started.pane, started.confidence, started.harness_session_id
    );
    let _guard = PaneGuard {
        herdr: herdr_for_cleanup,
        pane: started.pane.clone(),
    };

    // send: submits and confirms via `herdr agent prompt --wait --until
    // working` — never a fire-and-forget write to the PTY.
    adapter
        .send(
            &started.pane,
            task_id,
            &format!("Reply with exactly the word {token} and nothing else."),
        )
        .expect("send must confirm via `herdr agent prompt --wait --until working`");

    // `send` only confirms *submission*; wait for the turn itself to finish
    // through real Herdr, before reading the answer back — mirrors
    // `live_pi.rs`'s identical rule.
    let waited = std::process::Command::new("herdr")
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

    // Read the token back off the real pane through Herdr — proof `send`
    // actually delivered *this* run's prompt, independent of whether
    // Irrlicht is even running.
    let transcript = std::process::Command::new("herdr")
        .args(["agent", "read", &started.pane.0])
        .output()
        .expect("herdr agent read");
    let screen = String::from_utf8_lossy(&transcript.stdout);
    assert!(
        screen.contains(&token),
        "the agent's answer to *this* run's task must be on the pane; looked for {token}"
    );

    // Claude's own observation source is Irrlicht, never Herdr (this
    // module's central claim) — so also confirm the token surfaces there,
    // keyed by exactly this pane's `launcher.herdr_pane_id`. Skipped, not
    // failed, when Irrlicht was already unreachable before this session
    // existed (see the reachability probe above); asserted hard otherwise,
    // since Irrlicht indexing a fresh transcript is measured to take one to
    // a few seconds, never tens of seconds.
    if irrlicht_is_reachable {
        let pane = started.pane.0.clone();
        let token_for_closure = token.clone();
        let found = poll_irrlicht_until(Duration::from_secs(45), move |raw| {
            payload_mentions_token_for_pane(raw, &pane, &token_for_closure)
        });
        assert!(
            found,
            "Irrlicht is reachable but never reported token {token} for pane {:?} — one of the \
             command shapes this adapter depends on (Irrlicht's launcher.herdr_pane_id join key, \
             its `adapter: \"claude-code\"` field) has likely drifted",
            started.pane
        );
    } else {
        println!(
            "SKIPPED Irrlicht-side confirmation: Irrlicht is not reachable at 127.0.0.1:7837 on \
             this machine (ADR 0011 decision 3 — Factory degrades without it). The Herdr-side \
             checks above already proved `send` and `start` work; only the observation half is \
             unverified by this run."
        );
    }

    // stop: closes the pane; a later observe must show it gone. Irrlicht
    // was measured (2026-09-09) to drop a closed session from
    // `/api/v1/sessions` within a couple of seconds of the process exiting
    // (kqueue `NOTE_EXIT`, ADR 0011) — not instantly — so this polls rather
    // than asserting on the very next call.
    adapter
        .stop(&started.pane)
        .expect("stop must succeed against the live pane");
    let deadline = Instant::now() + Duration::from_secs(15);
    let gone = loop {
        let observed = adapter.observe(&started.pane).expect(
            "observe must still answer for a pane whose session was stopped, even if only to \
             say it is gone",
        );
        if !observed.session_alive || Instant::now() >= deadline {
            break observed;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    assert!(
        !gone.session_alive,
        "a stopped session must no longer be reported alive"
    );

    // A drill that goes green in under a second did not run — mirrors
    // `live_pi.rs`'s identical floor.
    assert!(
        clock.elapsed().as_secs() >= 1,
        "this drill finished suspiciously fast ({:?}) — it likely never waited on a real model",
        clock.elapsed()
    );
}
