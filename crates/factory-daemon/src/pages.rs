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
