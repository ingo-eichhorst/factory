//! Opt-in real VM/PTY coverage for #218. The fixture is NOT Claude Code:
//! it proves launcher detection, naming and interactive prompt transport,
//! never model behavior, screen-rule accuracy or a task's reported outcome.
#![cfg(unix)]

use factory_core::adapter::agent::{LaunchKind, LaunchSpec};
use factory_core::adapter::runtime::{AgentRuntime, RuntimeStatus, StartRequest};
use factory_core::openshell::{self, CallbackTarget, OpenshellConfig, PlanInput};
use factory_plugins::HerdrRuntime;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use tokio::process::Command;

async fn command(words: &[String]) -> Value {
    let mut child = Command::new(&words[0]);
    child.args(&words[1..]).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(180), child.output())
        .await
        .expect("native QA command deadline")
        .unwrap();
    assert!(
        output.status.success(),
        "{words:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

struct OwnedSandbox {
    cli: String,
    name: String,
    instance: String,
    run: String,
}

struct OwnedTab {
    herdr: String,
    tab: Option<String>,
}

impl Drop for OwnedTab {
    fn drop(&mut self) {
        // Only the tab returned by this test's start, even after a panic.
        if let Some(tab) = &self.tab {
            let _ = std::process::Command::new(&self.herdr)
                .args(["tab", "close", tab])
                .output();
        }
    }
}

impl Drop for OwnedSandbox {
    fn drop(&mut self) {
        // Never delete by a guessed name: prove both labels are this test's.
        let Ok(output) = std::process::Command::new(&self.cli)
            .args(["sandbox", "get", &self.name, "-o", "json"])
            .output()
        else {
            return;
        };
        let Ok(got) = serde_json::from_slice::<Value>(&output.stdout) else {
            return;
        };
        if got["labels"]["factory.instance"] == self.instance
            && got["labels"]["factory.run"] == self.run
        {
            let _ = std::process::Command::new(&self.cli)
                .args(["sandbox", "delete", &self.name])
                .output();
        }
    }
}

#[tokio::test]
async fn a_command_harness_is_named_and_prompted_through_a_real_openshell_tty() {
    let (Ok(image), Ok(herdr)) = (
        std::env::var("FACTORY_QA_OPENSHELL_IMAGE"),
        std::env::var("FACTORY_QA_HERDR_BIN"),
    ) else {
        eprintln!("skipping OpenShell harness acceptance: set FACTORY_QA_OPENSHELL_IMAGE and FACTORY_QA_HERDR_BIN (an isolated qa-* session wrapper)");
        return;
    };
    let peer = command(&[herdr.clone(), "status".into(), "--json".into()]).await;
    let socket = peer["server"]["socket"]
        .as_str()
        .expect("QA Herdr server socket");
    assert!(
        socket.contains("/sessions/qa-"),
        "refusing a non-QA Herdr session: {peer}"
    );
    let cli = std::env::var("FACTORY_QA_OPENSHELL_BIN").unwrap_or_else(|_| "openshell".into());
    let id = uuid::Uuid::new_v4().to_string();
    let root = std::env::temp_dir().join(format!("factory-openshell-harness-{id}"));
    let cwd = root.join("fixture");
    let guides = root.join("guides");
    let state = root.join("state");
    std::fs::create_dir_all(cwd.join("bin")).unwrap();
    std::fs::create_dir_all(&guides).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let fixture = cwd.join("bin/claude");
    std::fs::write(
        &fixture,
        include_str!("fixtures/openshell-harness/bin/claude"),
    )
    .unwrap();
    std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cfg: OpenshellConfig = serde_yaml_ng::from_value(serde_yaml_ng::to_value(json!({
        "image": image, "upload": "workdir", "download": "none", "policy": {
            "filesystem_policy": {"include_workdir": true,
                "read_only": ["/usr", "/lib", "/bin", "/sbin", "/etc", "/opt", "/proc", "/sys", "/var", "/dev/urandom", "/dev/random"],
                "read_write": ["/sandbox", "/tmp", "/dev/null", "/dev/zero", "/dev/tty", "/dev/pts", "/dev/shm"]}
        }
    })).unwrap()).unwrap();
    let launch = LaunchSpec {
        kind: LaunchKind::Named("claude".into()),
        args: Vec::new(),
        agent_kind: None,
        env: BTreeMap::from([(
            "PATH".into(),
            "/sandbox/work/fixture/bin:/usr/local/bin:/usr/bin:/bin".into(),
        )]),
    };
    let callback = CallbackTarget {
        host: "host.openshell.internal".into(),
        port: 1,
    };
    let plan = openshell::plan(&PlanInput {
        config: &cfg,
        cli: &cli,
        image: &image,
        providers: &[],
        instance_id: &id,
        run_id: &id,
        task_id: &id,
        cwd: &cwd,
        guides_dir: &guides,
        state_dir: &state,
        launch: &launch,
        prompt: "QA_FIRST_PROMPT",
        callback: &callback,
    })
    .unwrap();
    std::fs::write(&plan.policy_path, &plan.policy).unwrap();
    std::fs::create_dir_all(&plan.stage_dir).unwrap();
    for (path, contents, mode) in &plan.stage_files {
        std::fs::write(path, contents).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(*mode)).unwrap();
    }
    std::fs::write(&plan.host_launcher, &plan.host_launcher_script).unwrap();
    let guard = OwnedSandbox {
        cli: cli.clone(),
        name: plan.sandbox.clone(),
        instance: id.clone(),
        run: id.clone(),
    };
    command(&plan.create).await;
    for upload in &plan.uploads {
        command(upload).await;
    }
    let runtime = HerdrRuntime::with_bin(&herdr);
    let name = format!("factory-qa-{}", &id[..8]);
    let session = runtime
        .start(&StartRequest {
            id: id.clone(),
            scope: String::new(),
            name: name.clone(),
            label: name.clone(),
            cwd: root.clone(),
            launch: plan.launch.clone(),
        })
        .await
        .unwrap();
    let mut tab = OwnedTab {
        herdr: herdr.clone(),
        tab: Some(session.meta["tab_id"].clone()),
    };
    let exercise = async {
        assert_eq!(session.meta.get("mode").map(String::as_str), Some("agent"));
        assert_eq!(session.meta.get("agent_name"), Some(&name));
        assert!(runtime
            .attach_command(&session)
            .unwrap()
            .contains(&format!("agent attach {name}")));
        loop {
            if runtime
                .read(&session, 80)
                .await
                .unwrap()
                .contains("QA_INITIAL:QA_FIRST_PROMPT")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let got = command(&[herdr.clone(), "agent".into(), "get".into(), name.clone()]).await;
        assert_eq!(got["result"]["agent"]["agent"], "claude");
        assert_eq!(got["result"]["agent"]["pane_id"], session.handle);
        assert_ne!(runtime.status(&session).await.unwrap(), RuntimeStatus::Gone);
        runtime.submit(&session, "QA_FOLLOWUP").await.unwrap();
        loop {
            if runtime
                .read(&session, 80)
                .await
                .unwrap()
                .contains("QA_RECEIVED:QA_FOLLOWUP")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(30), exercise).await;
    // stop closes this test's tab only; Drop then deletes its labelled VM.
    runtime.stop(&session).await.unwrap();
    tab.tab = None;
    drop(tab);
    assert_eq!(runtime.status(&session).await.unwrap(), RuntimeStatus::Gone);
    result.expect("initial and follow-up prompts cross the real VM/PTY");
    let owned = command(&[
        cli.clone(),
        "sandbox".into(),
        "get".into(),
        plan.sandbox.clone(),
        "-o".into(),
        "json".into(),
    ])
    .await;
    assert_eq!(owned["labels"]["factory.instance"], id);
    assert_eq!(owned["labels"]["factory.run"], id);
    command(&plan.delete).await;
    drop(guard);
    let absent = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(&cli)
            .args(["sandbox", "get", &plan.sandbox, "-o", "json"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!absent.status.success(), "this test's VM must be deleted");
    let connected = command(&[cli, "status".into(), "-o".into(), "json".into()]).await;
    assert_eq!(
        connected["status"], "connected",
        "absence must not mean a lost gateway"
    );
    std::fs::remove_dir_all(&root).unwrap();
}
