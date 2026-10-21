//! Scope pages (`#343`): L4's publish and rollback commands, and the read
//! that serves one version's content. The store, the CSP preflight and the
//! `ScopePageFact` provider are `factory_process::pages`; this is the part
//! that knows who is asking -- which scope a name means, whether that scope
//! declares the page, the size limit, and the journal entry on the run.
//!
//! One handler for every way in: the CLI over the socket, `POST /api/rpc`
//! from a sandboxed run and `POST /api/pages/publish` all arrive as
//! `Request::PagePublish` through `Engine::handle`, so `access.rs` has
//! already checked the caller's grant and scope before anything here runs.

use crate::access::Caller;
use crate::l4_service::L4Service;
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::{PageContentBody, PagePart};
use factory_core::task::TaskEntry;
use factory_kernel::PageVersionFact;
use factory_process::pages::{csp_findings, NewPageVersion, PageDecl};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;
use tokio::sync::Mutex;

use crate::engine::Engine;

/// The calendar is the only scope page allowed to edit its source tracker.
/// Keep the write narrow: a known conference edition and one enumerated status.
const RADAR_SCOPE: &str = "conference-radar";
const RADAR_PAGE: &str = "conference-radar";
const RADAR_TRACKER: &str = "data/ingo-conference-radar.json";

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ConferenceChoice {
    pub scope: String,
    pub name: String,
    pub conference: String,
    pub edition_year: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConferenceChoiceRow {
    pub conference: String,
    pub edition_year: String,
    pub status: String,
}

fn choice_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn choice_status(value: &str) -> Option<&'static str> {
    match value {
        "unreviewed" => Some("watchlist"),
        "not_interested" => Some("not interested"),
        "not_this_time" => Some("not this time"),
        "interested" => Some("interested"),
        "applied" => Some("submitted/applied"),
        "accepted" => Some("accepted/participating"),
        "waitlisted" => Some("waitlisted"),
        "rejected" => Some("rejected"),
        "withdrawn" => Some("withdrawn"),
        _ => None,
    }
}

fn display_status(value: &str) -> &'static str {
    match value {
        "not interested" => "not_interested",
        "not this time" => "not_this_time",
        "interested" => "interested",
        "submitted/applied" => "applied",
        "accepted/participating" => "accepted",
        "waitlisted" => "waitlisted",
        "rejected" | "rejected/declined" => "rejected",
        "withdrawn" => "withdrawn",
        _ => "unreviewed",
    }
}

fn edition_year(entry: &Value) -> String {
    if let Some(start) = entry.get("event_start_date").and_then(Value::as_str) {
        if let Some(year) = start.get(..4).filter(|year| year.bytes().all(|b| b.is_ascii_digit())) {
            return year.to_string();
        }
    }
    let dates = entry
        .get("event_dates")
        .and_then(Value::as_str)
        .unwrap_or_default();
    dates
        .as_bytes()
        .windows(4)
        .find(|part| part.starts_with(b"20") && part.iter().all(u8::is_ascii_digit))
        .map(|part| String::from_utf8_lossy(part).to_string())
        .unwrap_or_else(|| "undated".into())
}

fn tracker_rows(tracker: &Value) -> Result<Vec<ConferenceChoiceRow>> {
    let entries = tracker
        .get("conferences")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            FactoryError::BadRequest("the conference tracker has no conferences array".into())
        })?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let conference = entry.get("name")?.as_str()?.to_string();
            Some(ConferenceChoiceRow {
                conference,
                edition_year: edition_year(entry),
                status: display_status(
                    entry
                        .get("application_status")
                        .and_then(Value::as_str)
                        .unwrap_or("watchlist"),
                )
                .into(),
            })
        })
        .collect())
}

fn apply_choice(tracker: &mut Value, choice: &ConferenceChoice, status: &str) -> Result<bool> {
    let entries = tracker
        .get_mut("conferences")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            FactoryError::BadRequest("the conference tracker has no conferences array".into())
        })?;
    let matching: Vec<_> = entries
        .iter_mut()
        .filter(|entry| {
            entry.get("name").and_then(Value::as_str) == Some(choice.conference.as_str())
                && edition_year(entry) == choice.edition_year
        })
        .collect();
    if matching.len() != 1 {
        return Err(FactoryError::BadRequest(
            "conference edition was not found uniquely in the tracker".into(),
        ));
    }
    let entry = &mut *matching.into_iter().next().unwrap();
    if entry.get("application_status").and_then(Value::as_str) == Some(status) {
        return Ok(false);
    }
    entry["application_status"] = Value::String(status.into());
    entry["status_source"] = Value::String("dashboard user".into());
    entry["status_updated_at"] = Value::String(chrono::Utc::now().to_rfc3339());
    Ok(true)
}

async fn read_tracker(path: &Path) -> Result<Value> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("reading {}: {e}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("parsing {}: {e}", path.display())))
}

impl Engine {
    fn radar_tracker_path(&self, scope: &str, name: &str) -> Result<std::path::PathBuf> {
        if scope != RADAR_SCOPE || name != RADAR_PAGE {
            return Err(FactoryError::BadRequest(
                "conference choices are available only for conference-radar".into(),
            ));
        }
        let factory = self.factory_snapshot();
        let found = factory.scope(scope)?;
        if !found.pages.iter().any(|page| page.name == name) {
            return Err(FactoryError::BadRequest(
                "the conference-radar page is not declared".into(),
            ));
        }
        Ok(factory.scope_path(scope)?.join(RADAR_TRACKER))
    }

    pub(crate) async fn conference_choices(
        &self,
        scope: &str,
        name: &str,
    ) -> Result<Vec<ConferenceChoiceRow>> {
        let path = self.radar_tracker_path(scope, name)?;
        let _guard = choice_lock().lock().await;
        tracker_rows(&read_tracker(&path).await?)
    }

    pub(crate) async fn save_conference_choice(
        &self,
        choice: ConferenceChoice,
    ) -> Result<Vec<ConferenceChoiceRow>> {
        let path = self.radar_tracker_path(&choice.scope, &choice.name)?;
        if choice.conference.is_empty() || choice.conference.len() > 250 ||
            !(choice.edition_year == "undated" || choice.edition_year.len() == 4 &&
              choice.edition_year.starts_with("20") && choice.edition_year.bytes().all(|b| b.is_ascii_digit())) {
            return Err(FactoryError::BadRequest("invalid conference edition".into()));
        }
        let status = choice_status(&choice.status)
            .ok_or_else(|| FactoryError::BadRequest("invalid conference choice".into()))?;
        let _guard = choice_lock().lock().await;
        let mut tracker = read_tracker(&path).await?;
        if !apply_choice(&mut tracker, &choice, status)? {
            return tracker_rows(&tracker);
        }
        let today = chrono::Utc::now().format("%Y%m%d").to_string();
        let backup = path.parent().unwrap().join("backups").join(format!(
            "ingo-conference-radar.json.bak-{today}-dashboard-{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(backup.parent().unwrap())
            .await
            .map_err(|e| {
                FactoryError::Other(anyhow::anyhow!("creating tracker backup directory: {e}"))
            })?;
        tokio::fs::copy(&path, &backup)
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("backing up tracker: {e}")))?;
        let mut bytes = serde_json::to_vec_pretty(&tracker)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("serializing tracker: {e}")))?;
        bytes.push(b'\n');
        let temp = path.with_extension(format!("json.{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&temp, bytes)
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("writing tracker: {e}")))?;
        tokio::fs::rename(&temp, &path)
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("replacing tracker: {e}")))?;
        tracker_rows(&tracker)
    }
}


/// At most this many CSP findings are listed in a refusal; the rest are counted.
const FINDINGS_SHOWN: usize = 10;

/// What `Request::PagePublish` carries, past who is asking.
pub(crate) struct PublishPage {
    pub(crate) scope: Option<String>,
    pub(crate) name: String,
    pub(crate) title: Option<String>,
    pub(crate) html: String,
    pub(crate) data: Option<serde_json::Value>,
}

impl L4Service<'_> {
    /// The scope a page request means -- by name or path, canonical -- and
    /// that scope's declaration of the page. A page nobody declares is
    /// refused here, whoever asks: the owner may publish any declared page,
    /// never an undeclared one.
    fn declared_page(&self, scope: &str, name: &str) -> Result<(String, PageDecl)> {
        let factory = self.wiring.snapshot();
        let found = factory.scope(scope)?;
        match found.pages.iter().find(|p| p.name == name) {
            Some(decl) => Ok((found.name.clone(), decl.clone())),
            None if found.pages.is_empty() => Err(FactoryError::BadRequest(format!(
                "scope {:?} declares no pages; declare {name:?} under `scope.pages` in its .factory/config.yaml first",
                found.name
            ))),
            None => Err(FactoryError::BadRequest(format!(
                "scope {:?} declares no page {name:?}; it declares {}",
                found.name,
                found.pages.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
            ))),
        }
    }

    /// Store a new version of a page and make it the one shown. Refused,
    /// with nothing stored, when the scope does not declare it, when it is
    /// over its `max_bytes`, or when its markup loads something the page CSP
    /// would block. Journaled on the run that published it.
    pub(crate) async fn publish_page(&self, caller: &Caller, page: PublishPage) -> Result<(String, PageVersionFact)> {
        let asked = match (&page.scope, caller) {
            (Some(scope), _) => scope.clone(),
            (None, Caller::Agent { scope, .. }) => scope.clone(),
            (None, Caller::Owner) => {
                return Err(FactoryError::BadRequest(
                    "name the scope whose page this is (--scope <scope>); only a run publishes to its own scope by default".into(),
                ))
            }
        };
        let (scope, decl) = self.declared_page(&asked, &page.name)?;
        // `authorize` compared the name the caller gave; this is the name it
        // resolves to, and the one the page is stored under.
        if let Caller::Agent { scope: own, .. } = caller {
            if *own != scope {
                return Err(FactoryError::Denied(format!(
                    "{} works in {own}, not in {scope}; a run publishes only its own scope's pages",
                    caller.describe()
                )));
            }
        }
        if page.html.trim().is_empty() {
            return Err(FactoryError::BadRequest(format!("the page {:?} is empty; nothing was stored", page.name)));
        }
        let data_bytes = match &page.data {
            Some(data) => serde_json::to_vec(data)
                .map_err(|e| FactoryError::BadRequest(format!("the page data is not JSON: {e}")))?
                .len(),
            None => 0,
        };
        let bytes = (page.html.len() + data_bytes) as u64;
        if bytes > decl.max_bytes() {
            return Err(FactoryError::BadRequest(format!(
                "page {scope}/{} is {bytes} bytes with its data, over its max_bytes of {}; nothing was stored. \
                 Raise max_bytes in scope.pages or publish a smaller page",
                page.name,
                decl.max_bytes()
            )));
        }
        let findings = csp_findings(&page.html);
        if !findings.is_empty() {
            let mut listed: Vec<String> = findings.iter().take(FINDINGS_SHOWN).map(|f| format!("- {f}")).collect();
            if findings.len() > FINDINGS_SHOWN {
                listed.push(format!("- and {} more", findings.len() - FINDINGS_SHOWN));
            }
            return Err(FactoryError::BadRequest(format!(
                "page {scope}/{} would not render under its CSP (everything inline or data:, no network); nothing was stored:\n{}",
                page.name,
                listed.join("\n")
            )));
        }

        let run_id = match caller {
            Caller::Agent { run_id: Some(run_id), .. } => Some(run_id.clone()),
            _ => None,
        };
        let task_id = match &run_id {
            Some(run_id) => self.state.store.get_run(run_id).await?.map(|run| run.task_id),
            None => None,
        };
        let title = page
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| decl.title());
        let version = self
            .state
            .pages
            .publish(NewPageVersion {
                scope: scope.clone(),
                name: page.name.clone(),
                title,
                html: page.html,
                data: page.data,
                published_by: caller.describe(),
                run_id: run_id.clone(),
                task_id: task_id.clone(),
                keep: decl.keep(),
            })
            .await?;
        if let (Some(task_id), Some(run_id)) = (&task_id, &run_id) {
            let entry = TaskEntry::new(
                "daemon",
                "page_published",
                format!(
                    "published page {scope}/{} version {} ({} bytes, sha256 {})",
                    page.name,
                    &version.sha256[..12],
                    version.bytes,
                    version.sha256
                ),
            )
            .in_run(run_id.clone())
            .with_data(serde_json::json!({
                "scope": scope,
                "page": page.name,
                "version": version.sha256,
                "sha256": version.sha256,
                "bytes": version.bytes,
                "data_bytes": version.data_bytes,
            }));
            self.entry(task_id, entry).await;
        }
        tracing::info!(scope = %scope, page = %page.name, version = %version.sha256, bytes = version.bytes, "page published");
        Ok((scope, version))
    }

    /// Show an older kept version again. Journaled on the caller's run when
    /// an agent does it from one; a person's rollback from the UI has no run.
    pub(crate) async fn rollback_page(&self, caller: &Caller, scope: &str, name: &str, version: &str) -> Result<String> {
        let (scope, _) = self.declared_page(scope, name)?;
        let shown = self.state.pages.set_current(&scope, name, version).await?;
        if let Caller::Agent { run_id: Some(run_id), .. } = caller {
            if let Some(run) = self.state.store.get_run(run_id).await? {
                let entry = TaskEntry::new(
                    "daemon",
                    "page_rolled_back",
                    format!("page {scope}/{name} shows version {} again", &shown.sha256[..12]),
                )
                .in_run(run_id.clone())
                .with_data(serde_json::json!({"scope": scope, "page": name, "version": shown.sha256}));
                self.entry(&run.task_id, entry).await;
            }
        }
        tracing::info!(scope = %scope, page = %name, version = %shown.sha256, by = %caller.describe(), "page rolled back");
        Ok(scope)
    }

    /// One version's HTML or data, for the http interface to serve.
    pub(crate) async fn page_content(
        &self,
        scope: &str,
        name: &str,
        version: Option<&str>,
        part: PagePart,
    ) -> Result<PageContentBody> {
        let (scope, _) = self.declared_page(scope, name)?;
        let content = self
            .state
            .pages
            .content(&scope, name, version, part == PagePart::Html, part == PagePart::Data)
            .await?;
        Ok(PageContentBody {
            scope,
            name: name.to_string(),
            version: content.version,
            html: content.html,
            data: content.data,
        })
    }
}

#[cfg(test)]
mod tests {
    //! Publishing through `Engine::handle`, the way the CLI, `/api/rpc` and
    //! `/api/pages/publish` all arrive: envelope, caller, authorize, handler.
    use crate::engine::Engine;
    use super::{apply_choice, choice_status, edition_year, tracker_rows, ConferenceChoice};
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::protocol::{Envelope, PagePart, Payload, Request, Response};
    use factory_core::run::{NewRun, Run, Trigger};
    use factory_core::task::NewTask;
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;
    use std::sync::Arc;

    const RADAR: &str = include_str!("../../../examples/pages/conference-radar.html");

    #[test]
    fn conference_choices_update_one_edition_and_preserve_other_records() {
        let mut tracker = serde_json::json!({"conferences": [
            {"name":"DemoConf", "event_start_date":"2026-11-12", "application_status":"watchlist", "note":"keep"},
            {"name":"DemoConf", "event_start_date":"2027-11-12", "application_status":"accepted/participating"}
        ], "notes": ["keep"]});
        let choice = ConferenceChoice {
            scope: "conference-radar".into(),
            name: "conference-radar".into(),
            conference: "DemoConf".into(),
            edition_year: "2026".into(),
            status: "interested".into(),
        };
        assert_eq!(choice_status("interested"), Some("interested"));
        assert_eq!(choice_status("not_this_time"), Some("not this time"));
        assert_eq!(choice_status("malicious"), None);
        assert_eq!(edition_year(&tracker["conferences"][0]), "2026");
        assert!(apply_choice(&mut tracker, &choice, "interested").unwrap());
        assert!(!apply_choice(&mut tracker, &choice, "interested").unwrap());
        assert_eq!(
            tracker["conferences"][0]["application_status"],
            "interested"
        );
        assert_eq!(tracker["conferences"][0]["note"], "keep");
        assert_eq!(
            tracker["conferences"][1]["application_status"],
            "accepted/participating"
        );
        assert_eq!(tracker["notes"][0], "keep");
        assert_eq!(tracker_rows(&tracker).unwrap()[0].status, "interested");
        assert!(apply_choice(&mut tracker, &choice, "not this time").unwrap());
        assert_eq!(tracker_rows(&tracker).unwrap()[0].status, "not_this_time");
        let missing = ConferenceChoice {
            edition_year: "2028".into(),
            ..choice
        };
        assert!(apply_choice(&mut tracker, &missing, "rejected").is_err());
    }

    #[tokio::test]
    async fn conference_choice_write_is_durable_and_backed_up() {
        let base = engine();
        let mut factory = base.factory_snapshot();
        factory.config.scopes[0].name = "conference-radar".into();
        let tracker = factory
            .root
            .join("projects/radar/data/ingo-conference-radar.json");
        std::fs::create_dir_all(tracker.parent().unwrap()).unwrap();
        std::fs::write(&tracker, "{\n  \"conferences\": [{\"name\": \"DemoConf\", \"event_start_date\": \"2026-11-12\", \"application_status\": \"watchlist\"}]\n}\n").unwrap();
        let app = Engine::new(
            factory,
            Registry::with_builtins(),
            base.l4.store.clone(),
            PathBuf::from("factory"),
            Vec::new(),
        );
        let choice = ConferenceChoice {
            scope: "conference-radar".into(),
            name: "conference-radar".into(),
            conference: "DemoConf".into(),
            edition_year: "2026".into(),
            status: "interested".into(),
        };
        assert_eq!(
            app.conference_choices("conference-radar", "conference-radar")
                .await
                .unwrap()[0]
                .status,
            "unreviewed"
        );
        assert_eq!(
            app.save_conference_choice(choice).await.unwrap()[0].status,
            "interested"
        );
        assert_eq!(
            app.conference_choices("conference-radar", "conference-radar")
                .await
                .unwrap()[0]
                .status,
            "interested"
        );
        let backups = tracker.parent().unwrap().join("backups");
        let backup = std::fs::read_dir(backups)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(std::fs::read_to_string(backup)
            .unwrap()
            .contains("\"watchlist\""));
        assert!(std::fs::read_to_string(tracker)
            .unwrap()
            .contains("\"interested\""));
    }

    fn engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-pages-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory")).unwrap();
        let mut radar: Scope = serde_yaml_ng::from_str(
            "id: radar-id\nname: radar\npages:\n  - { name: conference-radar, title: Konferenz-Radar, max_bytes: 200KB, keep: 2 }\n  - { name: roadmap }\n",
        )
        .unwrap();
        radar.path = PathBuf::from("projects/radar");
        let mut other: Scope =
            serde_yaml_ng::from_str("id: other-id\nname: other\npages:\n  - { name: elsewhere }\n").unwrap();
        other.path = PathBuf::from("projects/other");
        let config = Config {
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
            scopes: vec![radar, other],
            ..Default::default()
        };
        config.validate().unwrap();
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(Factory { root, config }, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new()))
    }

    /// A task in `scope` with a run in progress whose token is `token`.
    async fn running(engine: &Engine, scope: &str, token: &str) -> (String, Run) {
        let task = factory_process::store::task_from_new(
            NewTask { title: "render the radar".into(), ..Default::default() },
            scope.into(),
            "shell".into(),
            "herdr".into(),
        );
        let task = engine.l4.store.create(&task).await.unwrap();
        let run = engine
            .l4
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "herdr".into(),
                token: token.into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        (task.id, run)
    }

    async fn send(engine: &Arc<Engine>, token: Option<&str>, request: Request) -> Response {
        engine.handle(Envelope { request, token: token.map(str::to_string) }).await
    }

    fn publish(scope: Option<&str>, name: &str, html: &str, data: Option<serde_json::Value>) -> Request {
        Request::PagePublish { scope: scope.map(str::to_string), name: name.into(), title: None, html: html.into(), data }
    }

    fn refused(response: &Response) -> String {
        match response {
            Response::Error { message, .. } => message.clone(),
            Response::Ok { data } => panic!("expected a refusal, got {data:?}"),
        }
    }

    #[tokio::test]
    async fn a_run_publishes_its_own_scopes_declared_page_and_the_publish_is_journaled_on_the_run() {
        let engine = engine();
        let (task_id, run) = running(&engine, "radar", "tok-radar").await;
        let data = serde_json::json!({"conferences": [{"name": "Lorem DevDays"}]});
        let response = send(&engine, Some("tok-radar"), publish(None, "conference-radar", RADAR, Some(data.clone()))).await;
        let Response::Ok { data: Payload::PageVersion { scope, name, version } } = response else {
            panic!("{response:?}")
        };
        assert_eq!((scope.as_str(), name.as_str()), ("radar", "conference-radar"));
        assert_eq!(version.run_id.as_deref(), Some(run.id.as_str()));
        assert_eq!(version.task_id.as_deref(), Some(task_id.as_str()));
        assert_eq!(version.title, "Konferenz-Radar", "the declared title when none is given");
        let dir = engine.factory_snapshot().factory_dir().join("pages/radar/conference-radar").join(&version.sha256);
        assert!(dir.join("page.html").is_file() && dir.join("data.json").is_file());

        let entries = engine.l4.store.entries(&task_id, 50).await.unwrap();
        let entry = entries.iter().find(|e| e.kind == "page_published").expect("journaled");
        assert_eq!(entry.run_id.as_deref(), Some(run.id.as_str()));
        let recorded = entry.data.as_ref().unwrap();
        assert_eq!(recorded["page"], "conference-radar");
        assert_eq!(recorded["sha256"], version.sha256.as_str());
        assert_eq!(recorded["bytes"], version.bytes);

        // The fact, the content and the data all read back the same version.
        let Response::Ok { data: Payload::Pages { pages } } = send(&engine, None, Request::Pages { scope: Some("radar".into()) }).await else {
            panic!()
        };
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].current.as_ref().unwrap().sha256, version.sha256);
        assert!(pages[1].current.is_none(), "roadmap is declared, not published yet");
        let content = send(
            &engine,
            None,
            Request::PageContent { scope: "radar".into(), name: "conference-radar".into(), version: None, part: PagePart::Data },
        )
        .await;
        let Response::Ok { data: Payload::PageContent { content } } = content else { panic!("{content:?}") };
        assert_eq!(content.data, Some(data));
        assert_eq!(content.html, None, "only the part asked for");
    }

    #[tokio::test]
    async fn an_undeclared_page_or_another_scopes_page_is_refused_and_nothing_is_stored() {
        let engine = engine();
        running(&engine, "radar", "tok-radar").await;
        let undeclared = send(&engine, Some("tok-radar"), publish(None, "not-declared", "<p>x</p>", None)).await;
        let why = refused(&undeclared);
        assert!(why.contains("declares no page \"not-declared\"") && why.contains("conference-radar, roadmap"), "{why}");

        let elsewhere = send(&engine, Some("tok-radar"), publish(Some("other"), "elsewhere", "<p>x</p>", None)).await;
        assert!(matches!(&elsewhere, Response::Error { code, .. } if code == "denied"), "{elsewhere:?}");
        assert!(refused(&elsewhere).contains("works in radar, not in other"));
        assert!(!engine.factory_snapshot().factory_dir().join("pages").join("other").exists());

        // The owner may publish any declared page, and must name its scope.
        let unnamed = send(&engine, None, publish(None, "elsewhere", "<p>x</p>", None)).await;
        assert!(refused(&unnamed).contains("--scope"));
        let owner = send(&engine, None, publish(Some("other"), "elsewhere", "<p>x</p>", None)).await;
        assert!(matches!(owner, Response::Ok { data: Payload::PageVersion { .. } }), "{owner:?}");
    }

    #[tokio::test]
    async fn over_max_bytes_or_against_the_csp_is_refused_before_anything_is_stored() {
        let engine = engine();
        running(&engine, "radar", "tok-radar").await;
        let big = "x".repeat(200_001);
        let why = refused(&send(&engine, Some("tok-radar"), publish(None, "conference-radar", &big, None)).await);
        assert!(why.contains("over its max_bytes of 200000") && why.contains("nothing was stored"), "{why}");
        // The data counts toward the limit too.
        let data = serde_json::json!({"padding": "y".repeat(199_970)}); // under the limit alone, over it with the page
        let why = refused(&send(&engine, Some("tok-radar"), publish(None, "conference-radar", "<p>small</p><p>page</p>", Some(data))).await);
        assert!(why.contains("max_bytes"), "{why}");

        let loads = r#"<script src="https://cdn.example/x.js"></script><img src="https://tile.example/1.png">"#;
        let why = refused(&send(&engine, Some("tok-radar"), publish(None, "conference-radar", loads, None)).await);
        assert!(why.contains("would not render under its CSP"), "{why}");
        assert!(why.contains("cdn.example") && why.contains("tile.example"), "{why}");
        assert!(!engine.factory_snapshot().factory_dir().join("pages").exists(), "nothing half stored");
    }

    #[tokio::test]
    async fn a_worker_publishes_only_from_a_run_and_rollback_needs_scope_reach() {
        let engine = engine();
        let (_, run) = running(&engine, "radar", "tok-radar").await;
        let first = send(&engine, Some("tok-radar"), publish(None, "conference-radar", "<p>one</p>", None)).await;
        let Response::Ok { data: Payload::PageVersion { version: first, .. } } = first else { panic!("{first:?}") };
        let second = send(&engine, Some("tok-radar"), publish(None, "conference-radar", "<p>two</p>", None)).await;
        let Response::Ok { data: Payload::PageVersion { version: second, .. } } = second else { panic!() };

        let rollback = || Request::PageRollback { scope: "radar".into(), name: "conference-radar".into(), version: first.sha256[..7].into() };
        let denied = send(&engine, Some("tok-radar"), rollback()).await;
        assert!(refused(&denied).contains("requires scope reach"), "a worker's run does not roll its page back");

        let rolled = send(&engine, None, rollback()).await;
        let Response::Ok { data: Payload::Page { page } } = rolled else { panic!("{rolled:?}") };
        assert_eq!(page.current.unwrap().sha256, first.sha256);
        assert_eq!(page.versions[0].sha256, second.sha256, "the newer version is still kept");

        // A rollback is a file under .factory/pages/: a fresh engine on the
        // same instance -- a restarted daemon -- still shows it.
        let restarted = Arc::new(Engine::new(
            engine.factory_snapshot(),
            Registry::with_builtins(),
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            Vec::new(),
        ));
        let content = send(
            &restarted,
            None,
            Request::PageContent { scope: "radar".into(), name: "conference-radar".into(), version: None, part: PagePart::Html },
        )
        .await;
        let Response::Ok { data: Payload::PageContent { content } } = content else { panic!("{content:?}") };
        assert_eq!(content.html.as_deref(), Some("<p>one</p>"));
        assert_eq!(content.version.sha256, first.sha256);
        let _ = run;
    }

    #[tokio::test]
    async fn the_dashboard_answers_which_scope_declares_each_page_tile() {
        let factory = engine().factory_snapshot();
        let mut config = factory.config.clone();
        config.scopes[0].dashboard = Some(serde_yaml_ng::from_str("tiles:\n  - { page: conference-radar, size: xl }\n  - { page: other/elsewhere, size: l }\n").unwrap());
        config.validate().unwrap();
        let resolved = Factory { root: factory.root.clone(), config: config.clone() };
        let (dashboard, source) = resolved.dashboard_for(Some("radar")).unwrap();
        let scopes = resolved.page_tile_scopes(dashboard.as_ref().unwrap(), source.as_deref());
        assert_eq!(scopes.get("conference-radar").map(String::as_str), Some("radar"));
        assert_eq!(scopes.get("other/elsewhere").map(String::as_str), Some("other"));

        config.scopes[0].dashboard = Some(serde_yaml_ng::from_str("tiles:\n  - { page: roadmapp, size: xl }\n").unwrap());
        let e = config.validate().unwrap_err().to_string();
        assert!(e.contains("scope \"radar\" dashboard.tiles[0].page \"roadmapp\""), "{e}");
        assert!(e.contains("it declares conference-radar, roadmap"), "{e}");
    }
}
