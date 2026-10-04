use super::*;
use factory_core::{
    adapter::TaskStore,
    config::{Config, Scope},
    task::{NewTask, Schedule},
};
use factory_plugins::{Registry, SqliteStore};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

struct Fixture {
    root: PathBuf,
    engine: Arc<Engine>,
}

#[test]
fn renewal_async_frames_stay_bounded_on_daemon_worker_stacks() {
    let f = fixture();
    let snapshot = f.engine.factory_snapshot();
    let tools = probes::Tools::default();
    let (_shutdown, rx) = tokio::sync::watch::channel(false);
    let dates = std::mem::size_of_val(&f.engine.important_dates(None));
    let probes = std::mem::size_of_val(&probes::observe(&snapshot, &tools, Utc::now()));
    let observer = std::mem::size_of_val(&observe_loop(f.engine.clone(), rx.clone()));
    let supervisor = std::mem::size_of_val(&run(f.engine.clone(), rx));
    let router = std::mem::size_of_val(
        &f.engine
            .handle_request(factory_core::protocol::Request::ImportantDates { scope: None }),
    );
    eprintln!("async frames: dates={dates}, probes={probes}, observer={observer}, supervisor={supervisor}, router={router}");
    assert!(
        dates < 256 * 1024
            && probes < 256 * 1024
            && observer < 256 * 1024
            && supervisor < 256 * 1024
            && router < 256 * 1024,
        "metadata jobs must not exhaust daemon worker stacks"
    );
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn engine(factory: Factory) -> Arc<Engine> {
    let database = factory.database_path();
    let tasks: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
    Arc::new(
        Engine::new(
            factory,
            Registry::with_builtins(),
            tasks,
            PathBuf::from("factory"),
            Vec::new(),
        )
        .with_renewal_stores(
            store::ObservationStore::open(&database).unwrap(),
            store::ObservationStore::open(&database).unwrap(),
            store::AlertStore::open(&database).unwrap(),
        ),
    )
}
fn fixture() -> Fixture {
    let root = std::env::temp_dir().join(format!("factory-dates-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join(".factory")).unwrap();
    let mut config: Config = serde_yaml_ng::from_str(
        "instance: {id: a1b2c3d4-0000-4000-8000-000000000000, name: test}\n",
    )
    .unwrap();
    let mut scope: Scope = serde_yaml_ng::from_str(
        "id: demo-id\nname: demo\nagents: [{name: curator, harness: shell}]\n",
    )
    .unwrap();
    scope.path = root.clone();
    config.scope = Some(scope.clone());
    config.scopes = vec![scope];
    Fixture {
        engine: engine(Factory {
            root: root.clone(),
            config,
        }),
        root,
    }
}
fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}
fn script(path: &Path, text: &str) {
    write(path, text);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
fn dated(id: &str, expiry: DateTime<Utc>) -> ExpiryObservation {
    let mut item = probes::observation(
        id.into(),
        "OpenShell factory-claude".into(),
        DateKind::Credential,
        DateSource::Openshell,
        Utc::now(),
    );
    item.expires_at = Some(expiry);
    item.basis = DateBasis::Observed;
    item.observed_at = Some(Utc::now());
    item.affects = vec![DateDependency {
        scope: Some("demo".into()),
        agent: Some("curator".into()),
        environment: None,
        provider: Some("factory-claude".into()),
        label: "demo/curator".into(),
    }];
    item
}

#[tokio::test]
async fn a_declared_overdue_date_is_one_current_inbox_item_and_hot_edits_resolve_it() {
    let f = fixture();
    let path = f.root.join(".factory/config.yaml");
    write(&path, "renewals: [{name: licence, kind: licence, expires: 2020-01-01, affects: [demo/curator]}]\n");
    let first = f.engine.important_dates(None).await.unwrap();
    assert_eq!(first.entries.len(), 1);
    assert_eq!(first.overdue, 1);
    assert_eq!(first.entries[0].milestone, Some(RenewalMilestone::Expired));
    for _ in 0..3 {
        assert_eq!(
            f.engine.important_dates(None).await.unwrap().entries[0]
                .observation
                .id,
            first.entries[0].observation.id
        );
    }
    write(&path, "renewals: [{name: licence, kind: licence, expires: 2090-01-01, affects: [demo/curator]}]\n");
    let updated = f.engine.important_dates(None).await.unwrap();
    assert_eq!(updated.overdue, 0);
    assert_eq!(updated.entries[0].milestone, None);
    write(
        &path,
        "renewals: [{name: licence, kind: licence, credential: secret-must-not-leave-config}]\n",
    );
    let invalid = f.engine.important_dates(None).await.unwrap();
    assert_eq!(
        invalid.entries[0].observation.expires_at,
        updated.entries[0].observation.expires_at
    );
    assert!(!invalid.observation_issues.is_empty());
    assert!(!serde_json::to_string(&invalid)
        .unwrap()
        .contains("secret-must-not-leave-config"));
}

#[tokio::test]
async fn observed_expiry_wins_the_documented_claude_alias_and_forecasts_the_exact_agent() {
    let f = fixture();
    write(&f.root.join(".factory/config.yaml"), "renewals: [{name: claude-subscription-token, kind: credential, expires: 2090-01-01, affects: [demo/curator]}]\n");
    let expiry = Utc::now() + chrono::Duration::days(60);
    f.engine
        .credential_expiries
        .replace(vec![dated("openshell:active:factory-claude", expiry)], true)
        .await
        .unwrap();
    let task = f
        .engine
        .create(NewTask {
            title: "future audit".into(),
            instructions: "true".into(),
            scope: Some("demo".into()),
            agent: Some("curator".into()),
            schedule: Some(Schedule::Every {
                seconds: 90 * 86400,
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    let report = f.engine.important_dates(None).await.unwrap();
    assert_eq!(
        report.entries.len(),
        1,
        "the declaration decorates its actual evidence rather than duplicating it"
    );
    let entry = &report.entries[0];
    assert!(entry.conflict);
    assert_eq!(entry.observation.expires_at, Some(expiry));
    assert_eq!(entry.milestone, Some(RenewalMilestone::ScheduledRun));
    assert_eq!(entry.scheduled_risks[0].task, task.id);
    assert_eq!(entry.observation.basis, DateBasis::Observed);
}

#[tokio::test]
async fn failed_discovery_preserves_a_last_known_expired_date_across_a_restart() {
    let f = fixture();
    let expired = dated("credential", Utc::now() - chrono::Duration::days(1));
    f.engine
        .credential_expiries
        .replace(vec![expired.clone()], true)
        .await
        .unwrap();
    f.engine
        .credential_expiries
        .replace(Vec::new(), false)
        .await
        .unwrap();
    let restarted = engine(f.engine.factory_snapshot());
    let report = restarted.important_dates(None).await.unwrap();
    assert_eq!(report.entries[0].state, DateState::Unknown);
    assert_eq!(report.entries[0].observation.expires_at, expired.expires_at);
    assert_eq!(report.entries[0].milestone, Some(RenewalMilestone::Expired));
    restarted
        .credential_expiries
        .replace(
            vec![dated("credential", Utc::now() + chrono::Duration::days(90))],
            true,
        )
        .await
        .unwrap();
    assert_eq!(
        restarted.important_dates(None).await.unwrap().entries[0].milestone,
        None
    );
}

#[tokio::test]
async fn instance_scope_and_global_declarations_never_share_an_inbox_identity() {
    let f = fixture();
    let mut snapshot = f.engine.factory_snapshot();
    snapshot.config.scopes[0].name = "instance".into();
    snapshot.config.scope = Some(snapshot.config.scopes[0].clone());
    let engine = engine(snapshot);
    write(&f.root.join(".factory/config.yaml"), "renewals: [{name: licence, kind: licence, expires: 2020-01-01}]\nscope:\n renewals: [{name: licence, kind: licence, expires: 2020-01-01}]\n");
    let report = engine.important_dates(None).await.unwrap();
    assert_eq!(report.entries.len(), 2);
    assert_ne!(
        report.entries[0].observation.id,
        report.entries[1].observation.id
    );
    assert_ne!(
        declaration_id(Some("a:b"), "c"),
        declaration_id(Some("a"), "b:c")
    );
}

#[tokio::test]
async fn separate_scope_file_rejects_misplaced_root_renewals_and_retains_its_last_good_section() {
    let f = fixture();
    let child = f.root.join("child");
    std::fs::create_dir_all(child.join(".factory")).unwrap();
    let mut snapshot = f.engine.factory_snapshot();
    snapshot.config.scope = None;
    snapshot.config.scopes[0].path = child.clone();
    let engine = engine(snapshot);
    let path = child.join(".factory/config.yaml");
    write(
        &path,
        "scope:\n renewals: [{name: licence, kind: licence, expires: 2020-01-01}]\n",
    );
    let before = engine.important_dates(None).await.unwrap();
    assert_eq!(before.entries.len(), 1);
    assert!(before.observation_issues.is_empty());
    write(&path, "renewals: [{name: misplaced, kind: licence, expires: 2027-01-01}]\nscope:\n renewals: []\n");
    let after = engine.important_dates(None).await.unwrap();
    assert_eq!(after.entries.len(), 1);
    assert_eq!(
        after.entries[0].observation.id,
        before.entries[0].observation.id
    );
    assert_eq!(after.entries[0].milestone, Some(RenewalMilestone::Expired));
    assert_eq!(after.observation_issues.len(), 1);
}

#[tokio::test]
async fn missing_expiry_metadata_does_not_silently_renew_an_expired_dependency() {
    let f = fixture();
    let expired = dated("provider", Utc::now() - chrono::Duration::days(1));
    f.engine
        .credential_expiries
        .replace(vec![expired.clone()], true)
        .await
        .unwrap();
    let mut unknown = expired.clone();
    unknown.expires_at = None;
    unknown.basis = DateBasis::Unknown;
    f.engine
        .credential_expiries
        .replace(vec![unknown], true)
        .await
        .unwrap();
    let report = f.engine.important_dates(None).await.unwrap();
    assert_eq!(report.entries[0].observation.expires_at, expired.expires_at);
    assert_eq!(report.entries[0].state, DateState::Unknown);
    assert_eq!(report.entries[0].milestone, Some(RenewalMilestone::Expired));
    let mut renewed = expired;
    renewed.expires_at = Some(Utc::now() + chrono::Duration::days(90));
    f.engine
        .credential_expiries
        .replace(vec![renewed], true)
        .await
        .unwrap();
    assert!(f.engine.important_dates(None).await.unwrap().entries[0]
        .milestone
        .is_none());
}

#[tokio::test]
async fn rare_push_receipts_survive_restart_and_a_new_expiry_rearms_them() {
    let mut f = fixture();
    let received = f.root.join("alerts.json");
    let command = format!("cat >> '{}'", received.display());
    let hook: RenewalsNotify =
        serde_yaml_ng::from_str(&format!("command: {command:?}\ntimeout: 2s\n")).unwrap();
    let mut snapshot = f.engine.factory_snapshot();
    snapshot.config.renewals_notify = Some(hook);
    f.engine = engine(snapshot.clone());
    let mut expired = dated("credential", Utc::now() - chrono::Duration::days(1));
    f.engine
        .credential_expiries
        .replace(vec![expired.clone()], true)
        .await
        .unwrap();
    let report = f.engine.important_dates(None).await.unwrap();
    push(&f.engine, &report).await.unwrap();
    let first = std::fs::read(&received).unwrap();
    push(&f.engine, &report).await.unwrap();
    assert_eq!(std::fs::read(&received).unwrap(), first);
    let restarted = engine(snapshot);
    push(&restarted, &restarted.important_dates(None).await.unwrap())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&received).unwrap(), first);
    expired.expires_at = Some(Utc::now() + chrono::Duration::days(5));
    restarted
        .credential_expiries
        .replace(vec![expired.clone()], true)
        .await
        .unwrap();
    push(&restarted, &restarted.important_dates(None).await.unwrap())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(&received).unwrap(),
        first,
        "seven-day/lead alerts do not push"
    );
    expired.expires_at = Some(Utc::now() + chrono::Duration::hours(20));
    restarted
        .credential_expiries
        .replace(vec![expired], true)
        .await
        .unwrap();
    push(&restarted, &restarted.important_dates(None).await.unwrap())
        .await
        .unwrap();
    assert!(
        std::fs::read(&received).unwrap().len() > first.len(),
        "a new date at one day gets one fresh push"
    );
}

#[tokio::test]
async fn a_metadata_probe_never_resolves_the_declared_credential_source() {
    let f = fixture();
    let cli = f.root.join("openshell");
    let source = f.root.join("credential-source");
    script(
        &source,
        &format!(
            "#!/bin/sh\ntouch '{}'\necho must-not-be-read\n",
            f.root.join("source-was-read").display()
        ),
    );
    script(&cli, "#!/bin/sh\ncase \"$1 $2\" in\n'gateway list') echo '[]';;\n*) echo '{\"providers\":[{\"name\":\"factory-claude-a1b2c3d4\",\"type\":\"claude-code-oauth\",\"credentials\":{\"secret\":\"must-not-be-kept\"},\"credential_expires_at_ms\":{\"token\":3813523200000}}]}';;\nesac\n");
    let mut snapshot = f.engine.factory_snapshot();
    snapshot.config.scopes[0].agents[0].sandbox = factory_core::config::Sandbox::Openshell;
    snapshot.config.scopes[0].agents[0].openshell = Some(serde_yaml_ng::from_str(&format!("cli: {}\nimage: custom-image\nproviders:\n - name: factory-claude\n   type: claude-code-oauth\n   credential: {{from: command, run: '{}'}}\npolicy: {{}}\n", cli.display(), source.display())).unwrap());
    let unavailable = f.root.join("no-tool").display().to_string();
    let tools = probes::Tools {
        openssl: unavailable.clone(),
        github: unavailable.clone(),
        tailscale: unavailable,
        openshell: Some(cli.display().to_string()),
        metadata_home: f.root.clone(),
        config_home: f.root.join(".config"),
        enabled: true,
    };
    let observed = probes::observe(&snapshot, &tools, Utc::now()).await;
    assert!(!f.root.join("source-was-read").exists());
    assert!(observed
        .credentials
        .iter()
        .any(|entry| entry.basis == DateBasis::Observed
            && entry
                .affects
                .iter()
                .any(|dependency| dependency.agent.as_deref() == Some("curator"))));
    assert!(!serde_json::to_string(&observed.credentials)
        .unwrap()
        .contains("must-not-be-kept"));
}

#[tokio::test]
async fn native_attestations_and_cra_keep_their_own_validity_and_fulfillment() {
    use crate::access::Caller;
    use factory_core::{
        intake::{Intake, IntakeSource, IntakeStage, SecurityVerdict, SourceKind},
        policy::{Attestation, ControlRef},
        reporting_clock::{ClockDeadlineKind, ClockItemRef, ClockMark},
    };
    let f = fixture();
    let now = Utc::now();
    let stale = Attestation {
        id: "old".into(),
        control: ControlRef::new("test", "expiry"),
        scope: "demo".into(),
        evidence: "knowledge:receipt".into(),
        note: None,
        attested_by: "owner".into(),
        attested_at: now - chrono::Duration::days(60),
        expires_at: now - chrono::Duration::days(1),
        withdrawn: None,
        clock: None,
        corrective: None,
    };
    f.engine.policies.append_attestation(&stale).await.unwrap();
    let first = f.engine.important_dates(None).await.unwrap();
    let date = first
        .entries
        .iter()
        .find(|date| date.observation.id == "policy:old")
        .unwrap();
    assert_eq!(date.state, DateState::Overdue);
    assert!(!date.resolved);
    assert!(date.milestone.is_none());
    assert_eq!(date.href, "#demo/policy/test%2Fexpiry");
    let mut fresh = stale.clone();
    fresh.id = "fresh".into();
    fresh.expires_at = now + chrono::Duration::days(40);
    f.engine.policies.append_attestation(&fresh).await.unwrap();
    assert!(
        f.engine
            .important_dates(None)
            .await
            .unwrap()
            .entries
            .iter()
            .find(|date| date.observation.id == "policy:old")
            .unwrap()
            .resolved
    );
    let intake = Intake {
        stage: IntakeStage::Received,
        source: Box::new(IntakeSource {
            kind: SourceKind::Github,
            reference: Some("https://github.com/example/repo/issues/9".into()),
            provider: None,
            relayed_by: None,
            repository: None,
            number: None,
            external_id: None,
        }),
        requester: "reporter".into(),
        received_at: now - chrono::Duration::hours(30),
        triage: None,
        triage_task: None,
        questions: vec![],
        decision: None,
        candidates: vec![],
        security: None,
        outbound: None,
    };
    let task = f
        .engine
        .receive_intake(
            NewTask {
                title: "confirmed security issue".into(),
                scope: Some("demo".into()),
                ..Default::default()
            },
            intake,
        )
        .await
        .unwrap();
    f.engine
        .intake_flag_security(&Caller::Owner, &task.id, "upstream evidence")
        .await
        .unwrap();
    f.engine
        .intake_security_decision(&Caller::Owner, &task.id, SecurityVerdict::Confirm, "")
        .await
        .unwrap();
    let before = f.engine.policy_clock(None).await.unwrap();
    let report = f.engine.important_dates(None).await.unwrap();
    let cra: Vec<_> = report
        .entries
        .iter()
        .filter(|date| date.observation.source == DateSource::CraDeadline)
        .collect();
    assert!(!cra.is_empty());
    assert!(cra
        .iter()
        .all(|date| date.href == "#demo/policy/cra%2Fart-14" && date.milestone.is_none()));
    assert!(cra
        .iter()
        .any(|date| date.state == DateState::Overdue && !date.resolved));
    let mut submission = stale;
    submission.id = "submission".into();
    submission.control = ControlRef::new("cra", "art-14");
    // Native CRA submissions remain effective even when attestation expiry
    // is past. The ledger must not invent a renewal against that clock.
    submission.clock = Some(ClockMark {
        item: ClockItemRef::Report {
            item: task.id.clone(),
        },
        deadline: ClockDeadlineKind::EarlyWarning,
    });
    f.engine
        .policies
        .append_attestation(&submission)
        .await
        .unwrap();
    let after = f.engine.policy_clock(None).await.unwrap();
    assert_eq!(before.items[0].awareness_at, after.items[0].awareness_at);
    let dates = f.engine.important_dates(None).await.unwrap();
    assert!(dates
        .entries
        .iter()
        .any(|date| date.observation.source == DateSource::CraDeadline
            && date.observation.name.contains("early")
            && date.resolved));
    assert!(!dates
        .entries
        .iter()
        .any(|date| date.observation.id == "policy:submission"));
    assert_eq!(
        f.engine.policies.all().await.unwrap().len(),
        3,
        "GETs never copy dates or manufacture evidence"
    );
}

#[tokio::test]
async fn default_discovery_reads_real_public_tls_notafter_and_only_stats_claude_source() {
    use std::process::{Command, Stdio};
    let f = fixture();
    let public = f.root.join(".config/openshell/gateways/default/mtls");
    std::fs::create_dir_all(&public).unwrap();
    let cert = public.join("tls.crt");
    let key = public.join("tls.key");
    let generated = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "45",
            "-subj",
            "/CN=dates-fixture",
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(generated.success());
    std::fs::copy(&cert, public.join("ca.crt")).unwrap();
    let expected = Command::new("openssl")
        .args(["x509", "-in"])
        .arg(&cert)
        .args(["-noout", "-enddate"])
        .output()
        .unwrap();
    let expected = std::str::from_utf8(&expected.stdout)
        .unwrap()
        .trim()
        .strip_prefix("notAfter=")
        .unwrap();
    let expected = chrono::NaiveDateTime::parse_from_str(expected, "%b %e %H:%M:%S %Y GMT")
        .unwrap()
        .and_utc();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let server = Command::new("openssl")
        .args([
            "s_server",
            "-accept",
            &port.to_string(),
            "-www",
            "-quiet",
            "-cert",
        ])
        .arg(&cert)
        .arg("-key")
        .arg(&key)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut server = Server(server);
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        server.0.try_wait().unwrap().is_none(),
        "fixture TLS server failed to start"
    );
    let openssl = f.root.join("openssl-fixture");
    script(&openssl, &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif test \"$1\" = s_client; then exec openssl s_client -connect 127.0.0.1:{port} -servername dates-fixture -showcerts; fi\nexec openssl \"$@\"\n", f.root.join("openssl-args").display()));
    let tailscale = f.root.join("tailscale-fixture");
    script(&tailscale, "#!/bin/sh\ncase \"$1\" in\nstatus) echo '{\"Self\":{\"DNSName\":\"fixture.ts.net.\"}}';;\nserve) echo '{\"Web\":{}}';;\nesac\n");
    let cli = f.root.join("openshell-fixture");
    script(&cli, "#!/bin/sh\ncase \"$1 $2\" in\n'gateway list') echo '[{\"name\":\"default\",\"endpoint\":\"https://gateway.fixture:8443\",\"active\":true}]';;\n*) echo '{\"providers\":[{\"name\":\"factory-claude\",\"type\":\"claude-code-oauth\",\"credential_expires_at_ms\":{}}]}';;\nesac\n");
    let token = f.root.join(".config/factory/secrets/claude-oauth-token");
    std::fs::create_dir_all(token.parent().unwrap()).unwrap();
    write(&token, "dummy-secret-never-read");
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0)).unwrap();
    let issued: DateTime<Utc> = std::fs::metadata(&token)
        .unwrap()
        .modified()
        .unwrap()
        .into();
    let github = f.root.join("github-fixture");
    script(&github, "#!/bin/sh\nprintf 'HTTP/2.0 200 OK\\n\\n{}\\n'\n");
    let tools = probes::Tools {
        openssl: openssl.display().to_string(),
        tailscale: tailscale.display().to_string(),
        github: github.display().to_string(),
        openshell: Some(cli.display().to_string()),
        metadata_home: f.root.clone(),
        config_home: f.root.join(".config"),
        enabled: true,
    };
    // No renewal or environment config. Default Tailscale + gateway discovery
    // must find public certs and observe the actual fixture TLS certificate.
    let observed = probes::observe(&f.engine.factory_snapshot(), &tools, Utc::now()).await;
    assert!(observed.infrastructure_complete && observed.credentials_complete);
    for id in [
        "tls:fixture.ts.net:8790",
        "tls:fixture.ts.net:8791",
        "tls:gateway.fixture:8443",
        "openshell-cert:default:ca.crt",
        "openshell-cert:default:tls.crt",
    ] {
        let date = observed
            .infrastructure
            .iter()
            .find(|date| date.id == id)
            .unwrap();
        assert_eq!(date.expires_at, Some(expected), "{id}: {:?}", date.issue);
        assert_eq!(date.basis, DateBasis::Observed);
    }
    let claude = observed
        .credentials
        .iter()
        .find(|date| date.id == "openshell:default:factory-claude")
        .unwrap();
    assert_eq!(
        claude.expires_at,
        Some(issued + chrono::Duration::days(365))
    );
    assert_eq!(claude.basis, DateBasis::Derived);
    assert!(
        observed
            .credentials
            .iter()
            .find(|date| date.id == "github-token")
            .unwrap()
            .no_expiry
    );
    let json = serde_json::to_string(&(observed.infrastructure, observed.credentials)).unwrap();
    assert!(!json.contains("dummy-secret-never-read") && !json.contains("PRIVATE KEY"));
    let args = std::fs::read_to_string(f.root.join("openssl-args")).unwrap();
    assert!(
        !args.contains("tls.key"),
        "the ledger never asks a tool to read the private key"
    );
    let mut snapshot = f.engine.factory_snapshot();
    snapshot.config.scopes[0].agents[0].sandbox = factory_core::config::Sandbox::Openshell;
    snapshot.config.scopes[0].agents[0].openshell = Some(serde_yaml_ng::from_str(&format!("cli: {}\ngateway: default\nimage: custom-image\nproviders: [factory-claude]\npolicy: {{}}\n", cli.display())).unwrap());
    let bound = probes::observe(&snapshot, &tools, Utc::now()).await;
    for id in [
        "tls:gateway.fixture:8443",
        "openshell-cert:default:ca.crt",
        "openshell:default:factory-claude",
    ] {
        let date = bound
            .infrastructure
            .iter()
            .chain(&bound.credentials)
            .find(|date| date.id == id)
            .unwrap();
        assert!(
            date.affects
                .iter()
                .any(|dependency| dependency.scope.as_deref() == Some("demo")
                    && dependency.agent.as_deref() == Some("curator")),
            "{id} must warn its actual gateway dependant"
        );
    }
    // A hostile public filename cannot redirect metadata discovery into a
    // private key. Existing expiry remains last-known on that failed source.
    std::fs::remove_file(public.join("ca.crt")).unwrap();
    std::os::unix::fs::symlink(&key, public.join("ca.crt")).unwrap();
    let observed = probes::observe(&f.engine.factory_snapshot(), &tools, Utc::now()).await;
    assert!(observed
        .infrastructure
        .iter()
        .find(|date| date.id == "openshell-cert:default:ca.crt")
        .unwrap()
        .issue
        .is_some());
}
