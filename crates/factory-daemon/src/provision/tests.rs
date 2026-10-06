//! `#234` against a stand-in `openshell` that keeps its providers and
//! profiles as files, and a stand-in `launchctl`. Nothing here talks to a
//! real gateway.

use super::*;
use factory_core::config::{Config, DaemonConfig, Factory, Instance, PolicyDeclaration};
use factory_plugins::{Registry, SqliteStore};
use std::os::unix::fs::PermissionsExt;

const SUFFIX: &str = "a1b2c3d4";

struct Fixture {
    root: PathBuf,
    dir: PathBuf,
    engine: Arc<Engine>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.join("calls")).unwrap_or_default()
    }
    fn file(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.join(name)).ok()
    }
    fn key(&self) -> AgentKey {
        ("demo".into(), "boxed".into())
    }
    fn readiness(&self) -> Readiness {
        self.engine.l2.provision.readiness(&self.key()).expect("the pass judged the agent")
    }
    async fn pass(&self) -> Readiness {
        self.engine.l2_service().provision_pass().await;
        self.readiness()
    }
}

fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The stand-in `openshell`. Every argv goes to `calls`; a credential
/// variable it is given goes to `given-<VAR>` (so a test can see the value
/// arrived) and only the fact that it was set goes to `calls`.
fn fake_openshell(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir.join("providers")).unwrap();
    std::fs::create_dir_all(dir.join("profiles")).unwrap();
    std::fs::write(dir.join("gateway"), "connected").unwrap();
    let path = dir.join("openshell");
    executable(
        &path,
        &format!(
            r#"#!/bin/sh
d='{d}'
printf '%s\n' "$*" >> "$d/calls"
for v in CLAUDE_CODE_OAUTH_TOKEN GITHUB_TOKEN QA_TOKEN; do
  eval "x=\${{$v:-}}"
  if [ -n "$x" ]; then printf '%s' "$x" > "$d/given-$v"; echo "  ($v set)" >> "$d/calls"; fi
done
case "$1 $2" in
'--version ') echo 'openshell 0.1.2' ;;
'status -o') printf '{{"gateway":"openshell","server":"https://127.0.0.1:17670","status":"%s","version":"0.1.2"}}\n' "$(cat "$d/gateway")" ;;
'provider list')
  printf '{{"providers":['; sep=''
  for f in "$d"/providers/*; do [ -e "$f" ] || continue
    printf '%s{{"name":"%s","type":"%s"}}' "$sep" "$(basename "$f")" "$(cat "$f")"; sep=','; done
  printf ']}}\n' ;;
'provider create')
  shift 2; while [ $# -gt 0 ]; do case "$1" in --name) n=$2; shift ;; --type) t=$2; shift ;; esac; shift; done
  if [ -e "$d/providers/$n" ]; then echo "provider $n already exists" >&2; exit 1; fi
  printf '%s' "$t" > "$d/providers/$n" ;;
'provider update') [ -e "$d/providers/$3" ] || {{ echo "no provider $3" >&2; exit 1; }} ;;
'provider delete') rm -f "$d/providers/$3" ;;
'provider get') [ -e "$d/providers/$3" ] || {{ echo "not found" >&2; exit 1; }} ;;
'profile list')
  printf '['; sep=''
  for f in "$d"/profiles/*; do [ -e "$f" ] || continue
    printf '%s{{"id":"%s","resource_version":7,"credentials":[{{"env_vars":["%s"]}}]}}' "$sep" "$(basename "$f")" "$(cat "$f")"; sep=','; done
  printf ']\n' ;;
'profile import'|'profile update')
  if [ "$2" = update ] && [ "$5" != "$(sed -n 's/^id: //p' "$4")" ]; then echo 'error: the following required arguments were not provided: <ID>' >&2; exit 2; fi
  if [ "$2" = update ] && ! grep -q '^resource_version: 7$' "$4"; then echo 'custom provider profile update requires a non-zero resource_version' >&2; exit 1; fi
  id=$(sed -n 's/^id: //p' "$4"); env=$(grep -o 'env_vars: \[[A-Z_]*' "$4" | sed 's/.*\[//')
  printf '%s' "$env" > "$d/profiles/$id" ;;
'sandbox create') echo '{{}}' ;;
'sandbox exec') if [ -e "$d/smoke" ]; then cat "$d/smoke"; else echo 'factory-smoke http x 200 '; echo 'factory-smoke done'; fi ;;
esac
"#,
            d = dir.display()
        ),
    );
    path
}

/// The stand-in `launchctl`: `kickstart` brings the fake gateway up unless
/// `gateway-stays-down` exists.
fn fake_launchctl(dir: &Path) -> PathBuf {
    let path = dir.join("launchctl");
    executable(
        &path,
        &format!(
            "#!/bin/sh\nd='{d}'\nprintf '%s\\n' \"$*\" >> \"$d/launchctl.log\"\ncase \"$1\" in kickstart) [ -e \"$d/gateway-stays-down\" ] || echo connected > \"$d/gateway\" ;; esac\n",
            d = dir.display()
        ),
    );
    path
}

/// An instance with one scope, `demo`, whose agent `boxed` has `block` as
/// its `openshell:` block.
fn fixture(block: &str, tools: impl FnOnce(&Path) -> Tools) -> Fixture {
    let root = std::env::temp_dir().join(format!("factory-provision-{}", uuid::Uuid::new_v4().simple()));
    let dir = root.join("tools");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(root.join(".factory")).unwrap();
    std::fs::create_dir_all(root.join("projects/demo")).unwrap();
    let cli = fake_openshell(&dir);
    let scope = demo_scope(&root, &cli, block);
    let config = Config {
        version: 1,
        instance: Instance { id: format!("{SUFFIX}-0000-4000-8000-000000000000"), name: "test".into() },
        daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
        scope: None,
        scopes: vec![scope],
        roles: Default::default(),
        dashboard: None,
        policies: PolicyDeclaration::default(),
        quality: Default::default(),
        infrastructure: Default::default(),
        secrets: Vec::new(),
        plugins_dir: None,
        renewals: Vec::new(),
        renewals_notify: None,
    };
    let factory_bin = root.join("factory");
    std::fs::write(&factory_bin, "factory cli v1").unwrap();
    let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
    let mut engine = Engine::new(Factory { root: root.clone(), config }, Registry::with_builtins(), store, factory_bin, Vec::new());
    let mut t = tools(&dir);
    t.launchctl = fake_launchctl(&dir);
    engine.l2.provision = Provisioner::new(t);
    Fixture { root, dir, engine: Arc::new(engine) }
}

/// The scope `demo`, whose agent `boxed` has `block` as its `openshell:`
/// block -- `CLI` and `ROOT` in it are the stand-in CLI and the instance root.
fn demo_scope(root: &Path, cli: &Path, block: &str) -> factory_core::config::Scope {
    let block = block.replace("CLI", &cli.display().to_string()).replace("ROOT", &root.display().to_string());
    let indented: String = block.lines().map(|l| format!("      {l}\n")).collect();
    let mut scope: factory_core::config::Scope = serde_yaml_ng::from_str(&format!(
        "id: demo-id\nname: demo\ngit: https://github.com/example/demo.git\nagents:\n  - name: boxed\n    harness: shell\n    sandbox: openshell\n    openshell:\n{indented}"
    ))
    .unwrap();
    scope.path = root.join("projects/demo");
    scope
}

fn no_build(_: &Path) -> Tools {
    Tools { launchctl: PathBuf::new(), build_script: None, source: None, gateway_wait: Duration::from_secs(1) }
}

fn private_file(path: &Path, contents: &str, mode: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

const MANAGED: &str = "\
image: ROOT/image.tar.gz
cli: CLI
callback: http://127.0.0.1:9
providers:
  - name: factory-claude
    type: claude-code-oauth
    credential: { from: file, path: ROOT/secrets/claude }
  - name: factory-github
    type: github-publish
    credential: { from: command, run: cat ROOT/secrets/gh }
policy:
  network_policies: {}
";

fn managed() -> Fixture {
    let f = fixture(MANAGED, no_build);
    std::fs::write(f.root.join("image.tar.gz"), "rootfs").unwrap();
    private_file(&f.root.join("secrets/claude"), "file-secret-123\n", 0o600);
    private_file(&f.root.join("secrets/gh"), "cmd-secret-456", 0o600);
    f
}

#[tokio::test]
async fn declared_providers_are_made_from_their_sources_and_the_values_go_only_to_the_child() {
    let f = managed();
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Ready, "{r:?}");

    // Created under this instance's suffix, from suffixed profiles.
    assert_eq!(f.file(&format!("providers/factory-claude-{SUFFIX}")).as_deref(), Some(format!("claude-code-oauth-{SUFFIX}").as_str()));
    assert_eq!(f.file(&format!("providers/factory-github-{SUFFIX}")).as_deref(), Some(format!("github-publish-{SUFFIX}").as_str()));
    assert_eq!(f.file(&format!("profiles/claude-code-oauth-{SUFFIX}")).as_deref(), Some("CLAUDE_CODE_OAUTH_TOKEN"));

    // The value reached the child through its environment ...
    assert_eq!(f.file("given-CLAUDE_CODE_OAUTH_TOKEN").as_deref(), Some("file-secret-123"));
    assert_eq!(f.file("given-GITHUB_TOKEN").as_deref(), Some("cmd-secret-456"));
    let calls = f.calls();
    assert!(calls.contains(&format!("provider create --name factory-claude-{SUFFIX} --type claude-code-oauth-{SUFFIX} --credential CLAUDE_CODE_OAUTH_TOKEN")), "{calls}");
    // ... and nowhere else: not a command line, not the readiness a page
    // or the Inbox shows, not the config the roster writes back.
    for secret in ["file-secret-123", "cmd-secret-456"] {
        assert!(!calls.contains(secret), "{secret} on a command line: {calls}");
        assert!(!serde_json::to_string(&f.engine.l2.provision.all().values().collect::<Vec<_>>()).unwrap().contains(secret));
        assert!(!serde_json::to_string(&declared(&f.engine.factory_snapshot())[0].config).unwrap().contains(secret));
    }

    // The smoke ran once, labelled so a restart sweeps an orphan, and was deleted.
    let created: Vec<&str> = calls.lines().filter(|l| l.starts_with("sandbox create")).collect();
    assert_eq!(created.len(), 1, "{calls}");
    assert!(created[0].contains(&format!("--provider factory-claude-{SUFFIX} --provider factory-github-{SUFFIX}")), "{}", created[0]);
    assert!(created[0].contains("--label factory.run=smoke-") && created[0].contains(&format!("--label factory.instance={SUFFIX}-")), "{}", created[0]);
    assert!(calls.lines().any(|l| l.starts_with("sandbox delete factory-s")), "{calls}");
    assert!(calls.lines().any(|l| l.starts_with("sandbox exec -n factory-s") && l.contains("--timeout")), "{calls}");

    // Nothing changed: nothing is given again and nothing is smoked again.
    std::fs::write(f.dir.join("calls"), "").unwrap();
    assert_eq!(f.pass().await.state, ReadinessState::Ready);
    let calls = f.calls();
    assert!(!calls.contains("provider update") && !calls.contains("provider create") && !calls.contains("sandbox create"), "{calls}");

    // Rotation: a new value in the file is given to the provider, and smoked.
    private_file(&f.root.join("secrets/claude"), "rotated-secret-789", 0o600);
    assert_eq!(f.pass().await.state, ReadinessState::Ready);
    let calls = f.calls();
    assert!(calls.contains(&format!("provider update factory-claude-{SUFFIX} --credential CLAUDE_CODE_OAUTH_TOKEN")), "{calls}");
    assert!(!calls.contains("provider update factory-github"), "only the one that changed: {calls}");
    assert!(calls.contains("sandbox create"), "{calls}");
    assert!(!calls.contains("rotated-secret-789"));
    assert_eq!(f.file("given-CLAUDE_CODE_OAUTH_TOKEN").as_deref(), Some("rotated-secret-789"));
}

#[tokio::test]
async fn a_profile_left_by_an_earlier_daemon_is_updated_by_its_id() {
    let f = managed();
    std::fs::write(f.dir.join(format!("profiles/claude-code-oauth-{SUFFIX}")), "OLD").unwrap();
    assert_eq!(f.pass().await.state, ReadinessState::Ready, "{:?}", f.readiness());
    assert!(f.calls().lines().any(|l| l.starts_with("profile update -f ") && l.ends_with(&format!(" claude-code-oauth-{SUFFIX}"))), "{}", f.calls());
    assert_eq!(f.file(&format!("profiles/claude-code-oauth-{SUFFIX}")).as_deref(), Some("CLAUDE_CODE_OAUTH_TOKEN"));
}

#[tokio::test]
async fn another_instances_providers_and_profiles_are_never_written() {
    let f = managed();
    // What a person -- or the live instance -- made by hand.
    std::fs::write(f.dir.join("providers/factory-claude"), "claude-code-oauth").unwrap();
    std::fs::write(f.dir.join("profiles/claude-code-oauth"), "HAND_MADE").unwrap();
    std::fs::write(f.dir.join("providers/factory-claude-0f0f0f0f"), "claude-code-oauth-0f0f0f0f").unwrap();
    assert_eq!(f.pass().await.state, ReadinessState::Ready);
    let calls = f.calls();
    for line in calls.lines().filter(|l| l.starts_with("provider ") || l.starts_with("profile import") || l.starts_with("profile update")) {
        if line.starts_with("provider list") {
            continue;
        }
        assert!(line.contains(&format!("-{SUFFIX}")) || line.contains("/profile.yaml"), "wrote something not its own: {line}");
    }
    assert_eq!(f.file("providers/factory-claude").as_deref(), Some("claude-code-oauth"));
    assert_eq!(f.file("profiles/claude-code-oauth").as_deref(), Some("HAND_MADE"));
    assert!(f.file("providers/factory-claude-0f0f0f0f").is_some());
    assert!(!calls.contains("provider delete"), "{calls}");
}

#[tokio::test]
async fn a_credential_file_others_can_read_is_refused_and_nothing_is_created() {
    let f = managed();
    private_file(&f.root.join("secrets/claude"), "file-secret-123", 0o644);
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Needs);
    let thing = r.thing.clone().unwrap();
    assert!(thing.contains("factory-claude") && thing.contains("readable or writable by others") && thing.contains("chmod 600"), "{thing}");
    assert!(r.command.as_deref().unwrap().starts_with("claude setup-token"), "{r:?}");
    assert!(!f.calls().contains("provider create"), "{}", f.calls());
    assert!(!r.reason().contains("file-secret-123"));

    std::fs::remove_file(f.root.join("secrets/claude")).unwrap();
    let r = f.pass().await;
    assert!(r.thing.unwrap().contains("cannot be read"), "a removed source is named");
}

#[tokio::test]
async fn a_source_removed_or_rotated_since_the_last_pass_is_found_at_dispatch() {
    let f = managed();
    let config = declared(&f.engine.factory_snapshot())[0].config.clone();
    assert_eq!(f.pass().await.state, ReadinessState::Ready);
    assert!(f.engine.l2_service().sandbox_gate(&f.key(), &config).await.is_ok());

    // Rotated: the dispatch waits for the pass that gives the new value.
    let engine = f.engine.clone();
    let looping = tokio::spawn(async move {
        loop {
            engine.l2.provision.wake.notified().await;
            engine.l2_service().provision_pass().await;
        }
    });
    private_file(&f.root.join("secrets/claude"), "rotated-before-dispatch", 0o600);
    assert!(f.engine.l2_service().sandbox_gate(&f.key(), &config).await.is_ok());
    assert_eq!(f.file("given-CLAUDE_CODE_OAUTH_TOKEN").as_deref(), Some("rotated-before-dispatch"));
    looping.abort();

    // Removed: the run fails closed now, with the one thing and its command.
    std::fs::remove_file(f.root.join("secrets/claude")).unwrap();
    let e = f.engine.l2_service().sandbox_gate(&f.key(), &config).await.unwrap_err();
    assert!(e.contains("needs the credential for factory-claude") && e.contains("cannot be read") && e.contains("claude setup-token"), "{e}");
}

#[tokio::test]
async fn a_failing_command_source_never_puts_what_it_printed_in_the_reason() {
    let f = fixture(&MANAGED.replace("cat ROOT/secrets/gh", "sh ROOT/leak.sh"), no_build);
    std::fs::write(f.root.join("leak.sh"), "printf leaked-secret-1; echo leaked-secret-1 >&2; exit 3\n").unwrap();
    std::fs::write(f.root.join("image.tar.gz"), "rootfs").unwrap();
    private_file(&f.root.join("secrets/claude"), "file-secret-123", 0o600);
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Needs);
    let thing = r.thing.clone().unwrap();
    assert!(thing.contains("factory-github") && thing.contains("exit status: 3"), "{thing}");
    assert!(!thing.contains("leaked-secret-1"), "{thing}");
    assert!(r.command.clone().unwrap().contains("make `sh ") && !r.reason().contains("leaked-secret-1"), "the hint names the command, never its output");
}

#[tokio::test]
async fn credential_failures_never_copy_stderr_or_a_truncated_secret() {
    let f = managed();
    let source = f.root.join("source-failure.sh");
    executable(&source, "#!/bin/sh\necho stderr-only-secret >&2\nexit 3\n");
    let error = command_value(&[source.display().to_string()], "credential source").await.unwrap_err();
    assert!(error.contains("exit status: 3"), "{error}");
    assert!(!error.contains("stderr-only-secret"), "{error}");

    let provider = f.root.join("provider-failure.sh");
    executable(&provider, "#!/bin/sh\nprintf '%s' \"$QA_TOKEN\" >&2\nexit 4\n");
    let value = Secret("long-sensitive-value-".repeat(40));
    let error = exec(
        &[provider.display().to_string()],
        Some(("QA_TOKEN", &value)),
        QUICK,
        "giving the provider its credential",
        &[value.expose()],
    )
    .await
    .unwrap_err();
    assert!(error.contains("exit status: 4"), "{error}");
    assert!(!error.contains("long-sensitive-value"), "{error}");
}

#[tokio::test]
async fn a_credential_the_endpoint_rejects_fails_the_smoke_and_the_agent_needs_it() {
    let f = managed();
    std::fs::write(
        f.dir.join("smoke"),
        "factory-smoke env factory-claude ok\nfactory-smoke http factory-claude 401 {\"error\":{\"message\":\"Invalid bearer token\"}}\nfactory-smoke done\n",
    )
    .unwrap();
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Needs);
    assert!(r.thing.as_deref().unwrap().contains("credential was rejected"), "{r:?}");
    assert!(r.command.as_deref().unwrap().contains("claude setup-token"), "{r:?}");
    assert!(f.calls().lines().any(|l| l.starts_with("sandbox delete factory-s")), "the smoke sandbox is deleted either way");
    let gate = f.engine.l2_service().sandbox_gate(&f.key(), &declared(&f.engine.factory_snapshot())[0].config).await.unwrap_err();
    assert!(gate.contains("credential was rejected"), "{gate}");
    // Nothing changed: the next pass does not make another VM to be told
    // the same thing, and the readiness keeps its age.
    let since = f.readiness().since;
    let again = f.pass().await;
    assert_eq!(f.calls().matches("sandbox create").count(), 1, "{}", f.calls());
    assert_eq!((again.state, again.since), (ReadinessState::Needs, since));
    // A new value is worth another try at once.
    private_file(&f.root.join("secrets/claude"), "a-new-token", 0o600);
    std::fs::remove_file(f.dir.join("smoke")).unwrap();
    assert_eq!(f.pass().await.state, ReadinessState::Ready);
    assert_eq!(f.calls().matches("sandbox create").count(), 2);
}

#[tokio::test]
async fn a_stopped_gateway_is_started_without_restarting_it_and_recorded() {
    let f = managed();
    std::fs::write(f.dir.join("gateway"), "disconnected").unwrap();
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Ready, "{r:?}");
    let log = f.file("launchctl.log").unwrap();
    assert!(log.contains(&format!("kickstart gui/{}/{GATEWAY_SERVICE}", unsafe { libc::getuid() })), "{log}");
    assert!(!log.contains("-k"), "a restart would take the gateway's sandboxes with it: {log}");
    let rows = f.engine.l2.provision.gateway_rows();
    assert_eq!(rows[0].status, "connected");
    assert!(rows[0].started_with.as_deref().unwrap().starts_with("launchctl kickstart"), "{rows:?}");
    assert!(rows[0].started_at.is_some());
}

#[tokio::test]
async fn a_gateway_that_went_down_since_the_last_pass_is_started_by_the_next_dispatch() {
    let f = managed();
    assert_eq!(f.pass().await.state, ReadinessState::Ready);
    std::fs::write(f.dir.join("gateway"), "disconnected").unwrap();
    let resolved = f.engine.l2_service().sandbox_gate(&f.key(), &declared(&f.engine.factory_snapshot())[0].config).await;
    assert!(resolved.is_ok(), "{resolved:?}");
    assert!(f.file("launchctl.log").unwrap().contains("kickstart"));
    assert_eq!(f.file("gateway").as_deref().map(str::trim), Some("connected"));
}

#[tokio::test]
async fn a_gateway_that_stays_down_is_what_the_agent_needs() {
    let f = managed();
    std::fs::write(f.dir.join("gateway"), "disconnected").unwrap();
    std::fs::write(f.dir.join("gateway-stays-down"), "").unwrap();
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Needs);
    assert!(r.thing.as_deref().unwrap().contains("which is disconnected"), "{r:?}");
    assert!(r.command.as_deref().unwrap().starts_with("launchctl kickstart gui/"), "{r:?}");
    // Not again within the backoff: one start per gateway per two minutes.
    f.pass().await;
    assert_eq!(f.file("launchctl.log").unwrap().matches("kickstart").count(), 1);
}

#[tokio::test]
async fn factorys_image_is_built_in_the_background_and_the_previous_one_kept_while_the_next_builds() {
    let f = fixture(&MANAGED.replace("image: ROOT/image.tar.gz\n", ""), |dir| {
        let script = dir.join("build.sh");
        executable(
            &script,
            &format!("#!/bin/sh\necho building into $OUT\n[ -e '{d}/slow-build' ] && sleep 2\nmkdir -p \"$OUT\" && echo rootfs > \"$OUT/factory-agent-rootfs.tar.gz\"\n", d = dir.display()),
        );
        Tools { build_script: Some(script), ..no_build(dir) }
    });
    private_file(&f.root.join("secrets/claude"), "file-secret-123", 0o600);
    private_file(&f.root.join("secrets/gh"), "cmd-secret-456", 0o600);
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Preparing, "{r:?}");
    assert!(r.thing.unwrap().contains("building Factory's sandbox image"));
    // The build tells the loop; here the test asks again once it is done.
    tokio::time::timeout(Duration::from_secs(10), f.engine.l2.provision.wake.notified()).await.expect("the build finished");
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Ready, "{r:?}");
    let first = r.image.clone().unwrap();
    assert!(first.contains("/.factory/openshell-images/") && first.ends_with("/factory-agent-rootfs.tar.gz"), "{first}");
    let gate = f.engine.l2_service().sandbox_gate(&f.key(), &declared(&f.engine.factory_snapshot())[0].config).await.unwrap();
    assert_eq!(gate.image, first);
    assert_eq!(gate.providers, [format!("factory-claude-{SUFFIX}"), format!("factory-github-{SUFFIX}")]);

    // A new `factory` CLI is installed: a new image is built, and runs use
    // the previous one until it exists.
    std::fs::write(f.dir.join("slow-build"), "").unwrap();
    std::fs::write(f.root.join("factory"), "factory cli v2").unwrap();
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Ready, "{r:?}");
    assert_eq!(r.image.as_deref(), Some(first.as_str()), "the old image until the new one exists");
    assert!(r.notes.iter().any(|n| n.contains("being built in the background")), "{:?}", r.notes);
    tokio::time::timeout(Duration::from_secs(10), f.engine.l2.provision.wake.notified()).await.expect("the rebuild finished");
    let r = f.pass().await;
    let second = r.image.clone().unwrap();
    assert_ne!(second, first);
    assert!(Path::new(&first).is_file(), "the previous image is kept");
    assert_eq!(f.calls().matches("sandbox create").count(), 2, "a new image is smoked");
}

#[tokio::test]
async fn an_agent_that_names_nothing_the_daemon_keeps_is_checked_but_never_gated_or_smoked() {
    let f = fixture("image: ROOT/image.tar.gz\ncli: CLI\ncallback: http://127.0.0.1:9\nproviders: [by-hand]\npolicy: {}\n", no_build);
    std::fs::write(f.root.join("image.tar.gz"), "rootfs").unwrap();
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Needs);
    assert!(r.command.as_deref().unwrap().starts_with("openshell provider create --name by-hand"), "{r:?}");
    std::fs::write(f.dir.join("providers/by-hand"), "github").unwrap();
    assert_eq!(f.pass().await.state, ReadinessState::Ready);
    assert!(!f.calls().contains("sandbox create"), "nothing of the daemon's to prove");
    let resolved = f.engine.l2_service().sandbox_gate(&f.key(), &declared(&f.engine.factory_snapshot())[0].config).await.unwrap();
    assert_eq!(resolved.providers, ["by-hand"]);
}

#[tokio::test]
async fn failed_rebuild_keeps_old_image_usable_but_is_visible_until_recovery() {
    let f = fixture(&MANAGED.replace("image: ROOT/image.tar.gz\n", ""), |dir| {
        let script = dir.join("build.sh");
        executable(&script, &format!(
            "#!/bin/sh\nsleep 1\nif [ -e '{d}/fail-build' ]; then echo 'missing musl C compiler' >&2; exit 1; fi\nmkdir -p \"$OUT\" && echo rootfs > \"$OUT/factory-agent-rootfs.tar.gz\"\n",
            d = dir.display()
        ));
        Tools { build_script: Some(script), ..no_build(dir) }
    });
    private_file(&f.root.join("secrets/claude"), "file-secret-123", 0o600);
    private_file(&f.root.join("secrets/gh"), "cmd-secret-456", 0o600);
    assert_eq!(f.pass().await.state, ReadinessState::Preparing);
    tokio::time::timeout(Duration::from_secs(10), f.engine.l2.provision.wake.notified()).await.unwrap();
    let ready = f.pass().await;
    assert_eq!(ready.state, ReadinessState::Ready);
    let old_image = ready.image.unwrap();

    std::fs::write(f.dir.join("fail-build"), "").unwrap();
    std::fs::write(f.root.join("factory"), "factory cli v2").unwrap();
    let rebuilding = f.pass().await;
    assert!(rebuilding.image_build_failure.is_none(), "in-progress is not failure");
    tokio::time::timeout(Duration::from_secs(10), f.engine.l2.provision.wake.notified()).await.unwrap();
    let stale = f.pass().await;
    assert_eq!(stale.state, ReadinessState::Ready, "the older smoke-tested image can still run");
    assert_eq!(stale.image.as_deref(), Some(old_image.as_str()));
    let failure = stale.image_build_failure.unwrap();
    assert!(failure.reason.contains("missing musl C compiler"));
    assert!(failure.reason.contains(".log"));
    assert!(failure.command.is_some());
    let gate = f.engine.l2_service().sandbox_gate(&f.key(), &declared(&f.engine.factory_snapshot())[0].config).await.unwrap();
    assert_eq!(gate.image, old_image);

    let doctor = f.engine.doctor_report().await.unwrap();
    assert_eq!(doctor.openshell_image_failures.len(), 1);
    let row = &doctor.openshell_image_failures[0];
    assert_eq!((&*row.scope, &*row.agent), ("demo", "boxed"));
    assert_eq!(row.image, old_image);
    assert_eq!(row.failure, failure);
    let repeated = f.pass().await;
    assert_eq!(repeated.image_build_failure.unwrap().since, failure.since, "evidence time is not refreshed on read");

    std::fs::remove_file(f.dir.join("fail-build")).unwrap();
    std::fs::write(f.root.join("factory"), "factory cli v3").unwrap();
    assert!(f.pass().await.image_build_failure.is_none(), "a different build is not the old failure");
    tokio::time::timeout(Duration::from_secs(10), f.engine.l2.provision.wake.notified()).await.unwrap();
    let recovered = f.pass().await;
    assert_eq!(recovered.state, ReadinessState::Ready);
    assert_ne!(recovered.image.unwrap(), old_image);
    assert!(recovered.image_build_failure.is_none());
    assert!(f.engine.doctor_report().await.unwrap().openshell_image_failures.is_empty());
}

#[tokio::test]
async fn two_agents_declaring_one_provider_differently_are_both_told() {
    let a: Declared = Declared {
        key: ("a".into(), "x".into()),
        config: serde_yaml_ng::from_str("providers:\n  - { name: p, type: t, credential: { from: env, name: A } }\npolicy: {}\n").unwrap(),
        git: None,
        secrets: BTreeMap::new(),
        unresolved: None,
    };
    let mut b = a.clone();
    b.key = ("b".into(), "y".into());
    let same = conflicting_providers(&[a.clone(), b.clone()]);
    assert!(same.is_empty(), "the same declaration twice is fine");
    b.config = serde_yaml_ng::from_str("providers:\n  - { name: p, type: t, credential: { from: env, name: B } }\npolicy: {}\n").unwrap();
    let conflicts = conflicting_providers(&[a, b]);
    assert_eq!(conflicts.len(), 2);
    assert!(conflicts[&("a".to_string(), "x".to_string())].contains("scope b agent y"));
}

#[tokio::test]
async fn sources_resolve_and_a_secret_never_prints() {
    let secret = resolve(&CredentialSource::Command { run: "printf '  spaced-value \\n'".into() }).await.unwrap();
    assert_eq!(secret.expose(), "spaced-value");
    assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
    let home = resolve(&CredentialSource::Env { name: "HOME".into() }).await.unwrap();
    assert!(!home.expose().is_empty());
    let unset = resolve(&CredentialSource::Env { name: "FACTORY_TEST_SURELY_UNSET_VARIABLE".into() }).await.unwrap_err();
    assert!(unset.contains("is not set in the daemon's environment"), "{unset}");
    let empty = resolve(&CredentialSource::Command { run: "true".into() }).await.unwrap_err();
    assert_eq!(empty, "it is empty");
    // `gh` under launchd: Homebrew's bin is on the PATH a command gets.
    let path = augmented_path();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")) && dirs.contains(&PathBuf::from("/usr/local/bin")), "{dirs:?}");
}

#[tokio::test]
async fn the_gate_waits_briefly_for_a_first_judgement_and_never_lets_a_needs_through() {
    let f = managed();
    let config = declared(&f.engine.factory_snapshot())[0].config.clone();
    let now = Utc::now();
    let needs = Readiness {
        state: ReadinessState::Needs,
        thing: Some("the OpenShell gateway openshell, which is stopped".into()),
        command: Some("launchctl kickstart gui/1/sh.brew.openshell".into()),
        since: now,
        checked_at: now,
        image: None,
        image_build_failure: None,
        notes: vec![],
        expiring: vec![],
    };
    f.engine.l2.provision.set_for_test(&f.key(), needs);
    let e = f.engine.l2_service().sandbox_gate(&f.key(), &config).await.unwrap_err();
    assert!(e.contains("needs the OpenShell gateway") && e.contains("launchctl kickstart"), "{e}");

    // Not judged yet: the gate asks for a pass and takes its answer.
    let f = managed();
    let config = declared(&f.engine.factory_snapshot())[0].config.clone();
    let engine = f.engine.clone();
    let looping = tokio::spawn(async move {
        loop {
            engine.l2.provision.wake.notified().await;
            engine.l2_service().provision_pass().await;
        }
    });
    let resolved = f.engine.l2_service().sandbox_gate(&f.key(), &config).await;
    looping.abort();
    assert_eq!(resolved.unwrap().image, f.root.join("image.tar.gz").display().to_string());
}

// -- #244: the declared secrets catalogue ----------------------------------

/// `MANAGED`, with both providers naming catalogue entries instead.
const BY_REFERENCE: &str = "\
image: ROOT/image.tar.gz
cli: CLI
callback: http://127.0.0.1:9
providers:
  - name: factory-claude
    type: claude-code-oauth
    credential: { secret: claude-oauth-token }
  - name: factory-github
    type: github-publish
    credential: { secret: github-gh-login }
policy:
  network_policies: {}
";

/// The catalogue the issue declares, over this fixture's two sources.
fn catalogue(root: &Path) -> Vec<SecretDecl> {
    serde_yaml_ng::from_str(&format!(
        "- name: claude-oauth-token\n  kind: token\n  source: {{ from: file, path: {r}/secrets/claude }}\n  expires: 2027-10-04\n  renew: \"claude setup-token, then (umask 077; cat > {r}/secrets/claude)\"\n\
         - name: github-gh-login\n  kind: token\n  source: {{ from: command, run: cat {r}/secrets/gh }}\n  expires: never\n  renew: gh auth login\n",
        r = root.display()
    ))
    .unwrap()
}

fn by_reference(f: &Fixture) {
    f.engine.replace_instance_secrets(catalogue(&f.root));
    f.engine.replace_scope("demo-id", demo_scope(&f.root, &f.dir.join("openshell"), BY_REFERENCE));
    f.engine.factory_snapshot().config.validate().expect("the catalogue and the references load");
}

#[tokio::test]
async fn moving_inline_credentials_into_the_catalogue_gives_the_same_digest_and_recreates_nothing() {
    // Acceptance 1: the curator stays ready and its providers are not
    // recreated when its inline sources become `{ secret: }` references.
    let f = managed();
    assert_eq!(f.pass().await.state, ReadinessState::Ready);
    let given = lock(&f.engine.l2.provision.given).clone();
    assert_eq!(given.len(), 2, "{given:?}");

    by_reference(&f);
    let d = &declared(&f.engine.factory_snapshot())[0];
    assert_eq!(d.secrets.keys().collect::<Vec<_>>(), ["factory-claude", "factory-github"]);
    std::fs::write(f.dir.join("calls"), "").unwrap();
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Ready, "{r:?}");
    let calls = f.calls();
    for verb in ["provider create", "provider update", "provider delete", "sandbox create"] {
        assert!(!calls.contains(verb), "{verb} after moving to the catalogue: {calls}");
    }
    assert_eq!(*lock(&f.engine.l2.provision.given), given, "the same value digest, per provider");

    // A dispatch, handed the block as written -- references and all.
    let raw = f.engine.factory_snapshot().scope("demo").unwrap().declared_agents()[0].openshell.clone().unwrap();
    assert!(serde_json::to_string(&raw).unwrap().contains(r#""secret":"claude-oauth-token""#));
    let resolved = f.engine.l2_service().sandbox_gate(&f.key(), &raw).await.unwrap();
    assert_eq!(resolved.providers, [format!("factory-claude-{SUFFIX}"), format!("factory-github-{SUFFIX}")]);
    assert!(!f.calls().contains("provider update"), "{}", f.calls());

    // The secret's expiry is the secret's: no per-provider expiring line.
    assert!(r.expiring.is_empty(), "{:?}", r.expiring);
    // And the catalogue's sources were checked -- whether they resolve, never what they gave.
    let checks = f.engine.l2.provision.source_checks();
    assert!(checks["claude-oauth-token"].resolves && checks["github-gh-login"].resolves, "{checks:?}");
    let shown = serde_json::to_string(&checks).unwrap();
    assert!(!shown.contains("file-secret-123") && !shown.contains("cmd-secret-456"), "{shown}");
}

#[tokio::test]
async fn a_referenced_secret_that_will_not_resolve_names_its_renew_line() {
    let f = managed();
    by_reference(&f);
    std::fs::remove_file(f.root.join("secrets/claude")).unwrap();
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Needs);
    assert!(r.thing.as_deref().unwrap().contains("cannot be read"), "{r:?}");
    assert_eq!(r.command, Some(format!("claude setup-token, then (umask 077; cat > {}/secrets/claude)", f.root.display())));
    let check = &f.engine.l2.provision.source_checks()["claude-oauth-token"];
    assert!(!check.resolves && check.reason.as_deref().unwrap().contains("cannot be read"), "{check:?}");
}

#[tokio::test]
async fn a_reference_the_catalogue_does_not_declare_is_a_need_never_a_panic() {
    let f = managed();
    f.engine.replace_scope("demo-id", demo_scope(&f.root, &f.dir.join("openshell"), BY_REFERENCE));
    let r = f.pass().await;
    assert_eq!(r.state, ReadinessState::Needs);
    assert!(r.thing.as_deref().unwrap().contains("the secret claude-oauth-token"), "{r:?}");
    assert!(!f.calls().contains("provider create"));
    let raw = f.engine.factory_snapshot().scope("demo").unwrap().declared_agents()[0].openshell.clone().unwrap();
    let e = f.engine.l2_service().sandbox_gate(&f.key(), &raw).await.unwrap_err();
    assert!(e.contains("the secret claude-oauth-token"), "{e}");
}

#[tokio::test]
async fn one_provider_named_inline_by_one_agent_and_by_reference_by_another_is_not_a_conflict() {
    let catalogue: Vec<SecretDecl> =
        serde_yaml_ng::from_str("- { name: a, kind: token, source: { from: env, name: A } }").unwrap();
    let inline: OpenshellConfig =
        serde_yaml_ng::from_str("providers:\n  - { name: p, type: t, credential: { from: env, name: A } }\npolicy: {}\n").unwrap();
    let reference: OpenshellConfig =
        serde_yaml_ng::from_str("providers:\n  - { name: p, type: t, credential: { secret: a } }\npolicy: {}\n").unwrap();
    let (resolved, secrets) = resolve_secrets(&reference, &catalogue).unwrap();
    assert_eq!(resolved, inline, "a reference resolves to the very source written inline");
    assert!(secrets.contains_key("p"));
    let a = Declared { key: ("a".into(), "x".into()), config: inline, git: None, secrets: BTreeMap::new(), unresolved: None };
    let b = Declared { key: ("b".into(), "y".into()), config: resolved, git: None, secrets, unresolved: None };
    assert!(conflicting_providers(&[a, b]).is_empty());
}
