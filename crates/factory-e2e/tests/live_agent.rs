//! Drill 5: the whole chain with a **real agent doing real work**.
//!
//! Drills 1 to 3 prove Factory's own bookkeeping; drill 4 proves the adapter
//! reads a live Herdr. This one closes the loop: a task queued through
//! `factory_delegation` is written into an actual `pi` terminal by an actual
//! `herdr agent prompt`, the agent answers, and the adapter reads the state
//! change back so the result can be recorded.
//!
//! Marked `#[ignore]` — it needs a running Herdr, a prepared instance, and a
//! working harness, and it takes as long as a model takes to answer. Prepare
//! it with `herdr workspace create --cwd <instance>/worker` and
//! `herdr agent start <name> --kind pi --pane <id>`, then:
//!
//! ```text
//! FACTORY_E2E_ROOT=<instance> FACTORY_E2E_PANE=wF:p1 \
//!   cargo test -p factory-e2e --test live_agent -- --ignored --nocapture
//! ```
//!
//! It writes only inside the throwaway instance's own `.factory/` and into
//! the pane it was told about. It never touches the live instance.

use std::process::Command;

use factory_adapter::{Adapter, HerdrCli, PaneId, PiAdapter};
use factory_paths::CanonicalPath;
use factory_store::Store;
use factory_task::TaskStatus;
use factory_task::deliver::{PromptWriteError, PromptWriter};

/// The one writer in this workspace that reaches a real terminal. Design §5
/// puts the journal write before this call, so a failure here is the
/// ambiguous case the delivery journal exists to record.
struct HerdrPromptWriter {
    pane: String,
}

impl PromptWriter for HerdrPromptWriter {
    fn write_prompt(&mut self, _session: uuid::Uuid, prompt: &str) -> Result<(), PromptWriteError> {
        // `--wait --until working` matters: without it this returns while the
        // agent is still in the `idle` it was in *before* the prompt, and a
        // later "wait until idle" then matches that same stale state and
        // proves nothing. The first run of this drill did exactly that and
        // went green without the agent having answered.
        let output = Command::new("herdr")
            .args([
                "agent", "prompt", &self.pane, prompt, "--wait", "--until", "working",
            ])
            .output()
            .map_err(|err| PromptWriteError::new(format!("herdr agent prompt failed: {err}")))?;
        if !output.status.success() {
            return Err(PromptWriteError::new(format!(
                "herdr agent prompt exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }
}

#[test]
#[ignore = "needs a running Herdr, a prepared instance, and a live pi pane"]
fn a_real_agent_answers_a_factory_task() {
    let root = std::env::var("FACTORY_E2E_ROOT").expect("set FACTORY_E2E_ROOT");
    let pane = std::env::var("FACTORY_E2E_PANE").expect("set FACTORY_E2E_PANE");
    let root = std::path::PathBuf::from(root);

    // 1. The instance, projected from its own config.yaml.
    let config = factory_config::load(root.join(".factory/config.yaml")).expect("load config");
    let mut store = Store::open(&root).expect("open store");
    let scopes = factory_registry::resolve(&config, &root).expect("resolve");
    let report = factory_registry::reconcile(&store, &scopes).expect("reconcile");
    factory_registry::apply(&mut store, &scopes, &report).expect("apply");
    let worker = scopes
        .iter()
        .find(|s| s.name == "worker")
        .expect("the worker scope")
        .id;

    // 2. A session in the directory the pane is actually sitting in.
    let workspace = CanonicalPath::resolve(root.join("worker")).expect("resolve workspace");
    let session = uuid::Uuid::parse_str("00000000-0000-4000-8000-00000000f001").expect("uuid");
    if factory_session::show(&store, session).is_err() {
        factory_session::begin_start(&mut store, session, worker, "worker-agent", 1, &workspace)
            .expect("begin_start");
        factory_session::mark_running(&mut store, session).expect("mark_running");
    }

    // The pane and harness session ids are recorded from an authoritative
    // observation, never assumed — that is what migration 5's columns are for.
    let adapter = PiAdapter::new(HerdrCli::new("herdr"));
    let before = adapter.observe(&PaneId(pane.clone())).expect("observe");
    println!(
        "BEFORE  status={:?} confidence={:?}",
        before.harness_state, before.confidence
    );
    assert!(before.session_alive, "the pane must hold a live agent");

    // 3. A human queues the work. Nothing below chooses the session by hand.
    // The workspace pins `uuid` without the `v4` feature on purpose (see
    // `factory_task::create::create`'s doc comment: the caller mints task
    // ids), so this drill mints its own from the clock rather than adding a
    // feature to a centrally owned manifest.
    let task = {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos() as u64;
        uuid::Uuid::parse_str(&format!(
            "00000000-0000-4000-8000-{:012x}",
            nanos & 0xffff_ffff_ffff
        ))
        .expect("uuid")
    };
    // A token unique to this run. Asserting on a fixed word would pass on the
    // *previous* run's answer still sitting in the scrollback — which is
    // exactly the kind of unearned green this drill exists to avoid.
    let token = format!("ACK-{}", &task.to_string()[24..]);
    println!("TOKEN {token}");

    factory_delegation::queue::queue_from_human(
        &mut store,
        task,
        worker,
        None,
        None,
        &format!("Reply with exactly the word {token} and nothing else."),
    )
    .expect("queue");

    let assignment =
        factory_task::assign::assign(&mut store, task, "worker-agent", 1).expect("assign");
    assert_eq!(
        assignment,
        factory_task::assign::Assignment::Assigned(session)
    );

    // 4. Delivery: journal first, then the real terminal write.
    let mut writer = HerdrPromptWriter { pane: pane.clone() };
    factory_task::deliver::deliver(&mut store, task, &mut writer).expect("deliver to a real pane");
    factory_task::deliver::mark_running(&mut store, task).expect("mark_running");
    assert_eq!(
        factory_task::create::show(&store, task)
            .expect("show")
            .status,
        TaskStatus::Running
    );

    // 5. Wait for the agent, then read its state back through the adapter.
    let waited = Command::new("herdr")
        .args([
            "agent",
            "wait",
            &pane,
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

    let after = adapter
        .observe(&PaneId(pane.clone()))
        .expect("observe after");
    println!(
        "AFTER   status={:?} signal={:?}",
        after.harness_state, after.task_signal
    );
    assert_ne!(
        after.harness_state, "working",
        "the agent must have finished before its result is recorded"
    );

    let transcript = Command::new("herdr")
        .args(["agent", "read", &pane])
        .output()
        .expect("herdr agent read");
    let screen = String::from_utf8_lossy(&transcript.stdout);
    println!(
        "SCREEN TAIL:\n{}",
        screen
            .chars()
            .rev()
            .take(700)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    );
    // The point of the whole drill: a real model, reached through Factory's
    // own delivery path, actually did what the task asked.
    assert!(
        screen.contains(&token),
        "the agent's answer to *this* run's task must be on the pane; looked for {token}"
    );

    // 6. The operator records the outcome. Factory never invents a result.
    factory_task::complete::done(
        &mut store,
        task,
        Some("agent replied on the live pane"),
        None,
    )
    .expect("done");
    assert_eq!(
        factory_task::create::show(&store, task)
            .expect("show")
            .status,
        TaskStatus::Done
    );

    // 7. And the delivery journal still refuses a silent second send.
    assert!(
        factory_task::deliver::deliver(&mut store, task, &mut writer).is_err(),
        "a delivered task is never silently re-delivered, live pane or not"
    );
}
