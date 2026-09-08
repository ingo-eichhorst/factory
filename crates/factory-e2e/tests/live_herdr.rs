//! Drill 4: the same recovery path as drill 3, but against the **live Herdr
//! running on this machine** instead of a constructed `Observation`.
//!
//! Marked `#[ignore]` on purpose. `./check.sh` must not depend on a running
//! Herdr, a logged-in harness, or a pane that happens to exist — a suite that
//! goes red because a terminal was closed teaches an operator to ignore red.
//! Run it deliberately:
//!
//! ```text
//! cargo test -p factory-e2e --test live_herdr -- --ignored --nocapture
//! ```
//!
//! It is strictly read-only against Herdr: `herdr agent list`, `agent get`,
//! and `agent explain`. It starts no pane, sends no prompt, and touches no
//! agent's state. The database it writes is a throwaway instance in a
//! temporary directory, never the live one.
//!
//! The session row here is seeded directly rather than through
//! `factory_session::begin_start`, because this drill's subject is the
//! adapter-to-recovery boundary; registration and session start are covered
//! end-to-end by drills 1 to 3.

mod common;

use factory_adapter::{Adapter, Confidence, HerdrCli, PaneId, PiAdapter};
use factory_recovery::reconnect::{Outcome, reconnect_after_supervisor_restart};

/// The first pane the live Herdr reports as running `pi`, if any.
fn a_live_pi_pane() -> Option<(String, String)> {
    let out = std::process::Command::new("herdr")
        .args(["agent", "list"])
        .output()
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    for agent in value["result"]["agents"].as_array()? {
        if agent["agent"].as_str() == Some("pi") {
            let pane = agent["pane_id"].as_str()?.to_string();
            let cwd = agent["cwd"].as_str()?.to_string();
            return Some((pane, cwd));
        }
    }
    None
}

fn seed_disconnected_session(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    scope_id: uuid::Uuid,
    workspace: &str,
    pane: &str,
    harness_session_id: Option<&str>,
) {
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO sessions \
         (id, scope_id, agent_name, workspace_path, state, herdr_pane_id, harness_session_id) \
         VALUES (?1, ?2, 'alpha-agent', ?3, 'disconnected', ?4, ?5)",
        (
            id.to_string(),
            scope_id.to_string(),
            workspace,
            pane,
            harness_session_id,
        ),
    )
    .expect("seed session");
    tx.execute(
        "INSERT INTO workspace_leases (session_id, canonical_workspace_path) VALUES (?1, ?2)",
        (id.to_string(), workspace),
    )
    .expect("seed lease");
    tx.commit().expect("commit");
}

#[test]
#[ignore = "needs a running Herdr with at least one live `pi` pane"]
fn a_live_pi_pane_is_observed_and_reconnected() {
    let Some((pane, cwd)) = a_live_pi_pane() else {
        panic!("no live `pi` pane found — start one, or skip this drill");
    };
    println!("LIVE PANE {pane} (cwd {cwd})");

    let adapter = PiAdapter::new(HerdrCli::new("herdr"));
    let observed = adapter
        .observe(&PaneId(pane.clone()))
        .expect("the live adapter must observe a real pi pane");
    println!("OBSERVED {observed:#?}");
    assert_eq!(
        observed.confidence,
        Confidence::Authoritative,
        "a pi pane with the Herdr lifecycle hook installed reports authoritatively"
    );
    let real_session_id = observed
        .harness_session_id
        .clone()
        .expect("a live pi pane names its own harness session");

    // 1. A recorded identity that matches what Herdr says: reconnection.
    let mut inst = common::build();
    let alpha = inst.alpha;
    let ws = inst.workspace("ws-live");
    let session = common::uid(90);
    seed_disconnected_session(
        &mut inst.store,
        session,
        alpha,
        ws.as_path().to_str().expect("utf-8 workspace"),
        &pane,
        Some(&real_session_id),
    );

    let records = reconnect_after_supervisor_restart(&mut inst.store, &adapter).expect("reconnect");
    println!("RECONNECT RECORDS {records:#?}");
    let record = records
        .iter()
        .find(|r| r.session_id == session)
        .expect("our session must be decided");
    assert!(
        matches!(record.outcome, Outcome::Reconnected),
        "live authoritative evidence for the same harness session must reconnect it, got {:?}",
        record.outcome
    );
    assert_eq!(
        factory_session::show(&inst.store, session)
            .expect("show")
            .state,
        factory_session::SessionState::Running,
    );

    // 2. The same live pane, but the database remembers a *different* harness
    //    session: ADR 0019 decision 3's warning that a pane id may since have
    //    been reused. The pane exists and answers authoritatively, and that
    //    still must not be enough.
    let mut inst2 = common::build();
    let alpha2 = inst2.alpha;
    let ws2 = inst2.workspace("ws-live-stale");
    let stale = common::uid(91);
    seed_disconnected_session(
        &mut inst2.store,
        stale,
        alpha2,
        ws2.as_path().to_str().expect("utf-8 workspace"),
        &pane,
        Some("00000000-0000-4000-8000-0000deadbeef"),
    );

    let records2 =
        reconnect_after_supervisor_restart(&mut inst2.store, &adapter).expect("reconnect stale");
    println!("STALE RECORDS {records2:#?}");
    let record2 = records2
        .iter()
        .find(|r| r.session_id == stale)
        .expect("the stale session must be decided");
    assert!(
        !matches!(record2.outcome, Outcome::Reconnected),
        "a reused pane id must never promote a session whose harness id disagrees, got {:?}",
        record2.outcome
    );
    assert_ne!(
        factory_session::show(&inst2.store, stale)
            .expect("show")
            .state,
        factory_session::SessionState::Running,
    );
}
