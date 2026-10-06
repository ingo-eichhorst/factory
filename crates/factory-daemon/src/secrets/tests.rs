//! `#244` end to end through `Engine::handle_request`: an instance on disk
//! with the issue's two catalogue entries over dummy sources, a scope whose
//! sandboxed agent names one by reference and keeps one inline, and the
//! values pinned to appear nowhere Factory writes or answers.

use super::*;
use factory_core::config::{CONFIG_FILE, FACTORY_DIR};
use factory_core::protocol::{Payload, Request, Response};
use factory_core::secrets::ExpiryState;
use factory_plugins::{Registry, SqliteStore};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const FILE_VALUE: &str = "fake-file-value-0f9e8d";
const COMMAND_VALUE: &str = "fake-gh-value-7a6b5c";
const INLINE_VALUE: &str = "fake-inline-value-3c2d1e";
const SUFFIX: &str = "a1b2c3d4";

struct Instance(PathBuf);

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Instance {
    fn root_file(&self) -> PathBuf {
        self.0.join(FACTORY_DIR).join(CONFIG_FILE)
    }
    fn root_text(&self) -> String {
        std::fs::read_to_string(self.root_file()).unwrap()
    }
}

fn private(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// The instance root declares the catalogue; `awesome-herdr`'s curator
/// names `claude-oauth-token` by reference, keeps one credential inline,
/// and -- when `provider_expires` -- repeats the secret's date on its own
/// provider.
fn instance(provider_expires: Option<&str>) -> (Instance, Arc<Engine>) {
    let dir = std::env::temp_dir().join(format!("factory-secrets-{}", uuid::Uuid::new_v4().simple()));
    let token = dir.join("secrets/claude-oauth-token");
    private(&token, &format!("{FILE_VALUE}\n"));
    private(&dir.join("secrets/inline"), INLINE_VALUE);
    // A command source whose text never spells its value.
    private(&dir.join("secrets/gh"), COMMAND_VALUE);
    let root = format!(
        "version: 1\n\
         instance: {{ id: {SUFFIX}-0000-4000-8000-000000000000, name: test }}\n\
         # The catalogue (#244).\n\
         secrets:\n\
         \x20 - name: claude-oauth-token\n\
         \x20   kind: token\n\
         \x20   source: {{ from: file, path: {token} }}\n\
         \x20   expires: 2027-10-04\n\
         \x20   renew: \"claude setup-token, then (umask 077; cat > {token})\"\n\
         \x20 - name: github-gh-login\n\
         \x20   kind: token\n\
         \x20   source: {{ from: command, run: \"cat {gh}\" }}\n\
         \x20   expires: never\n\
         \x20   renew: \"gh auth login\"\n\
         \n\
         # Who runs here.\n\
         roles: {{}}\n",
        token = token.display(),
        gh = dir.join("secrets/gh").display()
    );
    std::fs::create_dir_all(dir.join(FACTORY_DIR)).unwrap();
    std::fs::write(dir.join(FACTORY_DIR).join(CONFIG_FILE), root).unwrap();
    let expires = provider_expires.map(|d| format!("\n          expires: {d}")).unwrap_or_default();
    let scope = format!(
        "version: 1\n\
         scope:\n\
         \x20 id: awesome-id\n\
         \x20 name: awesome-herdr\n\
         \x20 agent:\n\
         \x20   name: awesome-herdr-curator\n\
         \x20   harness: claude-code\n\
         \x20   sandbox: openshell\n\
         \x20   openshell:\n\
         \x20     providers:\n\
         \x20       - name: factory-claude\n\
         \x20         type: claude-code-oauth\n\
         \x20         credential: {{ secret: claude-oauth-token }}{expires}\n\
         \x20       - name: factory-legacy\n\
         \x20         type: generic\n\
         \x20         credential: {{ from: file, path: {inline} }}\n\
         \x20     policy: {{}}\n",
        inline = dir.join("secrets/inline").display()
    );
    let at = dir.join("projects/awesome-herdr").join(FACTORY_DIR);
    std::fs::create_dir_all(&at).unwrap();
    std::fs::write(at.join(CONFIG_FILE), scope).unwrap();

    let mut factory = factory_core::config::Factory::load(&dir).unwrap();
    crate::discovery::apply(&mut factory).unwrap();
    factory.config.validate().unwrap();
    let engine = Arc::new(Engine::new(
        factory,
        Registry::with_builtins(),
        Arc::new(SqliteStore::in_memory().unwrap()),
        PathBuf::from("factory"),
        vec![],
    ));
    (Instance(dir), engine)
}

async fn environment(engine: &Arc<Engine>) -> (Vec<SecretRow>, Vec<UndeclaredCredential>, Vec<SecretChange>, String) {
    match engine.handle_request(Request::Environment).await {
        Response::Ok { data } => {
            let json = serde_json::to_string(&data).unwrap();
            let Payload::Environment { secrets, undeclared, secret_changes, .. } = data else { panic!("wrong payload") };
            (secrets, undeclared, secret_changes, json)
        }
        other => panic!("{other:?}"),
    }
}

fn assert_no_value(what: &str, text: &str) {
    for value in [FILE_VALUE, COMMAND_VALUE, INLINE_VALUE] {
        assert!(!text.contains(value), "a secret value in {what}: {text}");
    }
}

#[tokio::test]
async fn the_tab_shows_both_entries_with_presence_resolution_expiry_and_users_and_the_inline_one_as_undeclared() {
    let (_dir, engine) = instance(None);
    let (rows, _, _, _) = environment(&engine).await;
    assert_eq!(rows[0].resolves, None, "not checked until the provisioner's first pass");

    engine.l2_service().check_catalogue().await;
    let (rows, undeclared, changes, json) = environment(&engine).await;
    assert_no_value("the Environment payload", &json);
    assert!(changes.is_empty());

    let claude = &rows[0];
    assert_eq!(claude.name, "claude-oauth-token");
    assert_eq!((claude.from.as_str(), claude.present, claude.owner_only, claude.resolves), ("file", Some(true), Some(true), Some(true)));
    let left = (chrono::NaiveDate::from_ymd_opt(2027, 10, 4).unwrap() - Utc::now().date_naive()).num_days();
    assert_eq!(claude.days_left, Some(left));
    assert_eq!(claude.expires.unwrap().to_string(), "2027-10-04");
    assert!(claude.renew.as_deref().unwrap().starts_with("claude setup-token"));
    assert_eq!(
        claude.used_by,
        [SecretUse { scope: "awesome-herdr".into(), agent: "awesome-herdr-curator".into(), provider: format!("factory-claude-{SUFFIX}") }]
    );

    let github = &rows[1];
    assert_eq!((github.from.as_str(), github.present, github.resolves), ("command", None, Some(true)));
    assert_eq!((github.state, github.days_left), (ExpiryState::Never, None));
    assert!(github.used_by.is_empty());

    assert_eq!(undeclared.len(), 1);
    assert_eq!(undeclared[0].provider, format!("factory-legacy-{SUFFIX}"));
    assert_eq!((undeclared[0].scope.as_str(), undeclared[0].agent.as_str()), ("awesome-herdr", "awesome-herdr-curator"));
    assert_eq!(undeclared[0].state, ExpiryState::Unknown);

    // A source that stops resolving says why, and still never what.
    std::fs::set_permissions(_dir.0.join("secrets/claude-oauth-token"), std::fs::Permissions::from_mode(0o644)).unwrap();
    engine.l2_service().check_catalogue().await;
    let (rows, _, _, json) = environment(&engine).await;
    assert_eq!((rows[0].owner_only, rows[0].resolves), (Some(false), Some(false)));
    assert!(rows[0].reason.as_deref().unwrap().contains("chmod 600"), "{:?}", rows[0].reason);
    assert_no_value("the Environment payload", &json);
}

#[tokio::test]
async fn changing_the_date_writes_the_root_config_journals_who_and_raises_the_inbox_item_at_once() {
    let (dir, engine) = instance(None);
    let mut events = engine.shared.bus.subscribe();
    let soon = Utc::now().date_naive() + chrono::Duration::days(10);
    let metadata = SecretMetadata {
        expires: Some(Expiry::On(soon)),
        renew: Some("claude setup-token".into()),
        note: Some("the curator's subscription token".into()),
    };
    let row = match engine.handle_request(Request::SecretSet { name: "claude-oauth-token".into(), metadata }).await {
        Response::Ok { data: Payload::Secret { secret } } => secret,
        other => panic!("{other:?}"),
    };
    assert_eq!((row.state, row.days_left), (ExpiryState::DueSoon, Some(10)));

    // The root config: those three keys of that one entry, nothing else.
    let text = dir.root_text();
    assert!(text.contains(&format!("    expires: {}\n    renew: claude setup-token\n    note: the curator's subscription token\n", soon.format("%Y-%m-%d"))), "{text}");
    for kept in ["# The catalogue (#244).", "# Who runs here.", "secrets/gh\"", "expires: never"] {
        assert!(text.contains(kept), "{kept} lost: {text}");
    }
    assert_eq!(engine.factory_snapshot().config.secrets[0].expires, Some(Expiry::On(soon)), "live without a restart");

    assert!(
        matches!(events.try_recv(), Ok(factory_core::event::Event::ImportantDatesUpdated { .. })),
        "the ledger, its Inbox items and its push hook are told now"
    );

    // Journaled with who changed what.
    let entries = engine.l4.store.entries(SECRETS_JOURNAL, 10).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind, SECRET_CHANGED);
    assert!(entries[0].message.contains(&format!("expires 2027-10-04 -> {}", soon.format("%Y-%m-%d"))), "{}", entries[0].message);
    assert!(entries[0].message.ends_with("by owner") || entries[0].message.contains("by the owner") || entries[0].message.contains("by owner"), "{}", entries[0].message);
    let (_, _, changes, _) = environment(&engine).await;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].secret, "claude-oauth-token");

    // The Inbox has it now: the Important dates ledger reads the catalogue
    // (#236), and a date inside the 30-day lead is a milestone at once,
    // naming the agent that stops working.
    let report = engine.important_dates(None).await.unwrap();
    let entry = report
        .entries
        .iter()
        .find(|e| e.observation.id == "secret:claude-oauth-token")
        .expect("every declared secret is a ledger entry");
    assert!(entry.milestone.is_some() && !entry.resolved, "{entry:?}");
    assert_eq!(entry.observation.source, factory_core::renewals::DateSource::Secret);
    assert_eq!(entry.observation.renew, "claude setup-token");
    assert_eq!(entry.href, "#all/secrets");
    let affects: Vec<&str> = entry.observation.affects.iter().map(|d| d.label.as_str()).collect();
    assert_eq!(affects, [format!("awesome-herdr / awesome-herdr-curator / factory-claude-{SUFFIX}")]);
    let github = report.entries.iter().find(|e| e.observation.id == "secret:github-gh-login").unwrap();
    assert!(github.observation.no_expiry && github.milestone.is_none(), "{github:?}");
    // One entry per secret: the ledger has no second, per-provider copy.
    assert_eq!(report.entries.iter().filter(|e| e.observation.id.contains("factory-claude")).count(), 0);

    // Acceptance 5: no value anywhere this wrote or answered.
    engine.l2_service().check_catalogue().await;
    assert_no_value("the root config", &dir.root_text());
    assert_no_value("the journal", &serde_json::to_string(&entries).unwrap());
    assert_no_value("Important dates", &serde_json::to_string(&report).unwrap());
    assert_no_value("the Environment payload", &environment(&engine).await.3);
}

#[tokio::test]
async fn an_unchanged_write_writes_and_journals_nothing_and_an_unknown_name_is_refused() {
    let (dir, engine) = instance(None);
    let before = dir.root_text();
    let same = engine.factory_snapshot().config.secrets[1].metadata();
    assert!(matches!(
        engine.handle_request(Request::SecretSet { name: "github-gh-login".into(), metadata: same }).await,
        Response::Ok { .. }
    ));
    assert_eq!(dir.root_text(), before);
    assert!(engine.l4.store.entries(SECRETS_JOURNAL, 10).await.unwrap().is_empty());

    match engine.handle_request(Request::SecretSet { name: "nope".into(), metadata: SecretMetadata::default() }).await {
        Response::Error { message, .. } => assert!(message.contains("claude-oauth-token, github-gh-login"), "{message}"),
        other => panic!("{other:?}"),
    }
    let two_lines = SecretMetadata { note: Some("a\nb".into()), ..Default::default() };
    assert!(matches!(engine.handle_request(Request::SecretSet { name: "github-gh-login".into(), metadata: two_lines }).await, Response::Error { .. }));
    assert_eq!(dir.root_text(), before);
}

#[tokio::test]
async fn a_date_that_would_disagree_with_a_providers_own_expires_is_refused_and_nothing_is_written() {
    let (dir, engine) = instance(Some("2027-10-04"));
    let before = dir.root_text();
    let later = SecretMetadata { expires: Some("2028-10-04".parse().unwrap()), ..Default::default() };
    match engine.handle_request(Request::SecretSet { name: "claude-oauth-token".into(), metadata: later }).await {
        Response::Error { message, .. } => {
            assert!(message.contains("expires: 2027-10-04") && message.contains("expires: 2028-10-04"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(dir.root_text(), before);
    assert!(engine.l4.store.entries(SECRETS_JOURNAL, 10).await.unwrap().is_empty());
}

#[test]
fn change_words_name_each_field_that_moved() {
    let before = SecretMetadata { expires: Some(Expiry::Never), renew: Some("a".into()), note: None };
    let after = SecretMetadata { expires: None, renew: None, note: Some("n".into()) };
    assert_eq!(change_words(&before, &after), "expires never -> unset; renew removed; note set to \"n\"");
}
