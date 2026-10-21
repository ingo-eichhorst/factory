//! The Codex backend's model list for the installed client (`#340`): what L1
//! keeps of it, and the pure pieces that get it there.
//!
//! The backend serves its model list per client version, so a model the
//! configuration names can be refused to the codex installed here -- with an
//! HTTP 400 on a run's first turn, after the TUI is up, where nothing reports
//! it. L1's observation job (the daemon's important-dates loop) fetches the
//! list in the background and records it in [`Cache`]; [`Provider`] answers
//! `CodexModelsFact` from that, the live agent declarations and a fresh read
//! of the host's codex `config.toml`, and never touches the network.
//!
//! The ChatGPT login is read in-process and only ever written to curl's
//! stdin as request headers: [`Login`] has no `Debug`, and nothing it holds
//! reaches a fact, an observation, a log line or a message.
use chrono::{DateTime, Utc};
use factory_kernel::{
    CodexAgentModel, CodexModelSource, CodexModelsFact, CodexServedModel, FactoryError, Provide,
    Result,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::io::AsyncReadExt;

/// The ChatGPT Codex backend's model list; `client_version=` is appended.
pub const DEFAULT_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/models";

/// The largest `config.toml`, `auth.json` or package manifest read.
const MAX_FILE: u64 = 1024 * 1024;

/// What the observation job found the last time it asked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observation {
    pub client_version: Option<String>,
    pub served: Vec<CodexServedModel>,
    pub observed_at: Option<DateTime<Utc>>,
    pub issue: Option<String>,
    /// Where the job looked for codex's own files (`$CODEX_HOME`, else
    /// `~/.codex`), so a read takes `config.toml` from the same place, fresh.
    pub codex_home: Option<PathBuf>,
}

/// L1's own memory of the last observation. Lost on restart: the job runs
/// again as the daemon comes up, and until it has, every verdict is unknown
/// and nothing is refused.
#[derive(Default)]
pub struct Cache(Mutex<Option<Observation>>);

impl Cache {
    pub fn get(&self) -> Option<Observation> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Keep `observation`, and return what is kept. One that failed keeps the
    /// list fetched earlier for the same client version beside its issue; a
    /// list for another version is never carried over -- it says nothing
    /// about the client installed now.
    pub fn record(&self, mut observation: Observation) -> Observation {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if observation.observed_at.is_none() {
            if let Some(previous) = slot.as_ref().filter(|previous| {
                previous.observed_at.is_some() && previous.client_version == observation.client_version
            }) {
                observation.served = previous.served.clone();
                observation.observed_at = previous.observed_at;
            }
        }
        *slot = Some(observation.clone());
        observation
    }
}

/// One declared codex agent, as plain configuration. No `Debug`: its
/// arguments are configuration a person wrote, and are never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct CodexAgent {
    pub scope: String,
    pub agent: String,
    pub args: Vec<String>,
}

/// `CodexModelsFact` over L1's cache, with the declarations captured fresh
/// for this read.
pub struct Provider<'a> {
    pub cache: &'a Cache,
    pub agents: Vec<CodexAgent>,
}

impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L1;
}

#[async_trait::async_trait]
impl Provide<CodexModelsFact> for Provider<'_> {
    type Query = ();
    type Value = CodexModelsFact;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<CodexModelsFact> {
        let observation = self.cache.get();
        let host = match observation.as_ref().and_then(|o| o.codex_home.as_deref()) {
            Some(home) => read_host_config(home).await,
            None => HostConfig::default(),
        };
        Ok(fact(observation.as_ref(), &host, &self.agents))
    }
}

/// The fact, from an observation (or none yet), the host's codex config and
/// the declared agents. A model is served when the backend lists it, hidden
/// or not; nothing is decided without a list.
pub fn fact(observation: Option<&Observation>, host: &HostConfig, agents: &[CodexAgent]) -> CodexModelsFact {
    let list = observation.filter(|o| o.observed_at.is_some()).map(|o| o.served.as_slice());
    let verdict = |model: Option<&str>| -> Option<bool> {
        let (list, model) = (list?, model?);
        Some(list.iter().any(|served| served.slug == model))
    };
    let host_default = resolve(&Choice::default(), host);
    let default_served = match host_default.note {
        Some(_) => None,
        None => verdict(host.model.as_deref()),
    };
    let agents = agents
        .iter()
        .map(|agent| {
            let resolved = resolve(&declared_choice(&agent.args), host);
            let served = match resolved.note {
                Some(_) => None,
                None => verdict(resolved.model.as_deref()),
            };
            CodexAgentModel {
                scope: agent.scope.clone(),
                agent: agent.agent.clone(),
                model: resolved.model,
                source: resolved.source,
                served,
                note: resolved.note,
            }
        })
        .collect();
    CodexModelsFact {
        client_version: observation.and_then(|o| o.client_version.clone()),
        served: observation.map(|o| o.served.clone()).unwrap_or_default(),
        observed_at: observation.and_then(|o| o.observed_at),
        issue: match observation {
            Some(o) => o.issue.clone(),
            None => Some("not observed yet since the daemon started".into()),
        },
        default_model: host.model.clone(),
        default_served,
        agents,
    }
}

// ------------------------------------------------------------ which model

/// What an agent's own arguments say about its model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choice {
    /// `-m`/`--model`: beats everything, a profile included.
    pub model: Option<String>,
    /// `-c model=…`: a config override, which a profile's `model` still beats.
    pub config_model: Option<String>,
    /// `-p`/`--profile`.
    pub profile: Option<String>,
    /// `-c profile=…`.
    pub config_profile: Option<String>,
    /// `-c model_provider=…`.
    pub model_provider: Option<String>,
    /// `--oss`: a local model provider, not the ChatGPT backend.
    pub oss: bool,
}

/// Read a codex command line the way codex's own parser would for the few
/// flags that choose a model. Everything after `--` is the prompt.
pub fn declared_choice(args: &[String]) -> Choice {
    let mut choice = Choice::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let arg = arg.as_str();
        if arg == "--" {
            break;
        }
        if arg == "--oss" {
            choice.oss = true;
            continue;
        }
        let mut value = |long: &str, short: &str| -> Option<String> {
            if arg == long || arg == short {
                return args.next().cloned();
            }
            if let Some(rest) = arg.strip_prefix(long).and_then(|rest| rest.strip_prefix('=')) {
                return Some(rest.to_string());
            }
            let rest = arg.strip_prefix(short).filter(|rest| !rest.is_empty() && !arg.starts_with("--"))?;
            Some(rest.strip_prefix('=').unwrap_or(rest).to_string())
        };
        if let Some(model) = value("--model", "-m") {
            choice.model = Some(model);
        } else if let Some(profile) = value("--profile", "-p") {
            choice.profile = Some(profile);
        } else if let Some(pair) = value("--config", "-c") {
            let Some((key, raw)) = pair.split_once('=') else {
                continue;
            };
            let value = toml_value(raw.trim()).unwrap_or_else(|| raw.trim().to_string());
            match key.trim() {
                "model" => choice.config_model = Some(value),
                "profile" => choice.config_profile = Some(value),
                "model_provider" => choice.model_provider = Some(value),
                _ => {}
            }
        }
    }
    choice
}

/// The handful of keys in codex's `config.toml` that choose a model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostConfig {
    pub model: Option<String>,
    pub profile: Option<String>,
    pub model_provider: Option<String>,
    pub profiles: BTreeMap<String, ProfileConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileConfig {
    pub model: Option<String>,
    pub model_provider: Option<String>,
}

/// Which model an agent will ask for, from where, and -- when the ChatGPT
/// list cannot answer for it -- why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub model: Option<String>,
    pub source: CodexModelSource,
    pub note: Option<String>,
}

/// codex's own precedence: `-m` beats a profile's `model`, which beats the
/// configuration's `model` (a `-c model=` override included), which beats
/// codex's built-in choice. `-p` beats `-c profile=`, which beats the
/// configuration's `profile`.
pub fn resolve(choice: &Choice, host: &HostConfig) -> Resolved {
    let profile = choice
        .profile
        .as_ref()
        .or(choice.config_profile.as_ref())
        .or(host.profile.as_ref())
        .and_then(|name| host.profiles.get(name));
    let (model, source) = if let Some(model) = &choice.model {
        (Some(model.clone()), CodexModelSource::Declared)
    } else if let Some(model) = profile.and_then(|p| p.model.clone()) {
        (Some(model), CodexModelSource::Profile)
    } else if let Some(model) = &choice.config_model {
        (Some(model.clone()), CodexModelSource::Declared)
    } else if let Some(model) = &host.model {
        (Some(model.clone()), CodexModelSource::ConfigDefault)
    } else {
        (None, CodexModelSource::CodexDefault)
    };
    let provider = if choice.oss {
        Some("oss".to_string())
    } else {
        choice
            .model_provider
            .clone()
            .or_else(|| profile.and_then(|p| p.model_provider.clone()))
            .or_else(|| host.model_provider.clone())
    };
    let note = match provider.as_deref() {
        Some(provider) if provider != "openai" => {
            Some(format!("model provider {provider}: the ChatGPT model list does not cover it"))
        }
        _ if model.is_none() => Some("no model is named, so codex picks its own".into()),
        _ => None,
    };
    Resolved { model, source, note }
}

/// The keys [`HostConfig`] holds, from `config.toml`'s text. Not a TOML
/// parser: it reads `key = "string"` lines in the root table and in
/// `[profiles.<name>]`, and steps over multi-line strings and arrays so a
/// line inside one is never mistaken for a key or a table.
pub fn parse_host_config(text: &str) -> HostConfig {
    enum Table {
        Root,
        Profile(String),
        Other,
    }
    let mut config = HostConfig::default();
    let mut table = Table::Root;
    let mut skip_until: Option<&str> = None;
    let mut depth: i32 = 0;
    for line in text.lines() {
        if let Some(delimiter) = skip_until {
            if line.contains(delimiter) {
                skip_until = None;
            }
            continue;
        }
        if depth > 0 {
            depth += bracket_depth(line);
            continue;
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("[[") {
            table = Table::Other;
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let header = header.split(']').next().unwrap_or_default().trim();
            table = match header.strip_prefix("profiles.") {
                Some(name) => match toml_value(name.trim()) {
                    Some(quoted) => Table::Profile(quoted),
                    None if !name.contains('.') => Table::Profile(name.trim().to_string()),
                    None => Table::Other,
                },
                None => Table::Other,
            };
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let raw = raw.trim();
        for delimiter in ["\"\"\"", "'''"] {
            if let Some(rest) = raw.strip_prefix(delimiter) {
                if !rest.contains(delimiter) {
                    skip_until = Some(delimiter);
                }
            }
        }
        if skip_until.is_some() {
            continue;
        }
        if raw.starts_with('[') {
            depth = bracket_depth(raw);
            continue;
        }
        let Some(value) = toml_value(raw) else {
            continue;
        };
        let key = toml_value(key.trim()).unwrap_or_else(|| key.trim().to_string());
        match (&table, key.as_str()) {
            (Table::Root, "model") => config.model = Some(value),
            (Table::Root, "profile") => config.profile = Some(value),
            (Table::Root, "model_provider") => config.model_provider = Some(value),
            (Table::Profile(name), "model") => config.profiles.entry(name.clone()).or_default().model = Some(value),
            (Table::Profile(name), "model_provider") => {
                config.profiles.entry(name.clone()).or_default().model_provider = Some(value)
            }
            _ => {}
        }
    }
    config
}

/// How many more `[` than `]` a line opens, outside strings.
fn bracket_depth(line: &str) -> i32 {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in line.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '[' => depth += 1,
                ']' => depth -= 1,
                '#' => break,
                _ => {}
            },
        }
    }
    depth
}

/// A one-line TOML string -- `"basic"` with its common escapes, or
/// `'literal'` -- with any trailing comment ignored. `None` for anything else.
fn toml_value(raw: &str) -> Option<String> {
    let mut chars = raw.chars();
    match chars.next()? {
        '\'' => {
            let rest = &raw[1..];
            let end = rest.find('\'')?;
            Some(rest[..end].to_string())
        }
        '"' => {
            let mut out = String::new();
            let mut escaped = false;
            for c in chars {
                if escaped {
                    out.push(match c {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    return Some(out);
                } else {
                    out.push(c);
                }
            }
            None
        }
        _ => None,
    }
}

/// `config.toml` under `home`, read now. Missing or unreadable is an empty
/// configuration, which leaves codex to its own default.
pub async fn read_host_config(home: &Path) -> HostConfig {
    match read_bounded(&home.join("config.toml")).await {
        Some(bytes) => parse_host_config(&String::from_utf8_lossy(&bytes)),
        None => HostConfig::default(),
    }
}

/// A file of at most [`MAX_FILE`] bytes, or `None`.
pub async fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).await.ok()?;
    (bytes.len() as u64 <= MAX_FILE).then_some(bytes)
}

// ---------------------------------------------------------- which client

/// The package manifests that name an installed codex's version without
/// running it, nearest first: a Homebrew cask's `codex-package.json` beside
/// `bin/`, then npm's `package.json` one level up from `bin/codex.js`.
pub fn manifests_for(binary: &Path) -> Vec<PathBuf> {
    let real = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    let Some(package) = real.parent().and_then(Path::parent) else {
        return Vec::new();
    };
    vec![package.join("codex-package.json"), package.join("package.json")]
}

/// The version a codex package manifest records, when it is codex's.
pub fn version_from_manifest(bytes: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct Manifest {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        variant: Option<String>,
        #[serde(default)]
        version: Option<String>,
    }
    let manifest: Manifest = serde_json::from_slice(bytes).ok()?;
    let codex = manifest.variant.as_deref() == Some("codex") || manifest.name.as_deref() == Some("@openai/codex");
    manifest.version.filter(|v| codex && valid_version(v))
}

/// The version `codex --version` printed: `codex-cli 0.157.0`.
pub fn version_from_output(text: &str) -> Option<String> {
    let version = text.lines().next()?.split_whitespace().last()?;
    valid_version(version).then(|| version.to_string())
}

/// A version is safe to put in a URL: digits first, then alphanumerics,
/// dots, dashes and pluses only.
pub fn valid_version(version: &str) -> bool {
    version.len() <= 64
        && version.starts_with(|c: char| c.is_ascii_digit())
        && version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

// ------------------------------------------------------------ the request

/// The ChatGPT login codex itself uses. Deliberately no `Debug`, `Clone` or
/// `Serialize`: the only way out of here is [`Login::headers`], which goes to
/// curl's stdin.
pub struct Login {
    access_token: String,
    account_id: Option<String>,
}

impl Login {
    /// The request headers, one per line, for `curl -H @-`.
    pub fn headers(&self) -> Vec<u8> {
        let mut headers = format!("Authorization: Bearer {}\n", self.access_token);
        if let Some(account) = &self.account_id {
            headers.push_str(&format!("ChatGPT-Account-Id: {account}\n"));
        }
        headers.push_str("Accept: application/json\n");
        headers.into_bytes()
    }
}

/// The login in codex's `auth.json`. Every error is a fixed sentence: none
/// says anything the file contains.
pub fn login_from_auth(bytes: &[u8]) -> std::result::Result<Login, &'static str> {
    let auth: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| "codex's auth.json could not be read")?;
    let mode = auth.get("auth_mode").and_then(|v| v.as_str());
    let api_key = auth.get("OPENAI_API_KEY").is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()));
    if mode.is_some_and(|mode| mode != "chatgpt") || (mode.is_none() && api_key) {
        return Err("codex signs in with an API key here, which the ChatGPT model list does not cover");
    }
    let tokens = auth.get("tokens");
    let field = |name: &str| tokens.and_then(|t| t.get(name)).and_then(|v| v.as_str()).map(str::to_string);
    let access_token = field("access_token")
        .filter(|token| !token.is_empty())
        .ok_or("codex has no ChatGPT login on this host (`codex login`)")?;
    let account_id = field("account_id").filter(|id| !id.is_empty());
    let clean = |value: &str| value.chars().all(|c| c.is_ascii_graphic());
    if !clean(&access_token) || account_id.as_deref().is_some_and(|id| !clean(id)) {
        return Err("codex's ChatGPT login in auth.json is malformed");
    }
    Ok(Login { access_token, account_id })
}

/// The list's URL for `client_version`, which must already be valid.
pub fn models_url(endpoint: &str, client_version: &str) -> String {
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    format!("{endpoint}{separator}client_version={client_version}")
}

/// The backend's answer, read: the models it lists, in its priority order,
/// or a fixed explanation. Never the body itself.
pub fn answer(status: u16, body: &[u8]) -> std::result::Result<Vec<CodexServedModel>, String> {
    match status {
        200 => parse_models(body).map_err(str::to_string),
        401 | 403 => Err(format!(
            "the backend refused the ChatGPT login (HTTP {status}); codex renews it on its next start, or run `codex login`"
        )),
        0 => Err("the models endpoint did not answer".into()),
        other => Err(format!("the models endpoint answered HTTP {other}")),
    }
}

/// `{"models": [{"slug": …, "visibility": …, "priority": …}, …]}`.
pub fn parse_models(body: &[u8]) -> std::result::Result<Vec<CodexServedModel>, &'static str> {
    #[derive(Deserialize)]
    struct Response {
        models: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        slug: String,
        #[serde(default)]
        visibility: Option<String>,
        #[serde(default)]
        priority: Option<i64>,
    }
    let response: Response = serde_json::from_slice(body).map_err(|_| "the models endpoint answered something other than a model list")?;
    let mut entries: Vec<Entry> = response
        .models
        .into_iter()
        .filter(|entry| !entry.slug.is_empty() && entry.slug.len() <= 128 && entry.slug.chars().all(|c| c.is_ascii_graphic()))
        .collect();
    if entries.is_empty() {
        // Nothing is ever refused on an empty list: that is an outage's
        // answer far more often than a backend that serves nothing.
        return Err("the models endpoint listed no models for this client");
    }
    entries.sort_by_key(|entry| entry.priority.unwrap_or(i64::MAX));
    Ok(entries
        .into_iter()
        .map(|entry| CodexServedModel {
            hidden: entry.visibility.as_deref().is_some_and(|v| v != "list"),
            slug: entry.slug,
        })
        .collect())
}

/// The listed models a person would pick from, for a message: hidden ones
/// are served too, but not offered.
pub fn shown(served: &[CodexServedModel]) -> Vec<&str> {
    served.iter().filter(|m| !m.hidden).map(|m| m.slug.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn served(slugs: &[&str]) -> Vec<CodexServedModel> {
        slugs.iter().map(|s| CodexServedModel { slug: s.to_string(), hidden: false }).collect()
    }

    #[test]
    fn every_spelling_of_the_model_flag_is_read_and_the_last_wins() {
        for (line, want) in [
            (vec!["--yolo", "--model", "gpt-6-sol"], "gpt-6-sol"),
            (vec!["-m", "gpt-6-sol"], "gpt-6-sol"),
            (vec!["--model=gpt-6-sol"], "gpt-6-sol"),
            (vec!["-mgpt-6-sol"], "gpt-6-sol"),
            (vec!["-m=gpt-6-sol"], "gpt-6-sol"),
            (vec!["-m", "gpt-5.6-luna", "--model", "gpt-6-sol"], "gpt-6-sol"),
        ] {
            assert_eq!(declared_choice(&args(&line)).model.as_deref(), Some(want), "{line:?}");
        }
        let choice = declared_choice(&args(&["-c", "model=\"gpt-6-luna\"", "--config", "profile=fast"]));
        assert_eq!(choice.config_model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(choice.config_profile.as_deref(), Some("fast"));
        assert_eq!(declared_choice(&args(&["-c", "model=gpt-6"])).config_model.as_deref(), Some("gpt-6"));
        // A prompt after `--` is the prompt, not a flag.
        assert_eq!(declared_choice(&args(&["--yolo", "--", "-m", "x"])).model, None);
        // `--search` and `--yolo` are not `-s…`/`-y…` values of anything.
        assert_eq!(declared_choice(&args(&["--yolo", "--search"])), Choice::default());
    }

    #[test]
    fn config_toml_keys_are_read_from_the_right_tables_only() {
        let text = r#"
check_for_update_on_startup = false  # pinned (#173)
model = "gpt-6.1-sol"
model_reasoning_effort = "high"
developer_instructions = """
model = "not-this-one"
[profiles.trap]
"""
status_line = [
  "model = 'nor-this'",
]
[projects."/Users/factory"]
trust_level = "trusted"
model = "a-project-is-not-the-root"

[profiles.fast]
model = 'gpt-6-luna'

[profiles."local.box"]
model_provider = "ollama"

[tui.model_availability_nux]
"gpt-6.1-sol" = 4
"#;
        let config = parse_host_config(text);
        assert_eq!(config.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(config.profiles["fast"].model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(config.profiles["local.box"].model_provider.as_deref(), Some("ollama"));
        assert!(!config.profiles.contains_key("trap"));
        assert_eq!(config.profile, None);
    }

    #[test]
    fn precedence_follows_codex_declared_beats_profile_beats_config() {
        let host = parse_host_config("model = \"gpt-6.1-sol\"\n[profiles.fast]\nmodel = \"gpt-6-luna\"\n");
        let r = |line: &[&str]| resolve(&declared_choice(&args(line)), &host);
        assert_eq!(r(&["--yolo"]).model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(r(&["--yolo"]).source, CodexModelSource::ConfigDefault);
        assert_eq!(r(&["-p", "fast"]).source, CodexModelSource::Profile);
        assert_eq!(r(&["-p", "fast", "-m", "gpt-6-sol"]).model.as_deref(), Some("gpt-6-sol"));
        // codex applies `-c` to the configuration, which a profile still overrides.
        assert_eq!(r(&["-p", "fast", "-c", "model=gpt-6-sol"]).model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(r(&["-c", "model=gpt-6-sol"]).source, CodexModelSource::Declared);
        let bare = resolve(&Choice::default(), &HostConfig::default());
        assert_eq!((bare.model, bare.source), (None, CodexModelSource::CodexDefault));
        assert!(bare.note.is_some(), "no model named is not a verdict");
        assert!(r(&["--oss"]).note.unwrap().contains("oss"));
        assert!(r(&["-c", "model_provider=ollama"]).note.is_some());
        assert!(r(&["-c", "model_provider=openai"]).note.is_none());
    }

    #[test]
    fn the_verdict_needs_a_list_and_a_model_and_a_hidden_model_is_still_served() {
        let host = parse_host_config("model = \"gpt-6.1-sol\"\n");
        let agents = vec![
            CodexAgent { scope: "factory".into(), agent: "codex".into(), args: args(&["--yolo"]) },
            CodexAgent { scope: "radar".into(), agent: "scout".into(), args: args(&["--model", "gpt-6-sol"]) },
            CodexAgent { scope: "x".into(), agent: "reserve".into(), args: args(&["-m", "gpt-reserve"]) },
        ];
        let mut list = served(&["gpt-6-astra", "gpt-6-sol"]);
        list.push(CodexServedModel { slug: "gpt-reserve".into(), hidden: true });
        let observation = Observation {
            client_version: Some("0.157.0".into()),
            served: list,
            observed_at: Some(Utc::now()),
            ..Default::default()
        };
        let fact = fact(Some(&observation), &host, &agents);
        assert_eq!(fact.default_served, Some(false));
        let verdicts: Vec<_> = fact.agents.iter().map(|a| (a.agent.as_str(), a.served)).collect();
        assert_eq!(verdicts, [("codex", Some(false)), ("scout", Some(true)), ("reserve", Some(true))]);
        assert_eq!(fact.agents[0].source, CodexModelSource::ConfigDefault);

        let unknown = super::fact(None, &host, &agents);
        assert!(unknown.agents.iter().all(|a| a.served.is_none()));
        assert!(unknown.issue.is_some());
    }

    #[test]
    fn a_failed_observation_keeps_the_same_versions_list_and_drops_another_versions() {
        let cache = Cache::default();
        let at = Utc::now();
        cache.record(Observation {
            client_version: Some("0.157.0".into()),
            served: served(&["gpt-6-sol"]),
            observed_at: Some(at),
            ..Default::default()
        });
        let kept = cache.record(Observation {
            client_version: Some("0.157.0".into()),
            issue: Some("the models endpoint did not answer".into()),
            ..Default::default()
        });
        assert_eq!((kept.served.len(), kept.observed_at, kept.issue.is_some()), (1, Some(at), true));
        let upgraded = cache.record(Observation {
            client_version: Some("0.163.0".into()),
            issue: Some("the models endpoint did not answer".into()),
            ..Default::default()
        });
        assert!(upgraded.served.is_empty() && upgraded.observed_at.is_none());
    }

    #[test]
    fn versions_come_from_the_manifest_or_the_version_line_and_must_be_url_safe() {
        let cask = br#"{"layoutVersion":1,"version":"0.157.0","variant":"codex","entrypoint":"bin/codex"}"#;
        assert_eq!(version_from_manifest(cask).as_deref(), Some("0.157.0"));
        let npm = br#"{"name":"@openai/codex","version":"0.163.0"}"#;
        assert_eq!(version_from_manifest(npm).as_deref(), Some("0.163.0"));
        assert_eq!(version_from_manifest(br#"{"name":"other","version":"1.0.0"}"#), None);
        assert_eq!(version_from_output("codex-cli 0.157.0\n").as_deref(), Some("0.157.0"));
        assert_eq!(version_from_output("codex-cli 0.157.0&x=1"), None);
        assert!(!valid_version("latest"));
        assert_eq!(models_url(DEFAULT_ENDPOINT, "0.157.0"), "https://chatgpt.com/backend-api/codex/models?client_version=0.157.0");
        assert_eq!(models_url("http://127.0.0.1:9/m?a=b", "1.0.0"), "http://127.0.0.1:9/m?a=b&client_version=1.0.0");
    }

    #[test]
    fn the_login_leaves_only_as_headers_and_no_error_repeats_the_file() {
        let secret = "eyJsecret.token.value";
        let auth = format!(r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"{secret}","account_id":"acct-1","refresh_token":"r"}}}}"#);
        let login = login_from_auth(auth.as_bytes()).unwrap_or_else(|e| panic!("{e}"));
        let headers = String::from_utf8(login.headers()).unwrap();
        assert!(headers.contains(&format!("Authorization: Bearer {secret}\n")));
        assert!(headers.contains("ChatGPT-Account-Id: acct-1\n"));
        for bad in [
            format!(r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{secret}"}}"#),
            format!(r#"{{"OPENAI_API_KEY":"{secret}"}}"#),
            r#"{"auth_mode":"chatgpt","tokens":{}}"#.to_string(),
            format!(r#"{{"tokens":{{"access_token":"{secret}\nX-Injected: 1"}}}}"#),
            format!("not json {secret}"),
        ] {
            let error = login_from_auth(bad.as_bytes()).err().expect("refused");
            assert!(!error.contains(secret) && !error.contains("Injected"), "{error}");
        }
    }

    #[test]
    fn the_answer_is_sorted_by_priority_and_never_echoes_the_body() {
        let body = br#"{"models":[
            {"slug":"gpt-6-sol","visibility":"list","priority":4},
            {"slug":"codex-auto-review","visibility":"hide","priority":43},
            {"slug":"gpt-6-astra","visibility":"list","priority":2}]}"#;
        let list = answer(200, body).unwrap();
        assert_eq!(list.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(), ["gpt-6-astra", "gpt-6-sol", "codex-auto-review"]);
        assert_eq!(shown(&list), ["gpt-6-astra", "gpt-6-sol"]);
        assert!(answer(200, br#"{"models":[]}"#).is_err(), "an empty list decides nothing");
        let refused = answer(401, br#"{"detail":"secret-ish body"}"#).unwrap_err();
        assert!(refused.contains("401") && !refused.contains("secret-ish"));
        assert!(answer(200, b"<html>").unwrap_err().contains("other than a model list"));
    }
}
