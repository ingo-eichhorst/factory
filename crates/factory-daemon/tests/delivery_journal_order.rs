//! Design §5 step 3: "record the delivery attempt before writing to the
//! PTY." `factory_task::deliver::deliver` owns that ordering; this test pins
//! that `ops::task::send` actually goes through it rather than writing to
//! the adapter some other way first.
//!
//! The adapter here opens a **separate, read-only** connection to the same
//! database file at `send()` time and checks whether a `delivery_attempts`
//! row already exists for the task being sent. WAL mode makes that read
//! concurrent-safe even while the primary connection holds the mutex, and
//! `deliver`'s first transaction commits before it ever calls the writer —
//! so the row must already be visible by the time `send` runs, if the
//! journal-then-write ordering holds.

mod common;

use factory_adapter::{Adapter, AdapterError, Observation, PaneId, StartRequest, StartedSession};

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// An adapter that, at `send()` time, opens a second read-only connection to
/// `db_path` and records whether `delivery_attempts` already carries a row
/// for the task it was just asked to send.
struct JournalCheckingAdapter {
    db_path: std::path::PathBuf,
    // One entry per `send()` call, in order — not just the last one: a caller
    // that calls `send` an extra, premature time (the exact "write before the
    // journal" mutation) must show up as a second, `false` entry rather than
    // being overwritten by a correct entry that comes after it.
    journal_seen: std::sync::Mutex<Vec<bool>>,
}

impl JournalCheckingAdapter {
    fn new(db_path: std::path::PathBuf) -> Self {
        Self {
            db_path,
            journal_seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn journal_seen(&self) -> Vec<bool> {
        self.journal_seen.lock().unwrap().clone()
    }
}

impl Adapter for JournalCheckingAdapter {
    fn start(&self, req: &StartRequest) -> Result<StartedSession, AdapterError> {
        Ok(StartedSession {
            pane: PaneId(format!("pane-{}", req.session_id)),
            harness_session_id: None,
            confidence: factory_adapter::Confidence::Authoritative,
        })
    }

    fn send(&self, _pane: &PaneId, task_id: uuid::Uuid, _prompt: &str) -> Result<(), AdapterError> {
        let read_only = factory_store::Store::open_read_only(&self.db_path)
            .expect("open a second, read-only connection to the same database");
        let exists: bool = read_only
            .connection()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM delivery_attempts WHERE task_id = ?1)",
                [task_id.to_string()],
                |row| row.get(0),
            )
            .expect("query delivery_attempts");
        self.journal_seen.lock().unwrap().push(exists);
        Ok(())
    }

    fn observe(&self, pane: &PaneId) -> Result<Observation, AdapterError> {
        Ok(Observation {
            pane: pane.clone(),
            harness_state: "idle".to_string(),
            confidence: factory_adapter::Confidence::Unavailable,
            session_alive: true,
            task_signal: factory_adapter::TaskSignal::NoChange,
            transcript_path: None,
            harness_session_id: None,
        })
    }

    fn interrupt(&self, _pane: &PaneId) -> Result<(), AdapterError> {
        Ok(())
    }

    fn stop(&self, _pane: &PaneId) -> Result<(), AdapterError> {
        Ok(())
    }

    fn attach_command(&self, _pane: &PaneId) -> Result<Vec<String>, AdapterError> {
        Ok(vec![])
    }

    fn runtime_version(&self) -> Result<String, AdapterError> {
        Ok("journal-checking-fake".to_string())
    }
}

#[test]
fn delivery_journals_the_attempt_before_the_adapter_is_ever_called() {
    let fixture = common::build(&[common::ScopeSpec::new("alpha", "alpha")]);
    let scope_id = fixture.scope("alpha");
    let db_path = fixture
        .instance_root()
        .join(".factory")
        .join("factory.sqlite");

    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = std::sync::Arc::new(JournalCheckingAdapter::new(db_path));
    let handler = factory_daemon::FactoryHandler::new(
        store,
        ArcAdapter(adapter.clone()),
        fixture.instance_root(),
    );

    let session_id = uid(1001);
    use factory_daemon::Handler as _;
    handler
        .handle_command(factory_daemon::envelope::CommandRequest {
            request_id: uid(1),
            scope_id,
            command: "agent.start".to_string(),
            payload: serde_json::json!({ "session_id": session_id.to_string(), "agent_name": "agent" }),
            expected_revision: None,
        })
        .expect("agent.start succeeds");

    let task_id = uid(1002);
    let result = handler
        .handle_command(factory_daemon::envelope::CommandRequest {
            request_id: uid(2),
            scope_id,
            command: "task.send".to_string(),
            payload: serde_json::json!({
                "task_id": task_id.to_string(),
                "prompt": "do it",
                "target_session_id": session_id.to_string(),
            }),
            expected_revision: None,
        })
        .expect("task.send succeeds");
    assert_eq!(result.result["delivery"]["sent"], true);

    assert_eq!(
        adapter.journal_seen(),
        vec![true],
        "exactly one Adapter::send call is expected, and by the time it runs the \
         delivery_attempts row must already exist on a second connection — this is what \
         'journal before write' means operationally. A second, premature call recorded here \
         (or a `false` entry) means something wrote to the adapter before the journal committed."
    );
}

/// A thin `Adapter` wrapper so `Arc<JournalCheckingAdapter>` itself can be
/// handed to `FactoryHandler::new`, which needs an owned, `'static` value —
/// `JournalCheckingAdapter` is not `Clone` (it carries a `Mutex` the test
/// still wants to read afterward via the original `Arc`).
struct ArcAdapter(std::sync::Arc<JournalCheckingAdapter>);

impl Adapter for ArcAdapter {
    fn start(&self, req: &StartRequest) -> Result<StartedSession, AdapterError> {
        self.0.start(req)
    }
    fn send(&self, pane: &PaneId, task_id: uuid::Uuid, prompt: &str) -> Result<(), AdapterError> {
        self.0.send(pane, task_id, prompt)
    }
    fn observe(&self, pane: &PaneId) -> Result<Observation, AdapterError> {
        self.0.observe(pane)
    }
    fn interrupt(&self, pane: &PaneId) -> Result<(), AdapterError> {
        self.0.interrupt(pane)
    }
    fn stop(&self, pane: &PaneId) -> Result<(), AdapterError> {
        self.0.stop(pane)
    }
    fn attach_command(&self, pane: &PaneId) -> Result<Vec<String>, AdapterError> {
        self.0.attach_command(pane)
    }
    fn runtime_version(&self) -> Result<String, AdapterError> {
        self.0.runtime_version()
    }
}
