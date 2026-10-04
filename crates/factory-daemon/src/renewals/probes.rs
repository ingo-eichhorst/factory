//! Read-only, bounded metadata probes. Never `provider get`, a token source
//! command, a credential file read, or a private TLS key. Child stderr is
//! discarded; the cache receives only fixed explanations and expiry dates.
use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, Utc};
use factory_core::{
    config::Factory,
    openshell::{CredentialSource, ProviderDecl},
    renewals::*,
};
use futures_util::{stream, StreamExt};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

const MAX_OUTPUT: u64 = 2 * 1024 * 1024;
const MAX_ENDPOINTS: usize = 128;
const MAX_PAGES: usize = 20;

fn tailscale_program(path: &std::ffi::OsStr, bundled: &Path) -> String {
    if std::env::split_paths(path).any(|directory| directory.join("tailscale").is_file()) {
        "tailscale".into()
    } else if bundled.is_file() {
        bundled.to_string_lossy().into_owned()
    } else {
        "tailscale".into()
    }
}

#[derive(Clone)]
pub(crate) struct Tools {
    pub openssl: String,
    pub tailscale: String,
    pub github: String,
    pub openshell: Option<String>,
    pub metadata_home: PathBuf,
    pub config_home: PathBuf,
    pub enabled: bool,
}
impl Default for Tools {
    fn default() -> Self {
        let isolated = std::env::var_os("FACTORY_DATES_METADATA_HOME");
        let metadata_home = isolated
            .clone()
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let config_home = if isolated.is_some() {
            metadata_home.join(".config")
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| metadata_home.join(".config"))
        };
        Self {
            openssl: std::env::var("FACTORY_DATES_OPENSSL").unwrap_or_else(|_| "openssl".into()),
            tailscale: std::env::var("FACTORY_DATES_TAILSCALE").unwrap_or_else(|_| {
                tailscale_program(
                    &crate::provision::augmented_path(),
                    Path::new("/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
                )
            }),
            github: std::env::var("FACTORY_DATES_GITHUB").unwrap_or_else(|_| "gh".into()),
            openshell: std::env::var("FACTORY_DATES_OPENSHELL").ok(),
            metadata_home,
            config_home,
            enabled: std::env::var("FACTORY_RENEWALS_DISCOVERY").as_deref() != Ok("0"),
        }
    }
}

pub(crate) fn observation(
    id: String,
    name: String,
    kind: DateKind,
    source: DateSource,
    now: DateTime<Utc>,
) -> ExpiryObservation {
    ExpiryObservation {
        id,
        name,
        kind,
        source,
        scope: None,
        expires_at: None,
        no_expiry: false,
        basis: DateBasis::Unknown,
        detail: "expiry not yet observed".into(),
        observed_at: None,
        attempted_at: now,
        issue: None,
        affects: Vec::new(),
        lead_seconds: DEFAULT_LEAD_SECONDS,
        renew: "renew with the service owner".into(),
        owner: "owner".into(),
    }
}

/// Nothing a failed command printed is persisted. At most 2 MiB, and a
/// deadline with kill-on-drop, including a child that keeps stdout open.
async fn read_only(
    program: &str,
    args: &[&str],
    input: Option<&[u8]>,
    seconds: u64,
) -> Result<Vec<u8>, &'static str> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("PATH", crate::provision::augmented_path())
        .env("NO_COLOR", "1")
        .env_remove("FACTORY_TOKEN")
        .env_remove("FACTORY_TASK_TOKEN")
        .env_remove("FACTORY_RUN_TOKEN")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| "metadata tool unavailable")?;
    let result = async {
        if let Some(input) = input {
            let mut stdin = child.stdin.take().ok_or("metadata input unavailable")?;
            stdin
                .write_all(input)
                .await
                .map_err(|_| "metadata input failed")?;
            drop(stdin);
        }
        let stdout = child.stdout.take().ok_or("metadata output unavailable")?;
        let mut bytes = Vec::new();
        stdout
            .take(MAX_OUTPUT + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "metadata output failed")?;
        if bytes.len() as u64 > MAX_OUTPUT {
            let _ = child.kill().await;
            return Err("metadata output exceeds limit");
        }
        let status = child.wait().await.map_err(|_| "metadata command failed")?;
        if !status.success() {
            return Err("metadata command failed");
        }
        Ok(bytes)
    };
    tokio::time::timeout(Duration::from_secs(seconds), result)
        .await
        .map_err(|_| "metadata observation timed out")?
}

/// Read the first peer public certificate, then kill the inspection child.
/// Keep stdin open through the handshake: recent OpenSSL may exit on EOF
/// before it prints the peer chain. Do not wait for HTTP or mTLS success;
/// notAfter is expiry evidence, not a trust/health verdict. No TLS session
/// diagnostics leave this function, only the bounded public PEM block.
async fn peer_certificate(tools: &Tools, host: &str, port: u16) -> Result<Vec<u8>, &'static str> {
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut command = Command::new(&tools.openssl);
    command
        .args([
            "s_client",
            "-connect",
            &authority,
            "-servername",
            host,
            "-showcerts",
        ])
        .env("PATH", crate::provision::augmented_path())
        .env("NO_COLOR", "1")
        .env_remove("FACTORY_TOKEN")
        .env_remove("FACTORY_TASK_TOKEN")
        .env_remove("FACTORY_RUN_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| "TLS metadata tool unavailable")?;
    let read = async {
        let stdout = child
            .stdout
            .take()
            .ok_or("TLS metadata output unavailable")?;
        let mut reader = BufReader::new(stdout.take(MAX_OUTPUT + 1));
        let mut total = 0;
        let mut certificate = Vec::new();
        let mut started = false;
        loop {
            let mut line = Vec::new();
            let count = reader
                .read_until(b'\n', &mut line)
                .await
                .map_err(|_| "TLS metadata read failed")?;
            total += count;
            if total as u64 > MAX_OUTPUT {
                return Err("TLS metadata output exceeds limit");
            }
            if count == 0 {
                return Err("peer public certificate unavailable");
            }
            if line.starts_with(b"-----BEGIN CERTIFICATE-----") {
                started = true;
            }
            if started {
                certificate.extend(&line);
            }
            if started && line.starts_with(b"-----END CERTIFICATE-----") {
                return Ok(certificate);
            }
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(8), read)
        .await
        .unwrap_or(Err("TLS metadata observation timed out"));
    let _ = child.kill().await;
    result
}

#[derive(Clone)]
enum CertificateTarget {
    Endpoint { host: String, port: u16 },
    PublicFile(PathBuf),
}

fn endpoint(url: &str) -> Option<(String, u16)> {
    let uri: axum::http::Uri = url.parse().ok()?;
    if uri.scheme_str()? != "https" || uri.authority()?.as_str().contains('@') {
        return None;
    }
    let host = uri.host()?.trim_matches(['[', ']']);
    if host.is_empty()
        || host.starts_with('-')
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
    {
        return None;
    }
    Some((host.to_string(), uri.port_u16().unwrap_or(443)))
}
fn tls_item(host: String, port: u16, now: DateTime<Utc>) -> (ExpiryObservation, CertificateTarget) {
    let mut item = observation(
        format!("tls:{host}:{port}"),
        format!("TLS {host}:{port}"),
        DateKind::Certificate,
        DateSource::Tls,
        now,
    );
    item.renew =
        "renew the certificate with its issuer; Tailscale Serve normally renews automatically"
            .into();
    (item, CertificateTarget::Endpoint { host, port })
}
fn mark(
    item: &mut ExpiryObservation,
    expiry: Option<DateTime<Utc>>,
    basis: DateBasis,
    detail: &str,
    now: DateTime<Utc>,
) {
    item.expires_at = expiry;
    item.no_expiry = expiry.is_none();
    item.basis = basis;
    item.detail = detail.into();
    item.observed_at = Some(now);
    item.issue = None;
}
fn issue(item: &mut ExpiryObservation, message: &str) {
    item.issue = Some(message.into());
}

fn certificate_expiry(bytes: &[u8]) -> Option<DateTime<Utc>> {
    let text = std::str::from_utf8(bytes)
        .ok()?
        .trim()
        .strip_prefix("notAfter=")?;
    NaiveDateTime::parse_from_str(text, "%b %e %H:%M:%S %Y GMT")
        .ok()
        .map(|date| date.and_utc())
}
async fn certificate(
    tools: &Tools,
    mut item: ExpiryObservation,
    target: CertificateTarget,
    now: DateTime<Utc>,
) -> ExpiryObservation {
    let enddate = match target {
        CertificateTarget::Endpoint { host, port } => {
            // Expiry evidence only: not a trust, hostname or health verdict.
            match peer_certificate(tools, &host, port).await {
                Ok(pem) => {
                    read_only(
                        &tools.openssl,
                        &["x509", "-noout", "-enddate"],
                        Some(&pem),
                        3,
                    )
                    .await
                }
                Err(error) => Err(error),
            }
        }
        CertificateTarget::PublicFile(path) => {
            // Fixed public filenames from OpenShell, never tls.key. Refuse
            // symlinks too, so a public filename cannot redirect to a key.
            match tokio::fs::symlink_metadata(&path).await {
                Ok(meta)
                    if meta.is_file()
                        && meta.len() <= 256 * 1024
                        && !meta.file_type().is_symlink() =>
                {
                    // Only openssl reads the public PEM. The ledger receives
                    // the one-line end date, never the file's bytes.
                    match path.to_str() {
                        Some(path) => {
                            read_only(
                                &tools.openssl,
                                &["x509", "-in", path, "-noout", "-enddate"],
                                None,
                                3,
                            )
                            .await
                        }
                        None => Err("public certificate path unavailable"),
                    }
                }
                _ => Err("public certificate unavailable"),
            }
        }
    };
    let expiry = match enddate {
        Ok(date) => certificate_expiry(&date).ok_or("certificate expiry unavailable"),
        Err(error) => Err(error),
    };
    match expiry {
        Ok(date) => mark(
            &mut item,
            Some(date),
            DateBasis::Observed,
            "public certificate notAfter (not a trust or health check)",
            now,
        ),
        Err(error) => issue(&mut item, error),
    }
    item
}

#[derive(Clone, Deserialize)]
struct ProviderMetadata {
    name: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    credential_expires_at_ms: BTreeMap<String, i64>,
}
#[derive(Deserialize)]
struct ProviderPage {
    providers: Vec<ProviderMetadata>,
    #[serde(default)]
    next_page_token: String,
}
async fn providers(
    cli: &str,
    gateway: Option<&str>,
) -> Result<Vec<ProviderMetadata>, &'static str> {
    let mut items = Vec::new();
    let mut token = String::new();
    let mut seen = BTreeSet::new();
    for _ in 0..MAX_PAGES {
        let mut args = Vec::new();
        if let Some(gateway) = gateway {
            args.extend(["--gateway", gateway]);
        }
        args.extend(["provider", "list", "-o", "json", "--page-size", "100"]);
        if !token.is_empty() {
            args.extend(["--page-token", token.as_str()]);
        }
        let bytes = read_only(cli, &args, None, 10).await?;
        let page: ProviderPage =
            serde_json::from_slice(&bytes).map_err(|_| "provider expiry metadata unavailable")?;
        if page.providers.len() > 100 {
            return Err("provider page exceeds limit");
        }
        items.extend(page.providers);
        token = page.next_page_token;
        if token.is_empty() {
            return Ok(items);
        }
        if token.len() > 4096 || !seen.insert(token.clone()) {
            return Err("provider pagination invalid");
        }
    }
    Err("provider page limit reached")
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 253
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
fn metadata_expiry(provider: &ProviderMetadata) -> Option<DateTime<Utc>> {
    provider
        .credential_expires_at_ms
        .values()
        .filter(|millis| **millis > 0)
        .filter_map(|millis| DateTime::from_timestamp_millis(*millis))
        .min()
}
fn claude_file(tools: &Tools, path: &str) -> PathBuf {
    path.strip_prefix("~/")
        .map(|relative| tools.metadata_home.join(relative))
        .unwrap_or_else(|| PathBuf::from(path))
}
fn derive_claude(item: &mut ExpiryObservation, path: &Path, now: DateTime<Utc>) {
    // stat only. This remains safe when the file is unreadable, and never
    // evaluates a command/environment/Keychain credential source.
    match std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .and_then(|meta| meta.modified().ok())
    {
        Some(modified) => {
            let issued: DateTime<Utc> = modified.into();
            if let Some(expiry) = issued.checked_add_signed(ChronoDuration::days(365)) {
                mark(
                    item,
                    Some(expiry),
                    DateBasis::Derived,
                    "approximate: credential file mtime + 365 days; not a token claim",
                    now,
                );
            } else {
                issue(item, "credential file timestamp outside supported range");
            }
        }
        None => issue(
            item,
            "credential source modification time unavailable (contents not read)",
        ),
    }
}
fn agent_dependency(scope: &str, agent: &str, provider: &str) -> DateDependency {
    DateDependency {
        scope: Some(scope.into()),
        agent: Some(agent.into()),
        environment: None,
        provider: Some(provider.into()),
        label: format!("{scope}/{agent}"),
    }
}

fn github_expiry(bytes: &[u8]) -> Result<Option<DateTime<Utc>>, &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| "GitHub expiry metadata unavailable")?;
    let headers = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .map(|(headers, _)| headers)
        .unwrap_or(text);
    let first = headers.lines().next().unwrap_or_default();
    if !first
        .split_whitespace()
        .nth(1)
        .is_some_and(|code| code == "200")
    {
        return Err("GitHub authenticated metadata call did not succeed");
    }
    let value = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("github-authentication-token-expiration"))
        .map(|(_, value)| value.trim());
    match value {
        None => Ok(None),
        Some(value) => DateTime::parse_from_rfc3339(value)
            .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S %z"))
            .or_else(|_| {
                DateTime::parse_from_str(&value.replace(" UTC", " +0000"), "%Y-%m-%d %H:%M:%S %z")
            })
            .map(|date| Some(date.with_timezone(&Utc)))
            .map_err(|_| "GitHub expiry header not understood"),
    }
}

pub(crate) struct Observed {
    pub infrastructure: Vec<ExpiryObservation>,
    pub credentials: Vec<ExpiryObservation>,
    pub infrastructure_complete: bool,
    pub credentials_complete: bool,
}
impl Observed {
    pub(crate) fn unavailable(now: DateTime<Utc>) -> Self {
        let mut infrastructure = observation(
            "discovery:infrastructure".into(),
            "Infrastructure expiry discovery".into(),
            DateKind::Other,
            DateSource::Tls,
            now,
        );
        let mut credentials = observation(
            "discovery:providers".into(),
            "Provider expiry discovery".into(),
            DateKind::Other,
            DateSource::Openshell,
            now,
        );
        for item in [&mut infrastructure, &mut credentials] {
            issue(item, "metadata discovery did not finish within its deadline; last-known sources retained");
        }
        Self {
            infrastructure: vec![infrastructure],
            credentials: vec![credentials],
            infrastructure_complete: false,
            credentials_complete: false,
        }
    }
}
pub(crate) async fn observe(factory: &Factory, tools: &Tools, now: DateTime<Utc>) -> Observed {
    if !tools.enabled {
        return Observed {
            infrastructure: Vec::new(),
            credentials: Vec::new(),
            infrastructure_complete: true,
            credentials_complete: true,
        };
    }
    let mut infrastructure_complete = true;
    let mut credentials_complete = true;
    let mut certificates: BTreeMap<String, (ExpiryObservation, CertificateTarget)> =
        BTreeMap::new();
    for (scope, env) in factory.config.environments() {
        if let Some((host, port)) = env.url.as_deref().and_then(endpoint) {
            let (mut item, target) = tls_item(host, port, now);
            item.affects.push(DateDependency {
                scope: Some(scope),
                agent: None,
                environment: Some(env.name.clone()),
                provider: None,
                label: env.name,
            });
            certificates
                .entry(item.id.clone())
                .and_modify(|(old, _)| old.affects.extend(item.affects.clone()))
                .or_insert((item, target));
        }
    }
    let tailscale = read_only(&tools.tailscale, &["status", "--json"], None, 5).await;
    if tailscale.is_err() {
        infrastructure_complete = false;
    }
    if let Ok(bytes) = tailscale {
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(status) => {
                if let Some(host) = status
                    .pointer("/Self/DNSName")
                    .and_then(|value| value.as_str())
                    .map(|host| host.trim_end_matches('.'))
                    .filter(|host| safe_name(host))
                {
                    for port in [8790, 8791] {
                        let (item, target) = tls_item(host.into(), port, now);
                        certificates
                            .entry(item.id.clone())
                            .or_insert((item, target));
                    }
                    match read_only(&tools.tailscale, &["serve", "status", "--json"], None, 5).await
                    {
                        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                            Ok(serve) => {
                                for address in serve
                                    .get("Web")
                                    .and_then(|web| web.as_object())
                                    .into_iter()
                                    .flat_map(|web| web.keys())
                                {
                                    if let Some((host, port)) =
                                        endpoint(&format!("https://{address}"))
                                    {
                                        let (item, target) = tls_item(host, port, now);
                                        certificates
                                            .entry(item.id.clone())
                                            .or_insert((item, target));
                                    }
                                }
                            }
                            Err(_) => infrastructure_complete = false,
                        },
                        Err(_) => infrastructure_complete = false,
                    }
                } else {
                    infrastructure_complete = false;
                }
            }
            Err(_) => infrastructure_complete = false,
        }
    }
    let default_cli = tools
        .openshell
        .clone()
        .or_else(|| crate::openshell::resolve_cli(None).ok());
    if default_cli.is_none() {
        infrastructure_complete = false;
        credentials_complete = false;
    }
    let mut active_gateway = None;
    let mut gateway_certificates: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut gateways: BTreeMap<
        (String, Option<String>),
        Vec<(String, String, factory_core::openshell::OpenshellConfig)>,
    > = BTreeMap::new();
    if let Some(cli) = &default_cli {
        let discovery = read_only(cli, &["gateway", "list", "-o", "json"], None, 5).await;
        if discovery.is_err() {
            infrastructure_complete = false;
            credentials_complete = false;
        }
        if let Ok(bytes) = discovery {
            if let Ok(list) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                let list = list.as_array().or_else(|| {
                    list.get("gateways")
                        .and_then(|gateways| gateways.as_array())
                });
                if list.is_none() || list.is_some_and(|list| list.len() > 32) {
                    infrastructure_complete = false;
                    credentials_complete = false;
                }
                for gateway in list.into_iter().flatten().take(32) {
                    let Some(name) = gateway
                        .get("name")
                        .and_then(|name| name.as_str())
                        .filter(|name| safe_name(name))
                    else {
                        continue;
                    };
                    if gateway.get("active").and_then(|active| active.as_bool()) == Some(true) {
                        active_gateway = Some(name.to_string());
                    }
                    gateways
                        .entry((cli.clone(), Some(name.into())))
                        .or_default();
                    let certificate_ids = gateway_certificates
                        .entry((cli.clone(), name.into()))
                        .or_default();
                    if let Some((host, port)) = gateway
                        .get("endpoint")
                        .and_then(|url| url.as_str())
                        .and_then(endpoint)
                    {
                        let (item, target) = tls_item(host, port, now);
                        certificate_ids.push(item.id.clone());
                        certificates
                            .entry(item.id.clone())
                            .or_insert((item, target));
                    }
                    for public in ["ca.crt", "tls.crt"] {
                        let path = tools
                            .config_home
                            .join("openshell/gateways")
                            .join(name)
                            .join("mtls")
                            .join(public);
                        let mut item = observation(
                            format!("openshell-cert:{name}:{public}"),
                            format!("OpenShell {name} {public}"),
                            DateKind::Certificate,
                            DateSource::Tls,
                            now,
                        );
                        item.renew = "refresh the OpenShell gateway's public mTLS certificates with its owner".into();
                        certificate_ids.push(item.id.clone());
                        certificates
                            .insert(item.id.clone(), (item, CertificateTarget::PublicFile(path)));
                    }
                }
            } else {
                infrastructure_complete = false;
                credentials_complete = false;
            }
        }
    }
    for scope in &factory.config.scopes {
        for agent in scope.declared_agents() {
            let agent_name = agent.name();
            let Some(config) = agent.openshell else {
                continue;
            };
            if let Ok(cli) = crate::openshell::resolve_cli(config.cli.as_deref()) {
                gateways
                    .entry((
                        cli,
                        config.gateway.clone().or_else(|| active_gateway.clone()),
                    ))
                    .or_default()
                    .push((scope.name.clone(), agent_name, config));
            }
        }
    }
    let suffix = factory_core::openshell::instance_suffix(&factory.config.instance.id);
    let mut credentials = BTreeMap::new();
    if gateways.len() > 32 {
        credentials_complete = false;
    }
    for ((cli, gateway), agents) in gateways.into_iter().take(32) {
        if let Some(ids) = gateway
            .as_ref()
            .and_then(|gateway| gateway_certificates.get(&(cli.clone(), gateway.clone())))
        {
            for id in ids {
                if let Some((certificate, _)) = certificates.get_mut(id) {
                    certificate
                        .affects
                        .extend(agents.iter().map(|(scope, agent, _)| DateDependency {
                            scope: Some(scope.clone()),
                            agent: Some(agent.clone()),
                            environment: None,
                            provider: None,
                            label: format!("{scope}/{agent}"),
                        }));
                }
            }
        }
        let listed =
            tokio::time::timeout(Duration::from_secs(30), providers(&cli, gateway.as_deref()))
                .await
                .unwrap_or(Err("provider metadata observation timed out"));
        if listed.is_err() {
            credentials_complete = false;
        }
        if let Ok(listed) = &listed {
            for provider in listed.iter().filter(|provider| safe_name(&provider.name)) {
                let id = format!(
                    "openshell:{}:{}",
                    gateway.as_deref().unwrap_or("active"),
                    provider.name
                );
                let mut item = observation(
                    id,
                    format!("OpenShell {}", provider.name),
                    DateKind::Credential,
                    DateSource::Openshell,
                    now,
                );
                item.renew = "rotate the credential at its declared source; Factory provisions it, never mints it".into();
                if let Some(expiry) = metadata_expiry(provider) {
                    mark(
                        &mut item,
                        Some(expiry),
                        DateBasis::Observed,
                        "OpenShell credential expiry metadata",
                        now,
                    );
                } else {
                    item.detail =
                        "provider does not report an expiry; no credential value read".into();
                }
                if provider.kind.contains("claude") && item.expires_at.is_none() {
                    let default = tools
                        .metadata_home
                        .join(".config/factory/secrets/claude-oauth-token");
                    if default.is_file() {
                        derive_claude(&mut item, &default, now);
                    }
                }
                credentials.insert(item.id.clone(), item);
            }
        }
        for (scope, agent, config) in agents {
            for decl in &config.providers {
                let name = decl.gateway_name(&suffix);
                let id = format!(
                    "openshell:{}:{name}",
                    gateway.as_deref().unwrap_or("active")
                );
                let item = credentials.entry(id.clone()).or_insert_with(|| {
                    observation(
                        id,
                        format!("OpenShell {name}"),
                        DateKind::Credential,
                        DateSource::Openshell,
                        now,
                    )
                });
                item.affects.push(agent_dependency(&scope, &agent, &name));
                if let Err(error) = &listed {
                    issue(item, error);
                    continue;
                }
                if let ProviderDecl::Managed(managed) = decl {
                    if item.basis != DateBasis::Observed {
                        if let Some(expires) = managed
                            .expires
                            .and_then(|date| date.and_hms_opt(0, 0, 0))
                            .map(|date| date.and_utc())
                        {
                            mark(
                                item,
                                Some(expires),
                                DateBasis::Declared,
                                "provider's declared expires date",
                                now,
                            );
                        } else if managed.kind.contains("claude") {
                            if let CredentialSource::File { path } = &managed.credential {
                                derive_claude(item, &claude_file(tools, path), now);
                            }
                        }
                    }
                }
            }
        }
    }
    let mut github = observation(
        "github-token".into(),
        "GitHub authenticated token".into(),
        DateKind::Credential,
        DateSource::GithubAuth,
        now,
    );
    github.renew =
        "renew the GitHub token and update gh authentication or its declared source".into();
    match read_only(
        &tools.github,
        &["api", "--hostname", "github.com", "--include", "user"],
        None,
        10,
    )
    .await
    .and_then(|bytes| github_expiry(&bytes))
    {
        Ok(expiry) => mark(
            &mut github,
            expiry,
            DateBasis::Observed,
            "GitHub authenticated response expiry header; absent header means no reported expiry",
            now,
        ),
        Err(error) => issue(&mut github, error),
    }
    for scope in &factory.config.scopes {
        for agent in scope.declared_agents() {
            let agent_name = agent.name();
            if agent
                .provider
                .as_deref()
                .is_some_and(|provider| provider.contains("github"))
            {
                github
                    .affects
                    .push(agent_dependency(&scope.name, &agent.name(), "github"));
            }
            if let Some(config) = agent.openshell {
                for decl in config.providers {
                    if let ProviderDecl::Managed(managed) = decl {
                        if managed.kind.contains("github")
                            && matches!(managed.credential, CredentialSource::Command { ref run } if run.trim() == "gh auth token")
                        {
                            github.affects.push(agent_dependency(
                                &scope.name,
                                &agent_name,
                                &managed.name,
                            ));
                        }
                    }
                }
            }
        }
    }
    credentials.insert(github.id.clone(), github);
    if certificates.len() > MAX_ENDPOINTS {
        infrastructure_complete = false;
    }
    let mut infrastructure: Vec<_> = stream::iter(certificates.into_values().take(MAX_ENDPOINTS))
        .map(|(item, target)| certificate(tools, item, target, now))
        .buffer_unordered(8)
        .collect()
        .await;
    if !infrastructure_complete {
        let mut discovery = observation(
            "discovery:infrastructure".into(),
            "Infrastructure expiry discovery".into(),
            DateKind::Other,
            DateSource::Tls,
            now,
        );
        issue(
            &mut discovery,
            "some endpoint metadata could not be discovered; last-known sources retained",
        );
        infrastructure.push(discovery);
    }
    if !credentials_complete {
        let mut discovery = observation(
            "discovery:providers".into(),
            "Provider expiry discovery".into(),
            DateKind::Other,
            DateSource::Openshell,
            now,
        );
        issue(
            &mut discovery,
            "some provider metadata could not be discovered; last-known sources retained",
        );
        credentials.insert(discovery.id.clone(), discovery);
    }
    Observed {
        infrastructure,
        credentials: credentials.into_values().collect(),
        infrastructure_complete,
        credentials_complete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tls_and_github_dates_are_metadata_only_and_strict() {
        assert_eq!(
            endpoint("https://example.com:8790/path"),
            Some(("example.com".into(), 8790))
        );
        assert!(endpoint("https://user:secret@example.com").is_none());
        assert!(endpoint("http://example.com").is_none());
        assert_eq!(
            certificate_expiry(b"notAfter=Oct  4 00:00:00 2027 GMT\n"),
            Some("2027-10-04T00:00:00Z".parse().unwrap())
        );
        assert_eq!(github_expiry(b"HTTP/2.0 200 OK\nGitHub-Authentication-Token-Expiration: 2027-10-04 01:00:00 +0100\n\n{\"secret\":\"never-kept\"}").unwrap(), Some("2027-10-04T00:00:00Z".parse().unwrap()));
        assert_eq!(github_expiry(b"HTTP/2.0 200 OK\n\n{}").unwrap(), None);
        assert!(github_expiry(b"HTTP/2.0 401 Unauthorized\n\nsecret").is_err());
    }

    #[tokio::test]
    async fn metadata_failures_are_opaque_pagination_is_bounded_and_children_time_out() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("factory-dates-bounds-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _fixture = Fixture(root.clone());
        let cli = root.join("fixture");
        let bundled = root.join("Tailscale");
        std::fs::write(&bundled, "fixture").unwrap();
        assert_eq!(
            tailscale_program(std::ffi::OsStr::new("/no-tools"), &bundled),
            bundled.to_string_lossy()
        );
        assert_eq!(
            tailscale_program(std::ffi::OsStr::new("/no-tools"), &root.join("missing")),
            "tailscale"
        );
        std::fs::write(root.join("tailscale"), "fixture").unwrap();
        assert_eq!(tailscale_program(root.as_os_str(), &bundled), "tailscale");
        let write = |text: &str| {
            std::fs::write(&cli, text).unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        write("#!/bin/sh\nprintf 'secret-from-stderr' >&2; printf 'secret-from-stdout'; exit 7\n");
        assert_eq!(
            read_only(cli.to_str().unwrap(), &[], None, 10)
                .await
                .unwrap_err(),
            "metadata command failed"
        );
        write("#!/bin/sh\necho '{\"providers\":[],\"next_page_token\":\"same\"}'\n");
        assert!(matches!(
            providers(cli.to_str().unwrap(), None).await,
            Err("provider pagination invalid")
        ));
        write("#!/bin/sh\nexec sleep 10\n");
        let before = std::time::Instant::now();
        assert_eq!(
            read_only(cli.to_str().unwrap(), &[], None, 1)
                .await
                .unwrap_err(),
            "metadata observation timed out"
        );
        assert!(before.elapsed() < Duration::from_secs(8));
        write("#!/bin/sh\nexec head -c 2097153 /dev/zero\n");
        assert_eq!(
            read_only(cli.to_str().unwrap(), &[], None, 10)
                .await
                .unwrap_err(),
            "metadata output exceeds limit"
        );
        assert!(!safe_name("..") && !safe_name("../tls.key") && !safe_name("/etc/passwd"));
    }
}
