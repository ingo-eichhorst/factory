//! The Codex model check's daemon half (`#340`): L1's observation of the
//! backend's model list, run by the important-dates job, and what that
//! observation records -- L1's cache, and one important date per model a
//! codex agent asks for.
//!
//! The observation needs three things: the installed client's version (its
//! package manifest, else a bounded `codex --version`), the host's ChatGPT
//! login (codex's own `auth.json`, read in-process), and one `curl` to the
//! backend with the login as headers on **stdin** -- never in an argument,
//! where any process on the host could read it, and never in the
//! environment. curl's stderr is discarded, its output capped, and only the
//! listed slugs survive the answer. A step that cannot be taken leaves a fixed
//! sentence as the issue, and everything unknown, which refuses nothing.
use crate::renewals::probes::{self, Tools};
use chrono::{DateTime, Utc};
use factory_core::config::Factory;
use factory_core::renewals::{DateBasis, DateDependency, DateKind, DateSource, ExpiryObservation};
use factory_infrastructure::codex_models::{self as models, CodexAgent, Observation};
use factory_kernel::{CodexModelsFact, CodexServedModel};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// How long a fetched list is used before the backend is asked again. The
/// job runs every five minutes and the answer is about half a megabyte; a
/// list changes when a model is launched or retired, not by the minute.
const REFETCH_AFTER: chrono::Duration = chrono::Duration::hours(1);

/// The last list fetched, by endpoint and client version, kept by the job's
/// own tools across its runs -- never shared with a read.
#[derive(Clone, Default)]
pub(crate) struct Memo(Arc<Mutex<Option<Fetched>>>);

struct Fetched {
    endpoint: String,
    version: String,
    at: DateTime<Utc>,
    served: Vec<CodexServedModel>,
}

/// Every declared codex agent, as plain configuration, in configuration order.
pub(crate) fn codex_agents(factory: &Factory) -> Vec<CodexAgent> {
    let mut agents = Vec::new();
    for name in factory.scope_names() {
        let Ok(scope) = factory.scope(&name) else {
            continue;
        };
        for agent in scope.declared_agents() {
            if agent.harness == "codex" {
                agents.push(CodexAgent { scope: scope.name.clone(), agent: agent.name(), args: agent.args.clone() });
            }
        }
    }
    agents
}

/// One observation, with the agents it was made for.
pub(crate) struct Probe {
    pub observation: Observation,
    pub agents: Vec<CodexAgent>,
}

/// Ask the backend which models it serves to the installed codex. `None`
/// when there is nothing to ask about: the check is off, discovery is off, or
/// no scope declares a codex agent.
pub(crate) async fn probe(factory: &Factory, tools: &Tools, now: DateTime<Utc>) -> Option<Probe> {
    let config = &factory.config.infrastructure.codex_models;
    let agents = codex_agents(factory);
    if !tools.enabled || !config.enabled || agents.is_empty() {
        return None;
    }
    let observation = observe(&config.endpoint, tools, now).await;
    Some(Probe { observation, agents })
}

async fn observe(endpoint: &str, tools: &Tools, now: DateTime<Utc>) -> Observation {
    let mut observation = Observation { codex_home: Some(tools.codex_home.clone()), ..Default::default() };
    let version = match client_version(&tools.codex).await {
        Ok(version) => version,
        Err(issue) => {
            observation.issue = Some(issue.into());
            return observation;
        }
    };
    observation.client_version = Some(version.clone());
    if let Some(fetched) = tools.codex_memo.0.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if fetched.endpoint == endpoint && fetched.version == version && now - fetched.at < REFETCH_AFTER {
            observation.served = fetched.served.clone();
            observation.observed_at = Some(fetched.at);
            return observation;
        }
    }
    let login = match models::read_bounded(&tools.codex_home.join("auth.json")).await {
        Some(bytes) => models::login_from_auth(&bytes),
        None => Err("codex has no ChatGPT login on this host (`codex login`)"),
    };
    let login = match login {
        Ok(login) => login,
        Err(issue) => {
            observation.issue = Some(issue.into());
            return observation;
        }
    };
    let url = models::models_url(endpoint, &version);
    let headers = login.headers();
    drop(login);
    let answered = probes::read_only(
        &tools.curl,
        &["-sS", "--max-time", "15", "-H", "@-", "-w", "\n%{http_code}", "--url", &url],
        Some(&headers),
        20,
    )
    .await;
    drop(headers);
    let listed = match answered {
        Ok(bytes) => {
            let split = bytes.iter().rposition(|b| *b == b'\n').unwrap_or(0);
            let status = std::str::from_utf8(&bytes[split..]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            models::answer(status, &bytes[..split])
        }
        Err(_) => Err("the models endpoint did not answer".into()),
    };
    match listed {
        Ok(served) => {
            *tools.codex_memo.0.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Fetched { endpoint: endpoint.to_string(), version, at: now, served: served.clone() });
            observation.served = served;
            observation.observed_at = Some(now);
        }
        Err(issue) => observation.issue = Some(issue),
    }
    observation
}

/// The installed codex's version, without running it when its package says
/// so; otherwise a bounded `--version` (a binary stalled in `_dyld_start`
/// costs this job ten seconds, not a dispatch).
async fn client_version(program: &str) -> Result<String, &'static str> {
    let binary = resolve(program).ok_or("codex was not found on the daemon's PATH")?;
    for manifest in models::manifests_for(&binary) {
        if let Some(version) = models::read_bounded(&manifest).await.as_deref().and_then(models::version_from_manifest) {
            return Ok(version);
        }
    }
    let shown = binary.display().to_string();
    let output = probes::read_only(&shown, &["--version"], None, 10)
        .await
        .map_err(|_| "codex --version did not answer")?;
    models::version_from_output(&String::from_utf8_lossy(&output)).ok_or("codex --version printed no version")
}

/// Where `program` is on the PATH the probes run with.
fn resolve(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program)).filter(|path| path.is_file());
    }
    std::env::split_paths(&crate::provision::augmented_path())
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// One important date per model a codex agent asks for: no expiry while the
/// backend serves it, expired from the observation that it does not. An
/// observation that failed carries its issue and no date of its own, so the
/// store keeps the last one it had.
pub(crate) fn expiry_observations(fact: &CodexModelsFact, now: DateTime<Utc>) -> Vec<ExpiryObservation> {
    let mut by_model: BTreeMap<&str, Vec<(&str, &str, Option<bool>)>> = BTreeMap::new();
    for row in fact.agents.iter().filter(|row| row.note.is_none()) {
        if let Some(model) = &row.model {
            by_model.entry(model).or_default().push((&row.scope, &row.agent, row.served));
        }
    }
    let version = fact.client_version.as_deref().unwrap_or("the installed client");
    by_model
        .into_iter()
        .map(|(model, rows)| {
            let mut item = probes::observation(
                format!("codex-model:{model}"),
                format!("Codex model {model}"),
                DateKind::Other,
                DateSource::CodexModel,
                now,
            );
            item.affects = rows
                .iter()
                .map(|(scope, agent, _)| DateDependency {
                    scope: Some(scope.to_string()),
                    agent: Some(agent.to_string()),
                    environment: None,
                    provider: None,
                    label: format!("{scope}/{agent}"),
                })
                .collect();
            item.renew = "pin a model the backend serves -- the agent's `--model`, or `model` in the codex config.toml -- or upgrade codex".into();
            let served = rows.first().and_then(|(_, _, served)| *served);
            match (&fact.issue, served) {
                (Some(issue), _) => {
                    item.issue = Some(issue.clone());
                    item.detail = format!("whether the backend serves it to codex {version} is not current");
                }
                (None, Some(true)) => {
                    item.no_expiry = true;
                    item.basis = DateBasis::Observed;
                    item.observed_at = fact.observed_at;
                    item.detail = format!("served to codex {version} with this host's ChatGPT login");
                }
                (None, Some(false)) => {
                    item.expires_at = fact.observed_at;
                    item.basis = DateBasis::Observed;
                    item.observed_at = fact.observed_at;
                    item.detail = format!(
                        "not served to codex {version}; the backend lists {}",
                        models::shown(&fact.served).join(", ")
                    );
                }
                (None, None) => {
                    item.issue = Some("the model list is not known".into());
                }
            }
            item
        })
        .collect()
}

impl crate::state::L1State {
    /// What the observation job found, recorded: the Codex model list into
    /// L1's cache, and every infrastructure date -- a model's among them --
    /// into the expiry store. A model that stays unserved keeps the date it
    /// was first seen unserved on, so the Inbox item and the notify hook's
    /// receipt stay one item rather than a new one every five minutes.
    pub(crate) async fn record_observations(
        &self,
        mut infrastructure: Vec<ExpiryObservation>,
        complete: bool,
        codex: Option<Probe>,
    ) -> factory_core::error::Result<()> {
        if let Some(probe) = codex {
            let kept = self.codex_models.record(probe.observation);
            let host = match kept.codex_home.as_deref() {
                Some(home) => models::read_host_config(home).await,
                None => models::HostConfig::default(),
            };
            let fact = models::fact(Some(&kept), &host, &probe.agents);
            let previous = self.infrastructure_expiries.all().await.unwrap_or_default();
            let mut dated = expiry_observations(&fact, Utc::now());
            for item in &mut dated {
                let first = previous
                    .iter()
                    .find(|old| old.id == item.id && !old.no_expiry)
                    .and_then(|old| old.expires_at);
                if let (Some(first), Some(seen)) = (first, item.expires_at) {
                    item.expires_at = Some(seen.min(first));
                }
            }
            infrastructure.extend(dated);
        }
        self.infrastructure_expiries.replace(infrastructure, complete).await
    }
}

/// `$CODEX_HOME`, else `~/.codex` under the metadata home.
pub(crate) fn codex_home(metadata_home: &Path, isolated: bool) -> PathBuf {
    match std::env::var_os("CODEX_HOME").filter(|_| !isolated) {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => metadata_home.join(".codex"),
    }
}


#[cfg(test)]
mod tests;
