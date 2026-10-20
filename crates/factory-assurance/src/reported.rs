//! `reported.<source>.<metric>` (`#278`): a scope's own domain data, read on
//! request the same way every other L5 metric is -- never stored, never
//! watched, never pushed. A scope declares one *source* (a JSON file its own
//! tooling writes, or -- `#333` -- one of its own declared network dependency
//! services, read over loopback) and the metrics it reports through it, in its own
//! `.factory/config.yaml` (`scope.metrics`); the composition layer parses
//! that block as plain strings and hands it here, fresh, on every read --
//! the same "plain input supplied fresh per read" shape `quality_inputs`
//! already uses for a scope's declared quality layers. This module owns:
//!
//! - [`validate`]: turning the raw declarations into a [`Catalogue`] of
//!   [`ValidSource`]s plus [`Finding`]s for anything an author got wrong
//!   (a bad slug, a path that escapes the instance, a symlink, a duplicate
//!   source id, an unknown unit) -- never a hard error that would refuse the
//!   whole config load;
//! - [`read_source`]: opening one source's file, bounded and `O_NOFOLLOW`,
//!   and parsing it into a [`Document`]; [`read_service`] is its `#333`
//!   twin, one bounded HTTP GET to a loopback endpoint with no redirects;
//! - [`value_for`]: turning one asked `(source, metric)` pair, the
//!   [`Catalogue`], already-read [`Document`]s and a scope-coverage flag
//!   into the [`MetricValue`] `metrics_service::compute_one` hands back --
//!   the one place every `None` reason this family can give is decided.
//!
//! Nothing here ever reads a file itself unless `metrics_service` asks for
//! that exact source's `Document` -- there is no cache, no table and no
//! background read. See `metrics_service.rs`'s own doc comments for how the
//! three pieces are actually wired into one `metrics_for` call.
use crate::metrics::{Better, Unit};
use chrono::{DateTime, Utc};
use factory_kernel::{is_metric_segment, is_slug, MetricId, MetricValue, ScopeNode};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path, PathBuf},
};

// ============================================================ declaration

/// One scope's raw `scope.metrics` block, exactly as the composition layer
/// parsed it -- plain strings, no validation. The outer `Option` (on
/// `ScopeDeclaration::source`) means the scope wrote no `metrics:` block
/// at all; each field *inside* a present block is itself optional too --
/// a missing `id`/`file` is an authoring mistake [`validate`] turns into
/// a finding, the same as an invalid one, never a reason the scope's
/// whole config file fails to parse (`#278`).
#[derive(Debug, Clone, Default)]
pub struct RawSource {
    pub id: Option<String>,
    pub file: Option<String>,
    /// `#333`: the name of one of this scope's own declared dependency
    /// services to read the document from instead of a file -- exactly one
    /// of `file` and `service` is given.
    pub service: Option<String>,
    /// `#333`: the request path `service` answers the document at.
    pub path: Option<String>,
    pub declare: Vec<RawDeclared>,
    /// `#333`: the declaring scope's own `dependencies.services`, as plain
    /// data, so a `service` can be resolved here without L5 naming L2's own
    /// declaration types. Empty when the scope declares none.
    pub services: Vec<RawService>,
}

/// One declared dependency service, reduced to what a `service:` source
/// needs: its name, whether its transport is `network`, and its endpoints
/// exactly as written (`127.0.0.1:8793`, `https://host.ts.net:8793/x`, ...).
#[derive(Debug, Clone, Default)]
pub struct RawService {
    pub name: String,
    pub network: bool,
    pub endpoints: Vec<String>,
}

/// See [`RawSource`]'s doc comment: every field is optional for the same
/// reason -- a missing one is [`validate`]'s job to turn into a finding,
/// not serde's job to refuse to parse.
#[derive(Debug, Clone, Default)]
pub struct RawDeclared {
    pub id: Option<String>,
    pub title: Option<String>,
    pub unit: Option<String>,
    /// `"higher"` or `"lower"` -- see `metrics::Better`. Missing or an
    /// unknown spelling are both [`FindingKind::UnknownBetter`]: `resolve`'s
    /// own placeholder cannot know this direction, and a wrong default
    /// would risk exactly the false wrong-direction (or false not-wrong)
    /// Goals finding this field exists to prevent.
    pub better: Option<String>,
}

/// One scope's identity plus its raw declaration, handed in fresh by the
/// daemon off the live config snapshot -- the `quality_inputs::
/// ScopeConfiguration` shape for this family.
#[derive(Debug, Clone)]
pub struct ScopeDeclaration {
    pub scope: ScopeNode,
    pub source: Option<RawSource>,
}

/// Every scope's declaration, instance-wide -- [`validate`]'s only input
/// besides the instance root, and the thing a duplicate source id is
/// detected across.
#[derive(Debug, Clone, Default)]
pub struct Configuration {
    pub scopes: Vec<ScopeDeclaration>,
}

// ================================================================ findings

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// A source or metric id is not the slug/segment shape it must be, or
    /// is missing.
    BadSlug,
    /// Two scopes declared the same source id.
    DuplicateSourceId,
    /// Two `declare` entries in one source repeated an id; the first wins.
    DuplicateMetricId,
    /// `file` is absolute, or normalizes to somewhere outside the instance
    /// root.
    PathEscapesInstance,
    /// `file` resolves through a path component named `secrets`.
    PathUnderSecrets,
    /// The resolved path is, or passes through, a symlink.
    PathIsSymlink,
    /// `declare[].unit` is not one of `metrics::Unit`'s spellings, or is
    /// missing.
    UnknownUnit,
    /// `declare[].better` is not `higher` or `lower`, or is missing.
    UnknownBetter,
    /// A field with no shape or spelling of its own to be "wrong" --
    /// `source.file`/`source.service`, a service source's `path`, or
    /// `declare[].title` -- was not given at all.
    MissingField,
    /// `#333`: `source` gave both `file` and `service`.
    FileAndService,
    /// `#333`: `source.service` names no dependency service this scope
    /// declares.
    UnknownService,
    /// `#333`: `source.service` names a `socket` or `file` service.
    NotANetworkService,
    /// `#333`: the service lists no loopback endpoint (`127.0.0.1`,
    /// `localhost` or `[::1]` with a port) -- a tailnet or any other
    /// endpoint is never used, wherever it is listed.
    NoLoopbackEndpoint,
    /// `#333`: a service source's `path` is not an absolute request path.
    BadServicePath,
}

/// One authoring mistake in a `scope.metrics` declaration -- never a reason
/// to fail the whole config load. `subject` is `<scope>/<source-id>` or
/// `<scope>/<source-id>/<metric-id>`, so a reader knows where to go fix it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    pub subject: String,
    pub detail: String,
}

fn finding(kind: FindingKind, subject: impl Into<String>, detail: impl Into<String>) -> Finding {
    Finding {
        kind,
        subject: subject.into(),
        detail: detail.into(),
    }
}

fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        a.subject
            .cmp(&b.subject)
            .then(a.kind.cmp(&b.kind))
            .then(a.detail.cmp(&b.detail))
    });
}

// ================================================================= valid

/// One metric a source declares, with its unit and direction already a
/// real [`Unit`]/[`Better`] -- the only place the declared `title`/`unit`/
/// `better` enter L5's registry listing (`metrics_service::finish`
/// overlays them onto the generic `resolve`d definition) and L6 Goals'
/// own wrong-direction check (`directions`, below).
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredMetric {
    pub title: String,
    pub unit: Unit,
    pub better: Better,
}

/// One source that passed [`validate`]: a slug id, the scope that declared
/// it, where its document is read from, and its declared metrics keyed by id.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidSource {
    pub id: String,
    pub scope: ScopeNode,
    pub location: Location,
    pub declared: BTreeMap<String, DeclaredMetric>,
}

/// Where a [`ValidSource`]'s document comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Location {
    /// The file's resolved absolute path, already proven to stay inside the
    /// instance root, outside any `secrets` component, and -- at validation
    /// time -- not a symlink.
    File(PathBuf),
    /// `#333`: a declared network service's loopback endpoint.
    Service(ServiceTarget),
    /// `#333`: a `service` source [`validate`] refused -- an unknown or
    /// non-network service, no loopback endpoint, a bad or missing `path`,
    /// or `file` given as well. Kept, unlike a refused file path, so its
    /// declared metrics still list and each reads `None` with `reason`
    /// rather than as an undeclared source; the finding says the same.
    Refused {
        service: Option<String>,
        reason: String,
    },
}

/// The one loopback endpoint a `service` source is read from, chosen once by
/// [`validate`] -- pure string work, nothing is resolved or connected there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceTarget {
    pub service: String,
    /// `host:port` exactly as the chosen endpoint wrote it, scheme and any
    /// path of its own left off -- also the request's `Host` header.
    pub endpoint: String,
    pub host: LoopbackHost,
    pub port: u16,
    pub path: String,
}

/// The three loopback spellings an endpoint may use. Each is connected to as
/// a literal address -- `localhost` included, so no resolver or hosts file
/// can point a read anywhere but this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopbackHost {
    V4,
    Localhost,
    V6,
}

impl ServiceTarget {
    pub fn url(&self) -> String {
        format!("http://{}{}", self.endpoint, self.path)
    }

    fn addresses(&self) -> Vec<std::net::SocketAddr> {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
        let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port);
        let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), self.port);
        match self.host {
            LoopbackHost::V4 => vec![v4],
            LoopbackHost::V6 => vec![v6],
            LoopbackHost::Localhost => vec![v4, v6],
        }
    }
}

/// What [`validate`] found: every source that passed, keyed by id, plus
/// every finding -- a rejected source or metric simply does not appear in
/// `sources`, so nothing downstream has to re-check it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalogue {
    pub sources: BTreeMap<String, ValidSource>,
    pub findings: Vec<Finding>,
}

/// Turn every scope's raw declaration into a [`Catalogue`], computed fresh
/// from `configuration` every time -- no status table, no cache. `root` is
/// the instance root; each source's `file` is resolved against its own
/// declaring scope's directory (`root.join(scope.path)`), lexically --
/// `.`/`..` are resolved by comparing path components, never by asking the
/// filesystem (which would also follow a symlink) -- and refused if it
/// normalizes to outside `root` or through a component named `secrets`,
/// case-insensitively, anywhere in the resolved relative path. A source
/// whose own id is invalid, or whose path is refused, contributes no
/// `ValidSource` at all; a source that is otherwise valid keeps every
/// `declare` entry that is itself valid, dropping only the bad ones.
pub fn validate(root: &Path, configuration: &Configuration) -> Catalogue {
    let mut findings = Vec::new();
    let mut sources: BTreeMap<String, ValidSource> = BTreeMap::new();
    let root_norm = normalize_lexical(root);

    for decl in &configuration.scopes {
        let Some(raw) = &decl.source else { continue };
        let subject = format!(
            "{}/{}",
            decl.scope.name,
            raw.id.as_deref().unwrap_or("<missing source id>")
        );

        // A missing id is [`FindingKind::BadSlug`], exactly like an
        // invalid one -- `#278`'s fix: neither ever fails this scope's
        // config file to parse, only drops this one source.
        let Some(source_id) = raw.id.as_deref().filter(|id| is_slug(id)) else {
            findings.push(finding(
                FindingKind::BadSlug,
                &subject,
                match &raw.id {
                    None => "no source id was given".to_string(),
                    Some(id) => format!("{id:?} is not a valid metrics source id (a slug)"),
                },
            ));
            continue;
        };
        if sources.contains_key(source_id) {
            findings.push(finding(
                FindingKind::DuplicateSourceId,
                &subject,
                format!(
                    "source id {source_id:?} is already declared by scope {:?}",
                    sources[source_id].scope.name
                ),
            ));
            continue;
        }

        let location = match (raw.file.as_deref(), raw.service.as_deref()) {
            (None, None) => {
                findings.push(finding(
                    FindingKind::MissingField,
                    &subject,
                    "no file or service was given for this source".to_string(),
                ));
                continue;
            }
            // `#333`: both is refused rather than one picked -- either
            // guess could silently read the wrong document.
            (Some(_), Some(service)) => {
                let reason =
                    "both file and service are given; a source reads exactly one".to_string();
                findings.push(finding(FindingKind::FileAndService, &subject, &reason));
                Location::Refused {
                    service: Some(service.to_string()),
                    reason,
                }
            }
            (None, Some(service)) => match resolve_service(service, raw) {
                Ok(target) => Location::Service(target),
                Err((kind, reason)) => {
                    findings.push(finding(kind, &subject, &reason));
                    Location::Refused {
                        service: Some(service.to_string()),
                        reason,
                    }
                }
            },
            (Some(file), None) => {
                let path = match resolve_contained(&root_norm, &decl.scope.path, file) {
                    Ok(path) => path,
                    Err((kind, detail)) => {
                        findings.push(finding(kind, &subject, detail));
                        continue;
                    }
                };
                if path_has_symlink(&root_norm, &path) {
                    findings.push(finding(
                        FindingKind::PathIsSymlink,
                        &subject,
                        "the declared file path is, or passes through, a symlink".to_string(),
                    ));
                    continue;
                }
                Location::File(path)
            }
        };

        let mut declared = BTreeMap::new();
        for d in &raw.declare {
            let metric_subject = format!(
                "{subject}/{}",
                d.id.as_deref().unwrap_or("<missing metric id>")
            );
            let Some(metric_id) = d.id.as_deref().filter(|id| is_metric_segment(id)) else {
                findings.push(finding(
                    FindingKind::BadSlug,
                    &metric_subject,
                    match &d.id {
                        None => "no metric id was given".to_string(),
                        Some(id) => format!("{id:?} is not a valid metric id"),
                    },
                ));
                continue;
            };
            if declared.contains_key(metric_id) {
                findings.push(finding(
                    FindingKind::DuplicateMetricId,
                    &metric_subject,
                    format!("metric id {metric_id:?} is declared twice; the first wins"),
                ));
                continue;
            }
            let Some(title) = d.title.as_deref() else {
                findings.push(finding(
                    FindingKind::MissingField,
                    &metric_subject,
                    "no title was given for this metric".to_string(),
                ));
                continue;
            };
            let Some(unit) = d.unit.as_deref().and_then(parse_unit) else {
                findings.push(finding(
                    FindingKind::UnknownUnit,
                    &metric_subject,
                    match &d.unit {
                        None => "no unit was given for this metric".to_string(),
                        Some(u) => format!("{u:?} is not a known metric unit"),
                    },
                ));
                continue;
            };
            let Some(better) = d.better.as_deref().and_then(parse_better) else {
                findings.push(finding(
                    FindingKind::UnknownBetter,
                    &metric_subject,
                    match &d.better {
                        None => "no direction (better) was given for this metric".to_string(),
                        Some(b) => format!("{b:?} is not `higher` or `lower`"),
                    },
                ));
                continue;
            };
            declared.insert(
                metric_id.to_string(),
                DeclaredMetric {
                    title: title.to_string(),
                    unit,
                    better,
                },
            );
        }

        sources.insert(
            source_id.to_string(),
            ValidSource {
                id: source_id.to_string(),
                scope: decl.scope.clone(),
                location,
                declared,
            },
        );
    }

    sort_findings(&mut findings);
    Catalogue { sources, findings }
}

fn parse_unit(raw: &str) -> Option<Unit> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

fn parse_better(raw: &str) -> Option<Better> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

/// Every valid source's declared direction, keyed by its full
/// `reported.<source>.<metric>` id -- the plain input a caller like L6
/// Goals threads into its own wrong-direction check, since `metrics::
/// resolve` is pure and has no access to a live declaration. An id this
/// map has nothing for (an unknown source, an unknown metric, or simply a
/// caller that never built one) is never judged wrong either way by
/// whoever reads it -- see `goals::load_with_reported_directions`.
pub fn directions(catalogue: &Catalogue) -> BTreeMap<String, Better> {
    let mut out = BTreeMap::new();
    for source in catalogue.sources.values() {
        for (metric_id, declared) in &source.declared {
            out.insert(format!("reported.{}.{metric_id}", source.id), declared.better);
        }
    }
    out
}

/// `#333`: `service` resolved against the declaring scope's own declared
/// dependency services -- it must exist, be a `network` service and list a
/// loopback endpoint -- plus its `path`. The first loopback endpoint in list
/// order is chosen, however many non-loopback ones come before it; a tailnet
/// or any other endpoint is never a candidate at all.
fn resolve_service(
    service: &str,
    raw: &RawSource,
) -> std::result::Result<ServiceTarget, (FindingKind, String)> {
    let Some(declared) = raw.services.iter().find(|s| s.name == service) else {
        return Err((
            FindingKind::UnknownService,
            format!("this scope declares no dependency service {service:?}"),
        ));
    };
    if !declared.network {
        return Err((
            FindingKind::NotANetworkService,
            format!(
                "dependency service {service:?} is not a network service; only a network \
                 service's loopback endpoint is read"
            ),
        ));
    }
    let Some((endpoint, host, port)) = declared.endpoints.iter().find_map(|e| loopback(e)) else {
        return Err((
            FindingKind::NoLoopbackEndpoint,
            format!(
                "dependency service {service:?} lists no loopback endpoint (127.0.0.1, \
                 localhost or [::1] with a port); no other endpoint is ever used"
            ),
        ));
    };
    let path = match raw.path.as_deref() {
        None => {
            return Err((
                FindingKind::MissingField,
                "no path was given for this service source".to_string(),
            ))
        }
        Some(path) if !is_request_path(path) => {
            return Err((
                FindingKind::BadServicePath,
                format!(
                    "{path:?} is not a request path: it must start with `/` and hold no \
                     spaces, control characters or `#`"
                ),
            ))
        }
        Some(path) => path.to_string(),
    };
    Ok(ServiceTarget {
        service: service.to_string(),
        endpoint,
        host,
        port,
        path,
    })
}

/// `endpoint` as a loopback `host:port`, or `None`. A bare `host:port` or an
/// `http://` URL is accepted -- any other scheme is not, since the read is
/// plain HTTP -- and whatever path the endpoint itself carries is left off:
/// the source's own `path` names the document.
fn loopback(endpoint: &str) -> Option<(String, LoopbackHost, u16)> {
    let trimmed = endpoint.trim();
    let rest = match trimmed.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("http://") => &trimmed[7..],
        _ => trimmed,
    };
    if rest.contains("://") {
        return None;
    }
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let (host, port) = if let Some(port) = hostport.strip_prefix("[::1]:") {
        (LoopbackHost::V6, port)
    } else {
        let (host, port) = hostport.rsplit_once(':')?;
        if host == "127.0.0.1" {
            (LoopbackHost::V4, port)
        } else if host.eq_ignore_ascii_case("localhost") {
            (LoopbackHost::Localhost, port)
        } else {
            return None;
        }
    };
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    Some((hostport.to_string(), host, port))
}

fn is_request_path(path: &str) -> bool {
    path.starts_with('/') && path.bytes().all(|b| b.is_ascii_graphic() && b != b'#')
}

/// `file` resolved against `root_norm.join(scope_path)`, lexically
/// normalized, and proven to stay inside `root_norm` with no `secrets`
/// path component -- see [`validate`]'s doc comment. Returns the resolved
/// absolute path, or the finding to raise instead.
fn resolve_contained(
    root_norm: &Path,
    scope_path: &Path,
    file: &str,
) -> std::result::Result<PathBuf, (FindingKind, String)> {
    let candidate = Path::new(file);
    if candidate.is_absolute() {
        return Err((
            FindingKind::PathEscapesInstance,
            "the declared file path must be relative to the scope directory".to_string(),
        ));
    }
    let scope_dir = normalize_lexical(&root_norm.join(scope_path));
    let joined = normalize_lexical(&scope_dir.join(candidate));
    let rel = lexical_relative(root_norm, &joined).ok_or_else(|| {
        (
            FindingKind::PathEscapesInstance,
            "the declared file path normalizes to somewhere outside the instance root"
                .to_string(),
        )
    })?;
    if rel
        .split('/')
        .any(|component| component.eq_ignore_ascii_case("secrets"))
    {
        return Err((
            FindingKind::PathUnderSecrets,
            "the declared file path passes through a `secrets` component".to_string(),
        ));
    }
    Ok(root_norm.join(rel))
}

/// `candidate`'s path relative to `root`, computed purely by comparing path
/// components -- the same technique `knowledge.rs`'s `lexical_relative`
/// uses for a vault import source, reused here for a scope's declared
/// metrics file. `None` when `candidate` does not lie under `root` at all.
fn lexical_relative(root: &Path, candidate: &Path) -> Option<String> {
    let mut r = root.components();
    let mut c = candidate.components();
    for rc in r.by_ref() {
        match c.next() {
            Some(cc) if cc == rc => continue,
            _ => return None,
        }
    }
    let rest: Vec<String> = c
        .map(|comp| comp.as_os_str().to_string_lossy().into_owned())
        .collect();
    if rest.is_empty() {
        None
    } else {
        Some(rest.join("/"))
    }
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether `full_path` is, or is reached through, a symlink anywhere
/// between `root` and itself -- a filesystem fact, checked once at
/// [`validate`] time in addition to [`read_source`]'s own `O_NOFOLLOW`
/// open, since a file can be swapped for a symlink after validation. A
/// component that does not exist yet (the common case: a scope's own
/// tooling has not written the file yet) is not a symlink, so an
/// as-yet-missing source is never itself a validation finding.
fn path_has_symlink(root: &Path, full_path: &Path) -> bool {
    let Some(rel) = full_path.strip_prefix(root).ok() else {
        return false;
    };
    let mut cur = root.to_path_buf();
    for comp in rel.components() {
        cur.push(comp.as_os_str());
        if std::fs::symlink_metadata(&cur)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

// =================================================================== read

/// A file this large is refused outright rather than read -- generous for a
/// scope's own small progress report, nowhere near what a real dataset or
/// ledger export would be.
pub const MAX_SOURCE_BYTES: u64 = 256 * 1024;

/// A `reason` string longer than this is truncated -- the one piece of free
/// text this family carries, from a scope's own tooling, shown verbatim (but
/// escaped by the renderer) in the CLI and UI.
pub const MAX_REASON_CHARS: usize = 500;

#[derive(Debug, Clone, Deserialize)]
struct DocMetric {
    id: String,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    as_of: Option<DateTime<Utc>>,
}

/// One source's parsed file. `as_of` is required: a document with no
/// timestamp at all is treated the same as one that fails to parse, since
/// there is no instant to answer "as of when" for any metric it reports.
#[derive(Debug, Clone, Deserialize)]
pub struct Document {
    as_of: DateTime<Utc>,
    #[serde(default)]
    metrics: Vec<DocMetric>,
}

/// Open `path` read-only, `O_NOFOLLOW` (refusing a symlink at the moment of
/// the read itself, independent of [`validate`]'s own one-time check),
/// bounded to [`MAX_SOURCE_BYTES`], and parse it as the source's JSON
/// document. Every failure comes back as a plain reason string --
/// [`value_for`] is the only place that string becomes a `MetricValue`'s
/// `reason`, verbatim, since it already describes the source itself, not
/// one metric in it.
pub fn read_source(path: &Path) -> std::result::Result<Document, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "the declared metrics file does not exist yet".to_string()
            } else if error.raw_os_error() == Some(libc::ELOOP) {
                "the declared metrics file is a symlink, which is refused".to_string()
            } else {
                format!("the declared metrics file could not be opened: {error}")
            }
        })?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("the declared metrics file could not be read: {error}"))?;
    if !metadata.is_file() {
        return Err("the declared metrics file is not a regular file".to_string());
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "the declared metrics file is larger than {MAX_SOURCE_BYTES} bytes"
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("the declared metrics file could not be read: {error}"))?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(format!(
            "the declared metrics file is larger than {MAX_SOURCE_BYTES} bytes"
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("the declared metrics file does not parse: {error}"))
}

/// `#333`: how long one service read may take, connect to last byte. Short
/// on purpose: the service is on this machine, and a read that blocks waits
/// in front of whichever `metrics`/`quality`/`goals`/policy read asked.
pub const SERVICE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// A response head larger than this is refused before any body is read.
const MAX_HEAD_BYTES: usize = 16 * 1024;

/// `#333`: one GET of `target`'s document over loopback, plain HTTP/1.1 with
/// `Connection: close` -- one attempt, no retry, no redirect followed (a
/// `3xx` is a non-`2xx` like any other), the body bounded by
/// [`MAX_SOURCE_BYTES`] exactly like a file, and the whole exchange bounded
/// by `timeout` ([`SERVICE_TIMEOUT`] outside tests). Every failure comes back
/// as a plain reason string, the same contract as [`read_source`].
pub async fn read_service(
    target: &ServiceTarget,
    timeout: std::time::Duration,
) -> std::result::Result<Document, String> {
    let body = match tokio::time::timeout(timeout, fetch(target)).await {
        Ok(body) => body?,
        Err(_) => {
            return Err(format!(
                "the metrics service {:?} did not answer within {timeout:?} ({})",
                target.service,
                target.url()
            ))
        }
    };
    serde_json::from_slice(&body).map_err(|error| {
        format!(
            "the metrics service {:?} answered a body that does not parse: {error} ({})",
            target.service,
            target.url()
        )
    })
}

/// Every `(source id, target)` read concurrently, each on its own
/// [`read_service`] -- one request per source, however many of its ids the
/// caller asked for.
pub async fn read_services(
    targets: Vec<(String, ServiceTarget)>,
    timeout: std::time::Duration,
) -> BTreeMap<String, std::result::Result<Document, String>> {
    let mut reads = tokio::task::JoinSet::new();
    for (source_id, target) in targets {
        reads.spawn(async move {
            let read = read_service(&target, timeout).await;
            (source_id, read)
        });
    }
    let mut out = BTreeMap::new();
    while let Some(joined) = reads.join_next().await {
        if let Ok((source_id, read)) = joined {
            out.insert(source_id, read);
        }
    }
    out
}

async fn fetch(target: &ServiceTarget) -> std::result::Result<Vec<u8>, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let url = target.url();
    let service = &target.service;
    let failed = |what: &str, error: std::io::Error| {
        format!("the metrics service {service:?} {what} ({url}): {error}")
    };
    let mut stream = None;
    let mut last = None;
    for address in target.addresses() {
        match tokio::net::TcpStream::connect(address).await {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(error) => last = Some(error),
        }
    }
    let Some(mut stream) = stream else {
        return Err(failed(
            "could not be reached",
            last.unwrap_or_else(|| std::io::Error::other("no address")),
        ));
    };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\nUser-Agent: factory\r\n\r\n",
        target.path, target.endpoint
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| failed("could not be sent the request", e))?;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if raw.len() > MAX_HEAD_BYTES {
            return Err(format!(
                "the metrics service {service:?} answered a response head larger than \
                 {MAX_HEAD_BYTES} bytes ({url})"
            ));
        }
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|e| failed("could not be read from", e))?;
        if read == 0 {
            return Err(format!(
                "the metrics service {service:?} closed the connection before a complete \
                 HTTP response ({url})"
            ));
        }
        raw.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut body = raw[head_end + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let code: u16 = status_line
        .strip_prefix("HTTP/")
        .and_then(|rest| rest.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| {
            format!(
                "the metrics service {service:?} did not answer HTTP: {:?} ({url})",
                sanitize_reason(&status_line.chars().take(80).collect::<String>())
            )
        })?;
    if !(200..300).contains(&code) {
        let redirect = if (300..400).contains(&code) {
            "; redirects are not followed"
        } else {
            ""
        };
        return Err(format!(
            "the metrics service {service:?} answered HTTP {code} ({url}){redirect}"
        ));
    }
    let mut chunked = false;
    let mut length: Option<u64> = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if name.trim().eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        } else if name.trim().eq_ignore_ascii_case("content-length") {
            length = value.parse().ok();
        }
    }
    let too_large = || {
        format!(
            "the metrics service {service:?} answered a body larger than {MAX_SOURCE_BYTES} \
             bytes ({url})"
        )
    };
    if !chunked && length.is_some_and(|n| n > MAX_SOURCE_BYTES) {
        return Err(too_large());
    }
    // Raw bytes kept before giving up: the body itself, plus generous room
    // for chunk framing when there is any.
    let bound = match (chunked, length) {
        (true, _) => 2 * MAX_SOURCE_BYTES + 1,
        (false, Some(n)) => n,
        (false, None) => MAX_SOURCE_BYTES + 1,
    };
    while (body.len() as u64) < bound {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|e| failed("could not be read from", e))?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    if chunked {
        if body.len() as u64 >= bound {
            return Err(too_large());
        }
        body = dechunk(&body).ok_or_else(|| {
            format!("the metrics service {service:?} answered a malformed chunked body ({url})")
        })?;
    } else if let Some(n) = length {
        if (body.len() as u64) < n {
            return Err(format!(
                "the metrics service {service:?} closed the connection before the whole \
                 body ({url})"
            ));
        }
        body.truncate(n as usize);
    }
    if body.len() as u64 > MAX_SOURCE_BYTES {
        return Err(too_large());
    }
    Ok(body)
}

/// A `Transfer-Encoding: chunked` body, decoded -- the same framing
/// `factory-cli`'s own client decodes; `None` for a body that ends mid-chunk
/// or carries a bad size line.
fn dechunk(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = body.windows(2).position(|w| w == b"\r\n")?;
        let size_text = String::from_utf8_lossy(&body[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or_default().trim().to_string();
        let size = usize::from_str_radix(&size_text, 16).ok()?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        if body.len() < size {
            return None;
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size..];
        body = body.strip_prefix(b"\r\n").unwrap_or(body);
    }
}

fn sanitize_reason(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .take(MAX_REASON_CHARS)
        .collect()
}

// ================================================================= value

/// One asked `reported.<source>.<metric>` id turned into the `MetricValue`
/// `metrics_service::compute_one` hands back -- the one place every `None`
/// reason this family can give is decided, in this order: an unknown source,
/// a source declared outside the selected subtree, an id the source never
/// declared, the source's own read failure (missing/unparsable/refused/too
/// large), an id the read document does not report, a `null` value (the
/// document's own `reason`, sanitized, or a generic one), and a non-finite
/// value. Anything else is `Some(value)`, `as_of` the metric's own
/// timestamp if it gave one, the document's otherwise -- never `now`.
pub fn value_for(
    full_id: &MetricId,
    source_id: &str,
    metric_id: &str,
    catalogue: &Catalogue,
    docs: &BTreeMap<String, std::result::Result<Document, String>>,
    in_subtree: bool,
    now: DateTime<Utc>,
) -> MetricValue {
    let none = |reason: String| MetricValue {
        id: full_id.clone(),
        value: None,
        as_of: now,
        reason: Some(reason),
    };

    let Some(source) = catalogue.sources.get(source_id) else {
        return none(format!("no scope declares a metrics source {source_id:?}"));
    };
    if !in_subtree {
        return outside_subtree(full_id, source_id, &source.scope.name, now);
    }
    if !source.declared.contains_key(metric_id) {
        return none(format!(
            "source {source_id:?} does not declare a metric {metric_id:?}"
        ));
    }
    let doc = match docs.get(source_id) {
        Some(Ok(doc)) => doc,
        Some(Err(reason)) => return none(reason.clone()),
        None => return none(format!("source {source_id:?} was not read for this request")),
    };
    let Some(entry) = doc.metrics.iter().find(|m| m.id == metric_id) else {
        return none(format!(
            "{source_id:?}'s metrics document does not report {metric_id:?}"
        ));
    };
    let as_of = entry.as_of.unwrap_or(doc.as_of);
    match entry.value {
        None => {
            let reason = entry
                .reason
                .as_deref()
                .map(sanitize_reason)
                .unwrap_or_else(|| "no value was reported, and no reason was given".to_string());
            MetricValue {
                id: full_id.clone(),
                value: None,
                as_of,
                reason: Some(reason),
            }
        }
        Some(v) if !v.is_finite() => {
            none(format!("{metric_id:?} reported a non-finite value"))
        }
        Some(v) => MetricValue {
            id: full_id.clone(),
            value: Some(v),
            as_of,
            reason: None,
        },
    }
}

/// The one `None` this family gives for a source whose declaring scope is
/// not in the selection -- factored out of [`value_for`] so a `policy`
/// `check: metric` (`#278` phase 2) can give exactly this same reason when
/// it applies phase 1's own coverage rule per *evaluated* scope, instead of
/// per request: `metrics_service`'s own per-scope post-filter calls this
/// too, rather than inventing a second wording for the same gap.
pub fn outside_subtree(
    full_id: &MetricId,
    source_id: &str,
    declaring_scope: &str,
    now: DateTime<Utc>,
) -> MetricValue {
    MetricValue {
        id: full_id.clone(),
        value: None,
        as_of: now,
        reason: Some(format!(
            "{source_id:?} is declared by scope {declaring_scope:?}, outside the selected subtree"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn scope(name: &str, path: &str) -> ScopeNode {
        ScopeNode {
            name: name.to_string(),
            path: PathBuf::from(path),
        }
    }

    fn source(id: &str, file: &str, declare: &[(&str, &str, &str, &str)]) -> RawSource {
        RawSource {
            id: Some(id.to_string()),
            file: Some(file.to_string()),
            declare: declare
                .iter()
                .map(|(id, title, unit, better)| RawDeclared {
                    id: Some(id.to_string()),
                    title: Some(title.to_string()),
                    unit: Some(unit.to_string()),
                    better: Some(better.to_string()),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn tmp_root() -> PathBuf {
        std::env::temp_dir().join(format!("factory-reported-test-{}", uuid::Uuid::new_v4()))
    }

    // -------------------------------------------------------------- validate

    #[test]
    fn a_good_declaration_resolves_inside_the_instance_root() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("projects/finance")).unwrap();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(source(
                    "finance",
                    "../../data/finance/metrics.json",
                    &[("beleg_coverage", "Beleg coverage", "ratio", "higher")],
                )),
            }],
        };
        let catalogue = validate(&root, &config);
        assert!(catalogue.findings.is_empty(), "{:?}", catalogue.findings);
        let source = catalogue.sources.get("finance").unwrap();
        assert_eq!(source.location, Location::File(root.join("data/finance/metrics.json")));
        assert_eq!(source.declared["beleg_coverage"].unit, Unit::Ratio);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_source_slug_is_a_finding_not_a_crash() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(source("Finance_1", "metrics.json", &[])),
            }],
        };
        let catalogue = validate(&root, &config);
        assert!(catalogue.sources.is_empty());
        assert_eq!(catalogue.findings.len(), 1);
        assert_eq!(catalogue.findings[0].kind, FindingKind::BadSlug);
    }

    #[test]
    fn a_duplicate_source_id_across_scopes_is_a_finding_and_only_the_first_is_kept() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![
                ScopeDeclaration {
                    scope: scope("finance", "projects/finance"),
                    source: Some(source("shared", "a.json", &[])),
                },
                ScopeDeclaration {
                    scope: scope("other", "projects/other"),
                    source: Some(source("shared", "b.json", &[])),
                },
            ],
        };
        let catalogue = validate(&root, &config);
        assert_eq!(catalogue.sources.len(), 1);
        assert_eq!(catalogue.sources["shared"].scope.name, "finance");
        assert_eq!(catalogue.findings.len(), 1);
        assert_eq!(catalogue.findings[0].kind, FindingKind::DuplicateSourceId);
    }

    #[test]
    fn a_path_climbing_above_the_instance_root_is_a_finding() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(source("finance", "../../../../etc/passwd", &[])),
            }],
        };
        let catalogue = validate(&root, &config);
        assert!(catalogue.sources.is_empty());
        assert_eq!(catalogue.findings[0].kind, FindingKind::PathEscapesInstance);
    }

    #[test]
    fn an_absolute_path_is_a_finding() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(source("finance", "/etc/passwd", &[])),
            }],
        };
        let catalogue = validate(&root, &config);
        assert!(catalogue.sources.is_empty());
        assert_eq!(catalogue.findings[0].kind, FindingKind::PathEscapesInstance);
    }

    #[test]
    fn a_path_through_a_secrets_component_is_a_finding() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(source("finance", "../../secrets/metrics.json", &[])),
            }],
        };
        let catalogue = validate(&root, &config);
        assert!(catalogue.sources.is_empty());
        assert_eq!(catalogue.findings[0].kind, FindingKind::PathUnderSecrets);
    }

    #[test]
    fn a_symlinked_file_is_a_finding() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("data/finance")).unwrap();
        std::fs::create_dir_all(root.join("projects/finance")).unwrap();
        let real = root.join("data/finance/real.json");
        write(&real, "{}");
        let link = root.join("data/finance/metrics.json");
        symlink(&real, &link).unwrap();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(source("finance", "../../data/finance/metrics.json", &[])),
            }],
        };
        let catalogue = validate(&root, &config);
        assert!(catalogue.sources.is_empty());
        assert_eq!(catalogue.findings[0].kind, FindingKind::PathIsSymlink);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `ScopeNode.path` is documented as relative-to-root (and that is what
    /// `discovery::apply` always produces), but `factory_kernel::scope`'s
    /// own tests allow an absolute one too. `Path::join` replaces, rather
    /// than appends, when the right side is absolute, so an absolute scope
    /// path makes `resolve_contained` ignore `root` entirely for the scope
    /// directory -- this must fail safe (a finding), never silently resolve
    /// to a path outside `root` and read it anyway.
    #[test]
    fn an_absolute_scope_path_fails_safe_as_a_finding_rather_than_escaping_the_check() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "/elsewhere/finance"),
                source: Some(source("finance", "metrics.json", &[])),
            }],
        };
        let catalogue = validate(&root, &config);
        assert!(catalogue.sources.is_empty());
        assert_eq!(catalogue.findings[0].kind, FindingKind::PathEscapesInstance);
    }

    #[test]
    fn a_missing_file_is_not_itself_a_validation_finding() {
        // Computed on read: a scope that declared a source before its own
        // tooling ever wrote the file must load cleanly. The missing file
        // becomes a `None` reason only when a value is actually asked for.
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(source(
                    "finance",
                    "../../data/finance/metrics.json",
                    &[("x", "X", "count", "higher")],
                )),
            }],
        };
        let catalogue = validate(&root, &config);
        assert!(catalogue.findings.is_empty());
        assert!(catalogue.sources.contains_key("finance"));
    }

    #[test]
    fn a_bad_metric_slug_an_unknown_unit_and_an_unknown_better_are_findings_but_keep_the_source() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(source(
                    "finance",
                    "metrics.json",
                    &[
                        ("Bad Id", "Bad", "ratio", "higher"),
                        ("good_one", "Good", "ratio", "higher"),
                        ("other", "Other", "furlongs", "higher"),
                        ("sideways", "Sideways", "ratio", "sideways"),
                    ],
                )),
            }],
        };
        let catalogue = validate(&root, &config);
        let source = catalogue.sources.get("finance").unwrap();
        assert_eq!(source.declared.len(), 1);
        assert!(source.declared.contains_key("good_one"));
        assert_eq!(catalogue.findings.len(), 3);
        assert!(catalogue
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::BadSlug));
        assert!(catalogue
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::UnknownUnit));
        assert!(catalogue
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::UnknownBetter));
    }

    // -- missing (not just invalid) fields never fail config load (`#278`) --

    /// A `declare[]` entry missing `better` entirely gets the same
    /// treatment as an unknown spelling of it: `UnknownBetter`, the
    /// metric dropped, the source otherwise kept. Never a parse error --
    /// this is `validate`'s job, never `discovery::read_scope`'s.
    #[test]
    fn a_declared_metric_missing_better_is_an_unknown_better_finding_and_is_dropped() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(RawSource {
                    id: Some("finance".to_string()),
                    file: Some("metrics.json".to_string()),
                    declare: vec![RawDeclared {
                        id: Some("x".to_string()),
                        title: Some("X".to_string()),
                        unit: Some("count".to_string()),
                        better: None,
                    }],
                    ..Default::default()
                }),
            }],
        };
        let catalogue = validate(&root, &config);
        let source = catalogue.sources.get("finance").unwrap();
        assert!(!source.declared.contains_key("x"));
        assert_eq!(catalogue.findings.len(), 1);
        assert_eq!(catalogue.findings[0].kind, FindingKind::UnknownBetter);
    }

    /// Same for a missing `title` -- there is no "invalid title" shape to
    /// reuse a finding kind from, so this is `MissingField`.
    #[test]
    fn a_declared_metric_missing_title_is_a_missing_field_finding_and_is_dropped() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(RawSource {
                    id: Some("finance".to_string()),
                    file: Some("metrics.json".to_string()),
                    declare: vec![RawDeclared {
                        id: Some("x".to_string()),
                        title: None,
                        unit: Some("count".to_string()),
                        better: Some("higher".to_string()),
                    }],
                    ..Default::default()
                }),
            }],
        };
        let catalogue = validate(&root, &config);
        let source = catalogue.sources.get("finance").unwrap();
        assert!(!source.declared.contains_key("x"));
        assert_eq!(catalogue.findings.len(), 1);
        assert_eq!(catalogue.findings[0].kind, FindingKind::MissingField);
    }

    /// Same for a missing `unit`: `UnknownUnit`, the same kind an invalid
    /// spelling of it already gets.
    #[test]
    fn a_declared_metric_missing_unit_is_an_unknown_unit_finding_and_is_dropped() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(RawSource {
                    id: Some("finance".to_string()),
                    file: Some("metrics.json".to_string()),
                    declare: vec![RawDeclared {
                        id: Some("x".to_string()),
                        title: Some("X".to_string()),
                        unit: None,
                        better: Some("higher".to_string()),
                    }],
                    ..Default::default()
                }),
            }],
        };
        let catalogue = validate(&root, &config);
        let source = catalogue.sources.get("finance").unwrap();
        assert!(!source.declared.contains_key("x"));
        assert_eq!(catalogue.findings.len(), 1);
        assert_eq!(catalogue.findings[0].kind, FindingKind::UnknownUnit);
    }

    /// A `source` missing `file` entirely drops the *whole* source (there
    /// is nothing to read without it), as a `MissingField` finding --
    /// never a reason the scope's config file fails to parse.
    #[test]
    fn a_source_missing_file_is_a_missing_field_finding_and_the_whole_source_is_dropped() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(RawSource {
                    id: Some("finance".to_string()),
                    file: None,
                    declare: vec![RawDeclared {
                        id: Some("x".to_string()),
                        title: Some("X".to_string()),
                        unit: Some("count".to_string()),
                        better: Some("higher".to_string()),
                    }],
                    ..Default::default()
                }),
            }],
        };
        let catalogue = validate(&root, &config);
        assert!(catalogue.sources.is_empty());
        assert_eq!(catalogue.findings.len(), 1);
        assert_eq!(catalogue.findings[0].kind, FindingKind::MissingField);
    }

    /// A source missing its own `id` is `BadSlug`, the same kind an
    /// invalid slug already gets -- and, since nothing was registered
    /// under any id, it can never collide with a later `DuplicateSourceId`.
    #[test]
    fn a_source_missing_id_is_a_bad_slug_finding() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(RawSource {
                    id: None,
                    file: Some("metrics.json".to_string()),
                    declare: Vec::new(),
                    ..Default::default()
                }),
            }],
        };
        let catalogue = validate(&root, &config);
        assert!(catalogue.sources.is_empty());
        assert_eq!(catalogue.findings.len(), 1);
        assert_eq!(catalogue.findings[0].kind, FindingKind::BadSlug);
    }

    #[test]
    fn a_duplicate_declared_metric_id_keeps_the_first() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(source(
                    "finance",
                    "metrics.json",
                    &[
                        ("x", "First", "ratio", "higher"),
                        ("x", "Second", "count", "lower"),
                    ],
                )),
            }],
        };
        let catalogue = validate(&root, &config);
        let source = catalogue.sources.get("finance").unwrap();
        assert_eq!(source.declared["x"].title, "First");
        assert_eq!(catalogue.findings[0].kind, FindingKind::DuplicateMetricId);
    }

    #[test]
    fn a_declared_metric_carries_its_own_direction_into_declaredmetric() {
        let root = tmp_root();
        let config = Configuration {
            scopes: vec![ScopeDeclaration {
                scope: scope("finance", "projects/finance"),
                source: Some(source(
                    "finance",
                    "metrics.json",
                    &[
                        ("higher_one", "Higher", "ratio", "higher"),
                        ("lower_one", "Lower", "count", "lower"),
                    ],
                )),
            }],
        };
        let catalogue = validate(&root, &config);
        let source = catalogue.sources.get("finance").unwrap();
        assert_eq!(source.declared["higher_one"].better, Better::Higher);
        assert_eq!(source.declared["lower_one"].better, Better::Lower);

        let directions = directions(&catalogue);
        assert_eq!(
            directions.get("reported.finance.higher_one"),
            Some(&Better::Higher)
        );
        assert_eq!(
            directions.get("reported.finance.lower_one"),
            Some(&Better::Lower)
        );
    }

    // ------------------------------------------------------------ read_source

    #[test]
    fn read_source_parses_a_well_formed_document() {
        let root = tmp_root();
        let path = root.join("metrics.json");
        write(
            &path,
            r#"{"as_of":"2026-10-05T09:00:00Z","metrics":[{"id":"beleg_coverage","value":0.9}]}"#,
        );
        let doc = read_source(&path).unwrap();
        assert_eq!(doc.metrics[0].id, "beleg_coverage");
        assert_eq!(doc.metrics[0].value, Some(0.9));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_source_refuses_a_symlink_even_though_validate_already_checked_once() {
        let root = tmp_root();
        let real = root.join("real.json");
        write(&real, r#"{"as_of":"2026-10-05T09:00:00Z","metrics":[]}"#);
        let link = root.join("metrics.json");
        symlink(&real, &link).unwrap();
        let error = read_source(&link).unwrap_err();
        assert!(error.contains("symlink"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_source_refuses_an_oversized_file() {
        let root = tmp_root();
        let path = root.join("metrics.json");
        let padding = "x".repeat((MAX_SOURCE_BYTES + 100) as usize);
        write(
            &path,
            &format!(r#"{{"as_of":"2026-10-05T09:00:00Z","reason":"{padding}","metrics":[]}}"#),
        );
        let error = read_source(&path).unwrap_err();
        assert!(error.contains("larger than"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_source_refuses_an_unparsable_file() {
        let root = tmp_root();
        let path = root.join("metrics.json");
        write(&path, "not json");
        let error = read_source(&path).unwrap_err();
        assert!(error.contains("does not parse"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_source_requires_as_of() {
        let root = tmp_root();
        let path = root.join("metrics.json");
        write(&path, r#"{"metrics":[{"id":"x","value":1.0}]}"#);
        assert!(read_source(&path).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_source_reports_a_missing_file_by_name_not_as_a_crash() {
        let root = tmp_root();
        let error = read_source(&root.join("never-written.json")).unwrap_err();
        assert!(error.contains("does not exist yet"), "{error}");
    }

    // ---------------------------------------------------------------- value_for

    fn doc(as_of: &str, metrics: Vec<DocMetric>) -> Document {
        Document {
            as_of: as_of.parse().unwrap(),
            metrics,
        }
    }

    fn metric(id: &str, value: Option<f64>) -> DocMetric {
        DocMetric {
            id: id.to_string(),
            value,
            reason: None,
            as_of: None,
        }
    }

    fn one_source_catalogue(declare: &[(&str, &str, Unit)]) -> Catalogue {
        // `better` plays no part in `value_for`'s own job (reading a
        // number), so every caller of this helper gets a fixed, arbitrary
        // direction -- `directions`/the Goals wrong-direction check are
        // covered by their own tests above and in `goals.rs`.
        let mut declared = BTreeMap::new();
        for (id, title, unit) in declare {
            declared.insert(
                id.to_string(),
                DeclaredMetric {
                    title: title.to_string(),
                    unit: *unit,
                    better: Better::Higher,
                },
            );
        }
        let mut sources = BTreeMap::new();
        sources.insert(
            "finance".to_string(),
            ValidSource {
                id: "finance".to_string(),
                scope: scope("finance", "projects/finance"),
                location: Location::File(PathBuf::from("/does-not-matter")),
                declared,
            },
        );
        Catalogue {
            sources,
            findings: Vec::new(),
        }
    }

    fn id(s: &str) -> MetricId {
        MetricId::new(s).unwrap()
    }

    #[test]
    fn value_for_reads_a_present_finite_value_as_of_the_documents_time() {
        let catalogue = one_source_catalogue(&[("beleg_coverage", "Beleg coverage", Unit::Ratio)]);
        let mut docs = BTreeMap::new();
        docs.insert(
            "finance".to_string(),
            Ok(doc(
                "2026-10-05T09:00:00Z",
                vec![metric("beleg_coverage", Some(0.9))],
            )),
        );
        let now = Utc::now();
        let value = value_for(
            &id("reported.finance.beleg_coverage"),
            "finance",
            "beleg_coverage",
            &catalogue,
            &docs,
            true,
            now,
        );
        assert_eq!(value.value, Some(0.9));
        assert_eq!(value.as_of.to_rfc3339(), "2026-10-05T09:00:00+00:00");
        assert_eq!(value.reason, None);
    }

    #[test]
    fn a_per_metric_as_of_overrides_the_documents_own() {
        let catalogue = one_source_catalogue(&[("x", "X", Unit::Count)]);
        let mut entry = metric("x", Some(1.0));
        entry.as_of = Some("2026-09-01T00:00:00Z".parse().unwrap());
        let mut docs = BTreeMap::new();
        docs.insert(
            "finance".to_string(),
            Ok(doc("2026-10-05T09:00:00Z", vec![entry])),
        );
        let value = value_for(
            &id("reported.finance.x"),
            "finance",
            "x",
            &catalogue,
            &docs,
            true,
            Utc::now(),
        );
        assert_eq!(value.as_of.to_rfc3339(), "2026-09-01T00:00:00+00:00");
    }

    #[test]
    fn an_unknown_source_is_none_with_a_reason() {
        let catalogue = one_source_catalogue(&[]);
        let value = value_for(
            &id("reported.nope.x"),
            "nope",
            "x",
            &catalogue,
            &BTreeMap::new(),
            true,
            Utc::now(),
        );
        assert_eq!(value.value, None);
        assert!(value.reason.unwrap().contains("no scope declares"));
    }

    #[test]
    fn a_source_outside_the_selected_subtree_is_none_with_a_reason() {
        let catalogue = one_source_catalogue(&[("x", "X", Unit::Count)]);
        let mut docs = BTreeMap::new();
        docs.insert(
            "finance".to_string(),
            Ok(doc("2026-10-05T09:00:00Z", vec![metric("x", Some(1.0))])),
        );
        let value = value_for(
            &id("reported.finance.x"),
            "finance",
            "x",
            &catalogue,
            &docs,
            false,
            Utc::now(),
        );
        assert_eq!(value.value, None);
        assert!(value.reason.unwrap().contains("outside the selected subtree"));
    }

    #[test]
    fn an_undeclared_id_is_none_with_a_reason_even_if_the_file_reports_it() {
        let catalogue = one_source_catalogue(&[("declared_one", "D", Unit::Count)]);
        let mut docs = BTreeMap::new();
        docs.insert(
            "finance".to_string(),
            Ok(doc(
                "2026-10-05T09:00:00Z",
                vec![metric("undeclared_one", Some(1.0))],
            )),
        );
        let value = value_for(
            &id("reported.finance.undeclared_one"),
            "finance",
            "undeclared_one",
            &catalogue,
            &docs,
            true,
            Utc::now(),
        );
        assert_eq!(value.value, None);
        assert!(value.reason.unwrap().contains("does not declare"));
    }

    #[test]
    fn a_read_failure_is_none_with_the_sources_own_reason() {
        let catalogue = one_source_catalogue(&[("x", "X", Unit::Count)]);
        let mut docs = BTreeMap::new();
        docs.insert("finance".to_string(), Err("the declared metrics file does not exist yet".to_string()));
        let value = value_for(
            &id("reported.finance.x"),
            "finance",
            "x",
            &catalogue,
            &docs,
            true,
            Utc::now(),
        );
        assert_eq!(value.value, None);
        assert_eq!(value.reason.unwrap(), "the declared metrics file does not exist yet");
    }

    #[test]
    fn an_id_the_document_omits_is_none_with_a_reason() {
        let catalogue = one_source_catalogue(&[("x", "X", Unit::Count)]);
        let mut docs = BTreeMap::new();
        docs.insert(
            "finance".to_string(),
            Ok(doc("2026-10-05T09:00:00Z", vec![])),
        );
        let value = value_for(
            &id("reported.finance.x"),
            "finance",
            "x",
            &catalogue,
            &docs,
            true,
            Utc::now(),
        );
        assert_eq!(value.value, None);
        assert!(value.reason.unwrap().contains("does not report"));
    }

    #[test]
    fn a_null_value_with_a_reason_is_a_first_class_answer() {
        let catalogue = one_source_catalogue(&[("x", "X", Unit::Count)]);
        let mut entry = metric("x", None);
        entry.reason = Some("no booking date recorded before 2026-11".to_string());
        let mut docs = BTreeMap::new();
        docs.insert(
            "finance".to_string(),
            Ok(doc("2026-10-05T09:00:00Z", vec![entry])),
        );
        let value = value_for(
            &id("reported.finance.x"),
            "finance",
            "x",
            &catalogue,
            &docs,
            true,
            Utc::now(),
        );
        assert_eq!(value.value, None);
        assert_eq!(
            value.reason.unwrap(),
            "no booking date recorded before 2026-11"
        );
    }

    #[test]
    fn a_null_value_with_no_reason_still_gets_one() {
        let catalogue = one_source_catalogue(&[("x", "X", Unit::Count)]);
        let mut docs = BTreeMap::new();
        docs.insert(
            "finance".to_string(),
            Ok(doc("2026-10-05T09:00:00Z", vec![metric("x", None)])),
        );
        let value = value_for(
            &id("reported.finance.x"),
            "finance",
            "x",
            &catalogue,
            &docs,
            true,
            Utc::now(),
        );
        assert_eq!(value.value, None);
        assert!(value.reason.is_some());
    }

    #[test]
    fn a_non_finite_value_is_none_never_the_literal_number() {
        let catalogue = one_source_catalogue(&[("x", "X", Unit::Count)]);
        let mut docs = BTreeMap::new();
        docs.insert(
            "finance".to_string(),
            Ok(doc(
                "2026-10-05T09:00:00Z",
                vec![metric("x", Some(f64::INFINITY))],
            )),
        );
        let value = value_for(
            &id("reported.finance.x"),
            "finance",
            "x",
            &catalogue,
            &docs,
            true,
            Utc::now(),
        );
        assert_eq!(value.value, None);
        assert!(value.reason.unwrap().contains("non-finite"));
    }

    #[test]
    fn a_reported_zero_is_a_real_zero_not_a_refusal() {
        let catalogue = one_source_catalogue(&[("x", "X", Unit::Count)]);
        let mut docs = BTreeMap::new();
        docs.insert(
            "finance".to_string(),
            Ok(doc("2026-10-05T09:00:00Z", vec![metric("x", Some(0.0))])),
        );
        let value = value_for(
            &id("reported.finance.x"),
            "finance",
            "x",
            &catalogue,
            &docs,
            true,
            Utc::now(),
        );
        assert_eq!(value.value, Some(0.0));
        assert_eq!(value.reason, None);
    }

    #[test]
    fn a_long_reason_from_the_file_is_bounded() {
        let catalogue = one_source_catalogue(&[("x", "X", Unit::Count)]);
        let mut entry = metric("x", None);
        entry.reason = Some("y".repeat(MAX_REASON_CHARS * 2));
        let mut docs = BTreeMap::new();
        docs.insert(
            "finance".to_string(),
            Ok(doc("2026-10-05T09:00:00Z", vec![entry])),
        );
        let value = value_for(
            &id("reported.finance.x"),
            "finance",
            "x",
            &catalogue,
            &docs,
            true,
            Utc::now(),
        );
        assert_eq!(value.reason.unwrap().len(), MAX_REASON_CHARS);
    }

    // ------------------------------------------------- service sources (#333)

    fn network(name: &str, endpoints: &[&str]) -> RawService {
        RawService {
            name: name.to_string(),
            network: true,
            endpoints: endpoints.iter().map(|e| e.to_string()).collect(),
        }
    }

    fn service_source(service: &str, path: Option<&str>, services: Vec<RawService>) -> RawSource {
        RawSource {
            id: Some("finance".to_string()),
            service: Some(service.to_string()),
            path: path.map(str::to_string),
            declare: vec![RawDeclared {
                id: Some("x".to_string()),
                title: Some("X".to_string()),
                unit: Some("count".to_string()),
                better: Some("lower".to_string()),
            }],
            services,
            ..Default::default()
        }
    }

    fn validate_one(source: RawSource) -> Catalogue {
        validate(
            &tmp_root(),
            &Configuration {
                scopes: vec![ScopeDeclaration {
                    scope: scope("finance", "projects/finance"),
                    source: Some(source),
                }],
            },
        )
    }

    #[test]
    fn a_service_source_takes_the_first_loopback_endpoint_even_when_a_tailnet_one_comes_first() {
        let catalogue = validate_one(service_source(
            "fava",
            Some("/buchhaltung/extension/FactoryMetrics/metrics"),
            vec![network(
                "fava",
                &["https://factory.tail1234.ts.net:8793/buchhaltung", "127.0.0.1:8793"],
            )],
        ));
        assert!(catalogue.findings.is_empty(), "{:?}", catalogue.findings);
        let source = &catalogue.sources["finance"];
        let Location::Service(target) = &source.location else {
            panic!("{:?}", source.location)
        };
        assert_eq!(target.endpoint, "127.0.0.1:8793");
        assert_eq!(target.host, LoopbackHost::V4);
        assert_eq!(
            target.url(),
            "http://127.0.0.1:8793/buchhaltung/extension/FactoryMetrics/metrics"
        );
        assert!(source.declared.contains_key("x"), "declared metrics are kept");
    }

    #[test]
    fn every_loopback_spelling_is_accepted_and_nothing_else_is() {
        for (endpoint, host, port) in [
            ("127.0.0.1:8793", LoopbackHost::V4, 8793),
            ("http://127.0.0.1:8793/ignored/path", LoopbackHost::V4, 8793),
            ("localhost:9000", LoopbackHost::Localhost, 9000),
            ("[::1]:1", LoopbackHost::V6, 1),
        ] {
            let (_, got_host, got_port) = loopback(endpoint).unwrap_or_else(|| panic!("{endpoint}"));
            assert_eq!((got_host, got_port), (host, port), "{endpoint}");
        }
        for refused in [
            "https://127.0.0.1:8793",
            "127.0.0.1",
            "localhost",
            "127.0.0.2:80",
            "10.0.0.1:80",
            "factory.tail1234.ts.net:8793",
            "user@127.0.0.1:80",
            "127.0.0.1:0",
            "127.0.0.1:99999",
            "127.0.0.1:80x",
            "[::2]:80",
            "localhost.evil.test:80",
        ] {
            assert!(loopback(refused).is_none(), "{refused} must not read as loopback");
        }
    }

    /// Each refused declaration keeps its source -- so its declared metrics
    /// still list -- reads `None` with the reason, and raises the finding.
    fn assert_refused(catalogue: &Catalogue, kind: FindingKind, reason_has: &str) {
        assert_eq!(catalogue.findings.len(), 1, "{:?}", catalogue.findings);
        assert_eq!(catalogue.findings[0].kind, kind);
        let source = &catalogue.sources["finance"];
        let Location::Refused { reason, .. } = &source.location else {
            panic!("{:?}", source.location)
        };
        assert!(reason.contains(reason_has), "{reason}");
        assert!(source.declared.contains_key("x"));
        let mut docs = BTreeMap::new();
        docs.insert("finance".to_string(), Err(reason.clone()));
        let value = value_for(
            &id("reported.finance.x"),
            "finance",
            "x",
            catalogue,
            &docs,
            true,
            Utc::now(),
        );
        assert_eq!(value.value, None);
        assert!(value.reason.unwrap().contains(reason_has));
    }

    #[test]
    fn a_tailnet_only_service_is_refused() {
        let catalogue = validate_one(service_source(
            "fava",
            Some("/metrics"),
            vec![network("fava", &["https://factory.tail1234.ts.net:8793/buchhaltung"])],
        ));
        assert_refused(&catalogue, FindingKind::NoLoopbackEndpoint, "no loopback endpoint");
    }

    #[test]
    fn an_unknown_service_is_refused() {
        let catalogue = validate_one(service_source(
            "nope",
            Some("/metrics"),
            vec![network("fava", &["127.0.0.1:8793"])],
        ));
        assert_refused(&catalogue, FindingKind::UnknownService, "no dependency service \"nope\"");
    }

    #[test]
    fn a_non_network_service_is_refused() {
        let catalogue = validate_one(service_source(
            "ledger",
            Some("/metrics"),
            vec![RawService {
                name: "ledger".to_string(),
                network: false,
                endpoints: Vec::new(),
            }],
        ));
        assert_refused(&catalogue, FindingKind::NotANetworkService, "not a network service");
    }

    #[test]
    fn file_and_service_together_are_refused() {
        let mut source = service_source(
            "fava",
            Some("/metrics"),
            vec![network("fava", &["127.0.0.1:8793"])],
        );
        source.file = Some("metrics.json".to_string());
        let catalogue = validate_one(source);
        assert_refused(&catalogue, FindingKind::FileAndService, "both file and service");
    }

    #[test]
    fn a_service_source_without_a_path_or_with_a_bad_one_is_refused() {
        let services = || vec![network("fava", &["127.0.0.1:8793"])];
        let catalogue = validate_one(service_source("fava", None, services()));
        assert_refused(&catalogue, FindingKind::MissingField, "no path");
        for bad in ["metrics", "/a b", "/a#b", "http://elsewhere/x"] {
            let catalogue = validate_one(service_source("fava", Some(bad), services()));
            assert_refused(&catalogue, FindingKind::BadServicePath, "not a request path");
        }
    }

    // ------------------------------------------------------------ read_service

    /// A one-shot local HTTP server: accepts connections on an ephemeral
    /// loopback port, counts them, reads each request head and writes
    /// `response` (or, for `None`, holds the connection open and says
    /// nothing). Returns the target to read and the request counter.
    async fn serve(
        response: Option<Vec<u8>>,
    ) -> (ServiceTarget, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                counter.fetch_add(1, Ordering::SeqCst);
                let response = response.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        match stream.read(&mut byte).await {
                            Ok(1) => head.push(byte[0]),
                            _ => return,
                        }
                    }
                    match response {
                        Some(bytes) => {
                            let _ = stream.write_all(&bytes).await;
                            let _ = stream.shutdown().await;
                        }
                        None => tokio::time::sleep(std::time::Duration::from_secs(30)).await,
                    }
                });
            }
        });
        let target = ServiceTarget {
            service: "fava".to_string(),
            endpoint: format!("127.0.0.1:{port}"),
            host: LoopbackHost::V4,
            port,
            path: "/metrics".to_string(),
        };
        (target, hits)
    }

    fn ok(body: &str) -> Option<Vec<u8>> {
        Some(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
        )
    }

    const DOC: &str = r#"{"as_of":"2026-10-05T09:00:00Z","metrics":[{"id":"x","value":3}]}"#;
    const QUICK: std::time::Duration = std::time::Duration::from_millis(300);

    #[tokio::test]
    async fn read_service_parses_the_same_document_a_file_holds() {
        let (target, hits) = serve(ok(DOC)).await;
        let doc = read_service(&target, QUICK).await.unwrap();
        assert_eq!(doc.metrics[0].id, "x");
        assert_eq!(doc.metrics[0].value, Some(3.0));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn read_service_decodes_a_chunked_body_and_a_close_delimited_one() {
        let half = DOC.len() / 2;
        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            half,
            &DOC[..half],
            DOC.len() - half,
            &DOC[half..]
        );
        let (target, _) = serve(Some(chunked.into_bytes())).await;
        assert_eq!(read_service(&target, QUICK).await.unwrap().metrics.len(), 1);

        let delimited = format!("HTTP/1.0 200 OK\r\n\r\n{DOC}");
        let (target, _) = serve(Some(delimited.into_bytes())).await;
        assert_eq!(read_service(&target, QUICK).await.unwrap().metrics.len(), 1);
    }

    #[tokio::test]
    async fn a_non_2xx_answer_is_a_reason() {
        let (target, _) = serve(Some(
            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ))
        .await;
        let error = read_service(&target, QUICK).await.unwrap_err();
        assert!(error.contains("answered HTTP 500"), "{error}");
    }

    #[tokio::test]
    async fn a_redirect_is_never_followed() {
        let (elsewhere, elsewhere_hits) = serve(ok(DOC)).await;
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\n\r\n",
            elsewhere.url()
        );
        let (target, hits) = serve(Some(redirect.into_bytes())).await;
        let error = read_service(&target, QUICK).await.unwrap_err();
        assert!(error.contains("answered HTTP 302"), "{error}");
        assert!(error.contains("redirects are not followed"), "{error}");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(elsewhere_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_body_that_does_not_parse_is_a_reason() {
        let (target, _) = serve(ok("<html>Fava</html>")).await;
        let error = read_service(&target, QUICK).await.unwrap_err();
        assert!(error.contains("does not parse"), "{error}");
        // `as_of` is required here exactly as it is in a file.
        let (target, _) = serve(ok(r#"{"metrics":[]}"#)).await;
        assert!(read_service(&target, QUICK).await.unwrap_err().contains("does not parse"));
    }

    #[tokio::test]
    async fn a_body_over_the_size_bound_is_a_reason_whether_declared_or_streamed() {
        let padding = "x".repeat(MAX_SOURCE_BYTES as usize + 10);
        let big = format!(r#"{{"as_of":"2026-10-05T09:00:00Z","pad":"{padding}","metrics":[]}}"#);
        let (target, _) = serve(ok(&big)).await;
        let error = read_service(&target, QUICK).await.unwrap_err();
        assert!(error.contains("larger than"), "{error}");

        let streamed = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{big}");
        let (target, _) = serve(Some(streamed.into_bytes())).await;
        let error = read_service(&target, QUICK).await.unwrap_err();
        assert!(error.contains("larger than"), "{error}");
    }

    #[tokio::test]
    async fn a_service_that_never_answers_times_out() {
        let (target, _) = serve(None).await;
        let started = std::time::Instant::now();
        let error = read_service(&target, QUICK).await.unwrap_err();
        assert!(error.contains("did not answer within"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_service_nobody_listens_on_is_a_reason() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let target = ServiceTarget {
            service: "fava".to_string(),
            endpoint: format!("localhost:{port}"),
            host: LoopbackHost::Localhost,
            port,
            path: "/metrics".to_string(),
        };
        let error = read_service(&target, QUICK).await.unwrap_err();
        assert!(error.contains("could not be reached"), "{error}");
    }

    #[tokio::test]
    async fn read_services_makes_one_request_per_source() {
        let (target, hits) = serve(ok(DOC)).await;
        let read = read_services(vec![("finance".to_string(), target)], QUICK).await;
        assert!(read["finance"].is_ok());
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
