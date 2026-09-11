//! A7 P02, partial executable probe, not an end-to-end business-effect test.
//! Real SQLite snapshots + delivery/reconciliation code; an in-memory writer
//! is the ONLY receiver. No Herdr, daemon, network, or external authority.
//! The old-snapshot test intentionally characterizes a missing guarantee:
//! passing it does NOT certify safe redelivery after arbitrary rollback.

mod restore_common;

use std::path::Path;

use factory_recovery::restore::reconcile;
use factory_store::Store;
use factory_task::deliver::{
    DeliverError, PromptWriteError, PromptWriter, authorise_resume, deliver,
};
use restore_common::{lease_is_open, seed_scope, seed_session, seed_task, session_state, task_row};

/// Receipt is ground truth for this test double only, never supplied to core.
/// Both modes return the same error; core cannot infer receipt from that error.
struct AmbiguousWriter {
    accept_before_error: bool,
    calls: usize,
    receipts: Vec<String>,
}

impl AmbiguousWriter {
    fn new(accept_before_error: bool) -> Self {
        Self {
            accept_before_error,
            calls: 0,
            receipts: Vec::new(),
        }
    }
}

impl PromptWriter for AmbiguousWriter {
    fn write_prompt(&mut self, _session: uuid::Uuid, prompt: &str) -> Result<(), PromptWriteError> {
        self.calls += 1;
        if self.accept_before_error {
            self.receipts.push(prompt.to_owned());
        }
        Err(PromptWriteError::new("synthetic ambiguous acknowledgement"))
    }
}

fn setup(root: &Path) -> (Store, uuid::Uuid, uuid::Uuid) {
    let mut store = Store::open(root).expect("isolated store");
    let scope = seed_scope(&mut store, 1, root);
    let session = seed_session(&mut store, 2, scope, &root.join(".factory/work"), "running");
    let task = seed_task(&mut store, 3, scope, "queued", Some(session));
    (store, task, session)
}

fn journal(store: &Store, task: uuid::Uuid) -> Vec<Option<String>> {
    let mut stmt = store
        .connection()
        .prepare("SELECT outcome FROM delivery_attempts WHERE task_id = ?1 ORDER BY id")
        .expect("prepare journal");
    stmt.query_map([task.to_string()], |row| row.get(0))
        .expect("read journal")
        .collect::<Result<_, _>>()
        .expect("journal rows")
}

fn budget(store: &Store, task: uuid::Uuid) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT authorised_deliveries FROM tasks WHERE id = ?1",
            [task.to_string()],
            |row| row.get(0),
        )
        .expect("budget")
}

/// Copy ONLY the closed VACUUM INTO result, never the live WAL database.
/// The source snapshot remains immutable even when two different worlds use it.
fn restore_copy(snapshot: &Path, destination: &Path) -> Store {
    std::fs::copy(snapshot, destination).expect("copy immutable snapshot");
    let mut store = Store::open_at(destination).expect("open restored copy");
    assert_eq!(store.integrity_check().expect("integrity"), "ok");
    reconcile(&mut store).expect("actual restore reconciliation");
    store
}

fn preserved_attempt(accept_before_error: bool) {
    let root = tempfile::tempdir().expect("temp root");
    let (mut live, task, session) = setup(root.path());
    let mut writer = AmbiguousWriter::new(accept_before_error);
    assert!(matches!(
        deliver(&mut live, task, &mut writer),
        Err(DeliverError::WriteFailed { .. })
    ));
    assert_eq!(writer.receipts.len(), usize::from(accept_before_error));
    assert_eq!(journal(&live, task), vec![Some("failed".into())]);
    // Even the live journal refuses another call despite the ambiguous error.
    assert!(matches!(
        deliver(&mut live, task, &mut writer),
        Err(DeliverError::AlreadyAttempted(_))
    ));
    assert_eq!(writer.calls, 1);

    let dir = root.path().join(".factory");
    let snapshot = dir.join("after-attempt.sqlite");
    live.backup_to(&snapshot)
        .expect("real snapshot after attempt");
    let mut restored = restore_copy(&snapshot, &dir.join("restored.sqlite"));
    assert_eq!(
        task_row(&restored, task),
        (
            "blocked".into(),
            Some("interrupted".into()),
            Some(session.to_string())
        )
    );
    assert_eq!(session_state(&restored, session), "disconnected");
    assert!(lease_is_open(&restored, session));
    assert_eq!(journal(&restored, task), vec![Some("failed".into())]);
    assert!(matches!(
        deliver(&mut restored, task, &mut writer),
        Err(DeliverError::NotQueued { .. })
    ));
    assert_eq!(writer.calls, 1, "no writer invocation on the held path");
    let again = reconcile(&mut restored).expect("repeat reconciliation");
    assert!(again.tasks.is_empty());
    assert!(again.sessions.is_empty());
    assert!(lease_is_open(&restored, session));
}

#[test]
fn retained_attempt_blocks_after_receipt_with_lost_ack() {
    preserved_attempt(true);
}

#[test]
fn retained_attempt_also_blocks_when_writer_received_nothing() {
    preserved_attempt(false);
}

#[test]
fn old_snapshot_cannot_distinguish_no_receipt_from_later_receipt() {
    let root = tempfile::tempdir().expect("temp root");
    let (mut live, task, session) = setup(root.path());
    let dir = root.path().join(".factory");
    let snapshot = dir.join("before-attempt.sqlite");
    live.backup_to(&snapshot)
        .expect("snapshot before any attempt");
    let snapshot_bytes = std::fs::read(&snapshot).expect("snapshot bytes");
    let mut writer = AmbiguousWriter::new(true);

    // Same snapshot, world A: no delivery has happened.
    let no_receipt = restore_copy(&snapshot, &dir.join("world-a.sqlite"));
    assert_eq!(writer.calls, 0);
    assert!(writer.receipts.is_empty());
    let world_a = (
        task_row(&no_receipt, task),
        journal(&no_receipt, task),
        budget(&no_receipt, task),
    );

    // World B: actual deliver() journals and calls the local receiver, which
    // accepts the prompt but reports an error. The old backup cannot include it.
    assert!(matches!(
        deliver(&mut live, task, &mut writer),
        Err(DeliverError::WriteFailed { .. })
    ));
    assert_eq!(writer.receipts.len(), 1);
    assert!(writer.receipts[0].contains(&task.to_string()));
    assert_eq!(journal(&live, task), vec![Some("failed".into())]);
    let mut later_receipt = restore_copy(&snapshot, &dir.join("world-b.sqlite"));
    let world_b = (
        task_row(&later_receipt, task),
        journal(&later_receipt, task),
        budget(&later_receipt, task),
    );
    assert_eq!(
        world_a, world_b,
        "reconciliation sees identical evidence in different worlds"
    );
    assert_eq!(world_b, (("queued".into(), None, None), vec![], 1));
    assert_eq!(session_state(&later_receipt, session), "disconnected");
    assert!(lease_is_open(&later_receipt, session));
    // Important boundary: queued does NOT itself prove automatic redelivery.
    assert!(matches!(
        deliver(&mut later_receipt, task, &mut writer),
        Err(DeliverError::NotAssigned(_))
    ));
    assert_eq!(writer.calls, 1);
    assert_eq!(
        std::fs::read(&snapshot).expect("unchanged source"),
        snapshot_bytes
    );
}

#[test]
fn resume_increases_budget_without_resolving_ambiguous_receipt() {
    let root = tempfile::tempdir().expect("temp root");
    let (mut live, task, session) = setup(root.path());
    let mut writer = AmbiguousWriter::new(true);
    assert!(matches!(
        deliver(&mut live, task, &mut writer),
        Err(DeliverError::WriteFailed { .. })
    ));
    let dir = root.path().join(".factory");
    let snapshot = dir.join("after-attempt.sqlite");
    live.backup_to(&snapshot).expect("snapshot");
    let mut restored = restore_copy(&snapshot, &dir.join("resumed.sqlite"));
    assert_eq!(budget(&restored, task), 1);

    // This is a direct library call, not an authenticated human request.
    // No revocation source is simulated: the current API receives none.
    authorise_resume(&mut restored, task).expect("resume mechanism");
    assert_eq!(budget(&restored, task), 2);
    assert_eq!(task_row(&restored, task), ("queued".into(), None, None));
    assert_eq!(journal(&restored, task), vec![Some("failed".into())]);
    assert_eq!(writer.receipts.len(), 1);
    assert_eq!(writer.calls, 1, "resume itself does not call the writer");
    assert!(lease_is_open(&restored, session));
    assert!(matches!(
        deliver(&mut restored, task, &mut writer),
        Err(DeliverError::NotAssigned(_))
    ));
    assert_eq!(writer.calls, 1);
}
