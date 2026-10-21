//! Scope pages (`#343`): an HTML page a scope's agents publish for a person
//! to look at, shown by the web UI in a sandboxed frame and as a dashboard
//! tile.
//!
//! A page is declared by its scope (`scope.pages`, [`PageDecl`]) and its
//! content arrives at runtime, through `factory page publish`. L4 owns it,
//! because publishing is something a run does and is journaled on that run.
//!
//! Versions are content-addressed under the instance's own
//! `.factory/pages/<scope>/<name>/<sha256>/` -- never inside a scope's
//! directory, so publishing never changes a scope's source. A `current`
//! file beside them names the version shown: a publish points it at the new
//! version, a rollback at an older one, and either survives a restart because
//! it is a file. The last `keep` versions are kept; the current one never
//! goes, however old.
//!
//! What a page is allowed to do in the browser is not decided here -- that
//! is the strict CSP the daemon serves it with ([`PAGE_CSP`]) and the
//! sandbox the UI frames it in. [`csp_findings`] only looks for what that
//! CSP would certainly refuse to load, so such a page is refused at publish
//! with the reason rather than stored and shown half broken.

use async_trait::async_trait;
use factory_kernel::{FactoryError, PageVersionFact, Provide, Result, ScopePageFact, ScopeTree};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Under the instance root's `.factory/`.
pub const PAGES_DIR: &str = "pages";

/// The page's own Content-Security-Policy, sent with every page response.
/// The issue's policy, plus `sandbox allow-scripts` -- so a page opened on
/// its own, outside the UI's frame, still runs in an opaque origin -- and
/// `base-uri`/`form-action` closed.
pub const PAGE_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
     img-src data: blob:; font-src data:; connect-src 'none'; frame-ancestors 'self'; \
     base-uri 'none'; form-action 'none'; sandbox allow-scripts";

/// `max_bytes` when a declaration names none: room for a page with its map
/// tiles embedded, like the conference radar's 1.7 MB.
pub const DEFAULT_MAX_BYTES: u64 = 20_000_000;
/// The most any declaration may allow. A sandboxed run publishes over
/// `POST /api/rpc`, whose body limit is sized from this.
pub const MAX_BYTES_CEILING: u64 = 32 * 1024 * 1024;
/// Versions kept when a declaration names no `keep`.
pub const DEFAULT_KEEP: u32 = 10;
pub const MAX_KEEP: u32 = 100;

const CURRENT_FILE: &str = "current";
const VERSION_FILE: &str = "version.json";
const HTML_FILE: &str = "page.html";
const DATA_FILE: &str = "data.json";

// ------------------------------------------------------------- declaration

/// A byte count as a config file writes it: a plain number of bytes, or a
/// number with a unit -- `KB`/`MB`/`GB` (powers of 1000) or
/// `KiB`/`MiB`/`GiB` (powers of 1024). `20MB` is 20,000,000 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteSize(pub u64);

impl ByteSize {
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let text = text.trim();
        let split = text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len());
        let (number, unit) = text.split_at(split);
        let number: u64 = number
            .parse()
            .map_err(|_| format!("{text:?} is not a size: write bytes, or a number with KB, MB, GB, KiB, MiB or GiB"))?;
        let factor: u64 = match unit.trim() {
            "" | "B" => 1,
            "KB" | "kB" => 1_000,
            "MB" => 1_000_000,
            "GB" => 1_000_000_000,
            "KiB" => 1 << 10,
            "MiB" => 1 << 20,
            "GiB" => 1 << 30,
            other => {
                return Err(format!(
                    "{text:?} has the unit {other:?}; write KB, MB, GB, KiB, MiB or GiB"
                ))
            }
        };
        number
            .checked_mul(factor)
            .map(ByteSize)
            .ok_or_else(|| format!("{text:?} is too large"))
    }
}

impl Serialize for ByteSize {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Bytes(u64),
            Text(String),
        }
        match Written::deserialize(deserializer)? {
            Written::Bytes(bytes) => Ok(ByteSize(bytes)),
            Written::Text(text) => ByteSize::parse(&text).map_err(serde::de::Error::custom),
        }
    }
}

/// One entry of a scope's `pages:` -- what the scope may publish. The
/// content is not here: it arrives at runtime, through `factory page
/// publish`. `deny_unknown_fields`, so a typo (`max_byte:`) is refused
/// rather than quietly ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageDecl {
    /// A slug -- lowercase letters, digits and `-` -- because it is a
    /// directory name and a URL segment.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<ByteSize>,
    /// How many versions to keep for rollback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<u32>,
}

impl PageDecl {
    pub fn title(&self) -> String {
        self.title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(&self.name)
            .to_string()
    }
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.map(|b| b.0).unwrap_or(DEFAULT_MAX_BYTES)
    }
    pub fn keep(&self) -> u32 {
        self.keep.unwrap_or(DEFAULT_KEEP)
    }
}

/// A scope's whole `pages:` list, checked the way config load checks
/// everything else: the scope and the entry named in the message.
pub fn validate_declarations(scope: &str, pages: &[PageDecl]) -> std::result::Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for (i, page) in pages.iter().enumerate() {
        let at = format!("scope {scope:?} pages[{i}]");
        if !factory_kernel::is_slug(&page.name) {
            return Err(format!(
                "{at} is named {:?}; a page name is lowercase letters, digits and `-`, starting with a letter or digit",
                page.name
            ));
        }
        if !seen.insert(page.name.as_str()) {
            return Err(format!("{at}: scope {scope:?} declares the page {:?} twice", page.name));
        }
        let max = page.max_bytes();
        if max == 0 || max > MAX_BYTES_CEILING {
            return Err(format!(
                "{at} ({:?}) has max_bytes {max}; it must be at least 1 and at most {MAX_BYTES_CEILING} (32 MiB)",
                page.name
            ));
        }
        let keep = page.keep();
        if keep == 0 || keep > MAX_KEEP {
            return Err(format!(
                "{at} ({:?}) keeps {keep} versions; keep at least 1 and at most {MAX_KEEP}",
                page.name
            ));
        }
    }
    Ok(())
}

/// A dashboard tile's `page:` -- either a bare page name, resolved from the
/// scope whose `dashboard:` block names it upward, or `<scope>/<name>`,
/// exactly that scope's page. A page name never contains `/`, so the last one
/// splits them, whatever the scope's own name contains.
pub fn split_page_reference(reference: &str) -> (Option<&str>, &str) {
    match reference.rsplit_once('/') {
        Some((scope, name)) => (Some(scope), name),
        None => (None, reference),
    }
}

// ------------------------------------------------------------ CSP preflight

/// What `PAGE_CSP` would refuse to load or do, found in the page's markup
/// before it is stored: one message per finding, with its line. Empty means
/// nothing static stands in the way.
///
/// A preflight, not the guard: it reads tags, attributes and CSS, the way a
/// browser would find a resource to fetch, and never runs anything. What a
/// script builds at runtime is still refused by the CSP itself, in the
/// browser. WebRTC is not governed by CSP at all, so the words that open a
/// peer connection are refused here too.
pub fn csp_findings(html: &str) -> Vec<String> {
    let mut findings = Vec::new();
    let mut scanner = Scanner { html, at: 0 };
    while let Some(tag) = scanner.next_tag() {
        check_tag(html, &tag, &mut findings);
        if let Some(raw) = RAW_TEXT.iter().find(|t| **t == tag.name) {
            let body_start = scanner.at;
            let body_end = scanner.skip_raw_text(raw);
            if tag.name == "style" {
                check_css(html, body_start, &html[body_start..body_end], "<style>", &mut findings);
            }
        }
    }
    for word in ["RTCPeerConnection", "RTCDataChannel"] {
        if let Some(at) = html.find(word) {
            findings.push(format!(
                "line {}: {word} -- a page may not open a WebRTC connection (no CSP governs one)",
                line_of(html, at)
            ));
        }
    }
    findings
}

/// Elements whose content is text, never markup, so a `<` inside one is not a tag.
const RAW_TEXT: &[&str] = &["script", "style", "textarea", "title", "xmp", "noscript", "noembed", "noframes"];

struct Tag {
    name: String,
    at: usize,
    attrs: Vec<(String, String)>,
}

impl Tag {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

struct Scanner<'a> {
    html: &'a str,
    at: usize,
}

impl Scanner<'_> {
    fn next_tag(&mut self) -> Option<Tag> {
        let bytes = self.html.as_bytes();
        loop {
            let open = self.at + self.html[self.at..].find('<')?;
            let rest = &self.html[open..];
            if rest.starts_with("<!--") {
                self.at = rest.find("-->").map(|i| open + i + 3).unwrap_or(self.html.len());
                continue;
            }
            if rest.starts_with("<!") || rest.starts_with("<?") || rest.starts_with("</") {
                self.at = rest.find('>').map(|i| open + i + 1).unwrap_or(self.html.len());
                continue;
            }
            let mut i = open + 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b':') {
                i += 1;
            }
            if i == open + 1 {
                self.at = open + 1;
                continue;
            }
            let name = self.html[open + 1..i].to_ascii_lowercase();
            let mut attrs = Vec::new();
            loop {
                while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
                    i += 1;
                }
                if i >= bytes.len() {
                    break;
                }
                if bytes[i] == b'>' {
                    i += 1;
                    break;
                }
                let start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'=' | b'>' | b'/') {
                    i += 1;
                }
                let attr = self.html[start..i].to_ascii_lowercase();
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let mut value = String::new();
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                        let quote = bytes[i];
                        let end = self.html[i + 1..]
                            .find(quote as char)
                            .map(|e| i + 1 + e)
                            .unwrap_or(bytes.len());
                        value = self.html[i + 1..end].to_string();
                        i = (end + 1).min(bytes.len());
                    } else {
                        let start = i;
                        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                            i += 1;
                        }
                        value = self.html[start..i].to_string();
                    }
                }
                if !attr.is_empty() {
                    attrs.push((attr, value));
                }
            }
            self.at = i;
            return Some(Tag { name, at: open, attrs });
        }
    }

    /// Past the end of a raw-text element's body; returns where the body ends.
    fn skip_raw_text(&mut self, name: &str) -> usize {
        let bytes = self.html.as_bytes();
        let mut from = self.at;
        while let Some(i) = self.html[from..].find("</") {
            let at = from + i;
            let after = &bytes[at + 2..];
            if after.len() >= name.len() && after[..name.len()].eq_ignore_ascii_case(name.as_bytes()) {
                self.at = at;
                return at;
            }
            from = at + 2;
        }
        self.at = self.html.len();
        self.html.len()
    }
}

fn line_of(html: &str, at: usize) -> usize {
    html[..at].bytes().filter(|b| *b == b'\n').count() + 1
}

/// `data:` and `blob:` are the only places `PAGE_CSP` loads anything from.
fn is_embedded(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    url.starts_with("data:") || url.starts_with("blob:")
}

fn short(url: &str) -> String {
    let url = url.trim();
    if url.chars().count() > 80 {
        format!("{}…", url.chars().take(80).collect::<String>())
    } else {
        url.to_string()
    }
}

const LOADING_RELS: &[&str] = &[
    "stylesheet", "icon", "apple-touch-icon", "preload", "prefetch", "prerender", "modulepreload",
    "preconnect", "dns-prefetch", "manifest", "import",
];

fn check_tag(html: &str, tag: &Tag, findings: &mut Vec<String>) {
    let line = line_of(html, tag.at);
    let mut found: Vec<String> = Vec::new();
    match tag.name.as_str() {
        "script" => {
            if let Some(src) = tag.attr("src") {
                found.push(format!(
                    "<script src=\"{}\"> -- the page CSP runs inline scripts only; put the script inside the page",
                    short(src)
                ));
            }
        }
        "link" => {
            let rel = tag.attr("rel").unwrap_or("").to_ascii_lowercase();
            let loads = rel.split_whitespace().any(|r| LOADING_RELS.contains(&r));
            let href = tag.attr("href").unwrap_or("");
            if rel.split_whitespace().any(|r| r == "stylesheet") {
                found.push(format!(
                    "<link rel=\"stylesheet\" href=\"{}\"> -- the page CSP allows inline styles only; put the CSS in a <style> element",
                    short(href)
                ));
            } else if loads && !href.is_empty() && !is_embedded(href) {
                found.push(format!(
                    "<link rel=\"{rel}\" href=\"{}\"> -- the page CSP loads nothing from the network",
                    short(href)
                ));
            }
        }
        "iframe" | "frame" | "frameset" | "object" | "embed" | "applet" | "portal" | "fencedframe" => {
            found.push(format!("<{}> -- the page CSP allows no nested frames, objects or embeds", tag.name));
        }
        "base" => found.push("<base> -- the page CSP forbids changing the base URL (base-uri 'none')".into()),
        "meta" => {
            let refresh = tag
                .attr("http-equiv")
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("refresh"));
            if refresh {
                found.push("<meta http-equiv=\"refresh\"> -- a page may not navigate itself away".into());
            }
        }
        _ => {}
    }
    for (attr, value) in &tag.attrs {
        match attr.as_str() {
            // `<script src>` and a frame or embed are already one finding each.
            "src" | "poster" | "background"
                if !matches!(
                    tag.name.as_str(),
                    "script" | "iframe" | "frame" | "frameset" | "object" | "embed" | "applet" | "portal" | "fencedframe"
                ) =>
            {
                if !value.trim().is_empty() && !is_embedded(value) {
                    found.push(format!(
                        "<{} {attr}=\"{}\"> -- the page CSP loads images and media only from data: or blob: URLs; embed it",
                        tag.name,
                        short(value)
                    ));
                }
            }
            "srcset" => {
                for candidate in value.split(',') {
                    let url = candidate.split_whitespace().next().unwrap_or("");
                    if !url.is_empty() && !is_embedded(url) {
                        found.push(format!(
                            "<{} srcset> names \"{}\" -- the page CSP loads images only from data: or blob: URLs",
                            tag.name,
                            short(url)
                        ));
                    }
                }
            }
            "href" | "xlink:href" if matches!(tag.name.as_str(), "image" | "use" | "feimage") => {
                let v = value.trim();
                if !v.is_empty() && !v.starts_with('#') && !is_embedded(v) {
                    found.push(format!(
                        "<{} {attr}=\"{}\"> -- the page CSP loads images only from data: or blob: URLs",
                        tag.name,
                        short(v)
                    ));
                }
            }
            "ping" if !value.trim().is_empty() => {
                found.push(format!("<{} ping> -- the page CSP makes no network requests", tag.name));
            }
            _ => {}
        }
    }
    findings.extend(found.into_iter().map(|what| format!("line {line}: {what}")));
    if let Some(style) = tag.attr("style") {
        check_css(html, tag.at, style, "a style attribute", findings);
    }
}

fn check_css(html: &str, at: usize, css: &str, place: &str, findings: &mut Vec<String>) {
    let lower = css.to_ascii_lowercase();
    if let Some(i) = lower.find("@import") {
        findings.push(format!(
            "line {}: @import in {place} -- the page CSP loads no stylesheets; inline it",
            line_of(html, at) + css[..i].bytes().filter(|b| *b == b'\n').count()
        ));
    }
    let mut from = 0;
    while let Some(i) = lower[from..].find("url(") {
        let start = from + i + 4;
        let end = lower[start..].find(')').map(|e| start + e).unwrap_or(lower.len());
        let url = css[start..end].trim().trim_matches(|c| c == '"' || c == '\'').trim();
        if !url.is_empty() && !url.starts_with('#') && !is_embedded(url) {
            findings.push(format!(
                "line {}: url({}) in {place} -- the page CSP loads images and fonts only from data: or blob: URLs; embed it",
                line_of(html, at) + css[..start].bytes().filter(|b| *b == b'\n').count(),
                short(url)
            ));
        }
        from = end;
    }
}

// ------------------------------------------------------------------- store

/// What a publish hands the store. Everything about who and when is filled
/// in by the caller; the store adds only the address.
#[derive(Debug, Clone)]
pub struct NewPageVersion {
    pub scope: String,
    pub name: String,
    pub title: String,
    pub html: String,
    pub data: Option<serde_json::Value>,
    pub published_by: String,
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    pub keep: u32,
}

/// One version's content, as read back for serving.
#[derive(Debug, Clone, PartialEq)]
pub struct PageContent {
    pub version: PageVersionFact,
    pub html: Option<String>,
    pub data: Option<serde_json::Value>,
}

/// The version address: SHA-256 over the HTML's length, the HTML and the
/// data, so a page whose data changed is a new version as surely as one
/// whose markup did.
pub fn version_address(html: &[u8], data: Option<&[u8]>) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"factory-page-v1\0");
    hasher.update((html.len() as u64).to_be_bytes());
    hasher.update(html);
    if let Some(data) = data {
        hasher.update(data);
    }
    format!("{:x}", hasher.finalize())
}

fn is_address(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A scope name as one directory name: anything but letters, digits, `-`,
/// `_` and `.` is percent-encoded, so `a/b` is `a%2Fb` and can never be
/// mistaken for scope `a`'s page `b`. A leading dot is encoded too.
pub fn scope_directory(scope: &str) -> String {
    let mut out = String::new();
    for (i, b) in scope.bytes().enumerate() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_') || (b == b'.' && i > 0);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn io(what: impl std::fmt::Display, error: std::io::Error) -> FactoryError {
    FactoryError::Other(anyhow::anyhow!("{what}: {error}"))
}

/// The instance's pages, on disk under `.factory/pages/`. Cheap to clone;
/// every clone shares one lock, so a publish and a rollback never interleave.
#[derive(Debug, Clone)]
pub struct PageStore {
    root: PathBuf,
    edit: Arc<tokio::sync::Mutex<()>>,
}

impl PageStore {
    /// `factory_dir` is the instance root's `.factory/`.
    pub fn new(factory_dir: &Path) -> Self {
        Self {
            root: factory_dir.join(PAGES_DIR),
            edit: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn page_dir(&self, scope: &str, name: &str) -> Result<PathBuf> {
        if !factory_kernel::is_slug(name) {
            return Err(FactoryError::BadRequest(format!("{name:?} is not a page name")));
        }
        if scope.is_empty() {
            return Err(FactoryError::BadRequest("a page belongs to a scope; none was named".into()));
        }
        Ok(self.root.join(scope_directory(scope)).join(name))
    }

    /// Store a version and make it the one shown, then drop what falls past
    /// `keep`. The same content published again is the same version: its
    /// record is rewritten with this publish, and it becomes current.
    pub async fn publish(&self, new: NewPageVersion) -> Result<PageVersionFact> {
        let dir = self.page_dir(&new.scope, &new.name)?;
        let data = new
            .data
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|e| FactoryError::BadRequest(format!("the page data is not JSON: {e}")))?;
        let sha256 = version_address(new.html.as_bytes(), data.as_deref());
        let version = PageVersionFact {
            sha256: sha256.clone(),
            bytes: (new.html.len() + data.as_ref().map_or(0, Vec::len)) as u64,
            data_bytes: data.as_ref().map(|d| d.len() as u64),
            title: new.title.clone(),
            published_at: chrono::Utc::now(),
            published_by: new.published_by.clone(),
            run_id: new.run_id.clone(),
            task_id: new.task_id.clone(),
        };
        let record = serde_json::to_vec_pretty(&version)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("encoding a page version: {e}")))?;

        let _edit = self.edit.lock().await;
        tokio::fs::create_dir_all(&dir).await.map_err(|e| io(dir.display(), e))?;
        let target = dir.join(&sha256);
        if tokio::fs::try_exists(&target).await.unwrap_or(false) {
            write_atomically(&target.join(VERSION_FILE), &record).await?;
        } else {
            let staging = dir.join(format!(".staging-{}", uuid::Uuid::new_v4()));
            tokio::fs::create_dir_all(&staging).await.map_err(|e| io(staging.display(), e))?;
            let written = async {
                tokio::fs::write(staging.join(HTML_FILE), new.html.as_bytes()).await?;
                if let Some(data) = &data {
                    tokio::fs::write(staging.join(DATA_FILE), data).await?;
                }
                tokio::fs::write(staging.join(VERSION_FILE), &record).await?;
                tokio::fs::rename(&staging, &target).await
            }
            .await;
            if let Err(e) = written {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                return Err(io(target.display(), e));
            }
        }
        write_atomically(&dir.join(CURRENT_FILE), sha256.as_bytes()).await?;
        self.prune(&dir, &sha256, new.keep).await;
        Ok(version)
    }

    /// Keep the newest `keep` versions and the current one; remove the rest.
    /// A version that will not go is logged and left, never an error: the
    /// publish that triggered this already succeeded.
    async fn prune(&self, dir: &Path, current: &str, keep: u32) {
        let versions = match read_versions(dir).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("listing page versions to prune: {e}");
                return;
            }
        };
        for version in versions.iter().skip(keep as usize) {
            if version.sha256 == current {
                continue;
            }
            let path = dir.join(&version.sha256);
            if let Err(e) = tokio::fs::remove_dir_all(&path).await {
                tracing::warn!(path = %path.display(), "could not remove an old page version: {e}");
            }
        }
    }

    /// Every kept version, newest first, and the one shown. With no `current`
    /// file, or one naming a version that is gone, the newest is shown.
    pub async fn versions(&self, scope: &str, name: &str) -> Result<(Option<PageVersionFact>, Vec<PageVersionFact>)> {
        let dir = self.page_dir(scope, name)?;
        let versions = read_versions(&dir).await?;
        let pointer = tokio::fs::read_to_string(dir.join(CURRENT_FILE))
            .await
            .ok()
            .map(|s| s.trim().to_string());
        let current = pointer
            .and_then(|sha| versions.iter().find(|v| v.sha256 == sha).cloned())
            .or_else(|| versions.first().cloned());
        Ok((current, versions))
    }

    /// One version by its address or a unique prefix of at least seven
    /// characters; `None` asks for the current one.
    pub async fn resolve(&self, scope: &str, name: &str, version: Option<&str>) -> Result<PageVersionFact> {
        let (current, versions) = self.versions(scope, name).await?;
        let Some(asked) = version.map(str::trim).filter(|v| !v.is_empty()) else {
            return current.ok_or_else(|| {
                FactoryError::BadRequest(format!("page {name:?} of scope {scope:?} is not published yet"))
            });
        };
        let asked = asked.to_ascii_lowercase();
        if asked.len() < 7 || !asked.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(FactoryError::BadRequest(format!(
                "{asked:?} is not a page version; give its sha256, or at least its first seven characters"
            )));
        }
        let matches: Vec<_> = versions.into_iter().filter(|v| v.sha256.starts_with(&asked)).collect();
        match matches.len() {
            1 => Ok(matches.into_iter().next().expect("one match")),
            0 => Err(FactoryError::BadRequest(format!(
                "page {name:?} of scope {scope:?} keeps no version {asked}"
            ))),
            _ => Err(FactoryError::BadRequest(format!(
                "{asked} could be more than one version of page {name:?}; give more of its sha256"
            ))),
        }
    }

    /// A version's HTML and, when asked, its data.
    pub async fn content(
        &self,
        scope: &str,
        name: &str,
        version: Option<&str>,
        html: bool,
        data: bool,
    ) -> Result<PageContent> {
        let found = self.resolve(scope, name, version).await?;
        let dir = self.page_dir(scope, name)?.join(&found.sha256);
        let html = if html {
            let path = dir.join(HTML_FILE);
            Some(tokio::fs::read_to_string(&path).await.map_err(|e| io(path.display(), e))?)
        } else {
            None
        };
        let data = if data && found.data_bytes.is_some() {
            let path = dir.join(DATA_FILE);
            let bytes = tokio::fs::read(&path).await.map_err(|e| io(path.display(), e))?;
            Some(serde_json::from_slice(&bytes).map_err(|e| {
                FactoryError::Other(anyhow::anyhow!("{} is not JSON: {e}", path.display()))
            })?)
        } else {
            None
        };
        Ok(PageContent { version: found, html, data })
    }

    /// Show an older kept version again. A file, so it holds across a restart.
    pub async fn set_current(&self, scope: &str, name: &str, version: &str) -> Result<PageVersionFact> {
        let dir = self.page_dir(scope, name)?;
        let _edit = self.edit.lock().await;
        let found = self.resolve(scope, name, Some(version)).await?;
        write_atomically(&dir.join(CURRENT_FILE), found.sha256.as_bytes()).await?;
        Ok(found)
    }
}

async fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let staging = path.with_extension(format!("staging-{}", uuid::Uuid::new_v4()));
    tokio::fs::write(&staging, bytes).await.map_err(|e| io(staging.display(), e))?;
    if let Err(e) = tokio::fs::rename(&staging, path).await {
        let _ = tokio::fs::remove_file(&staging).await;
        return Err(io(path.display(), e));
    }
    Ok(())
}

/// Every version directory with a readable record, newest first. A missing
/// page directory is no versions at all; an unreadable record is skipped.
async fn read_versions(dir: &Path) -> Result<Vec<PageVersionFact>> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(dir.display(), e)),
    };
    let mut versions = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| io(dir.display(), e))? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_address(&name) {
            continue;
        }
        let record = entry.path().join(VERSION_FILE);
        match tokio::fs::read(&record).await {
            Ok(bytes) => match serde_json::from_slice::<PageVersionFact>(&bytes) {
                Ok(v) if v.sha256 == name => versions.push(v),
                Ok(_) => tracing::warn!(path = %record.display(), "page version record names another address"),
                Err(e) => tracing::warn!(path = %record.display(), "unreadable page version record: {e}"),
            },
            Err(e) => tracing::warn!(path = %record.display(), "unreadable page version record: {e}"),
        }
    }
    versions.sort_by(|a, b| b.published_at.cmp(&a.published_at).then_with(|| a.sha256.cmp(&b.sha256)));
    Ok(versions)
}

// ---------------------------------------------------------------- the fact

/// Which pages to read: a scope and everything below it (all of them when
/// `None`), and optionally one page name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopePagesQuery {
    pub scope: Option<String>,
    pub name: Option<String>,
}

/// The live `ScopePageFact` read: the page store, plus the declarations and
/// scope identities captured fresh from the configuration for this read.
pub struct Provider {
    store: PageStore,
    scopes: ScopeTree,
    declarations: Vec<(String, Vec<PageDecl>)>,
}

impl Provider {
    pub fn new(store: PageStore, scopes: ScopeTree, declarations: Vec<(String, Vec<PageDecl>)>) -> Self {
        Self { store, scopes, declarations }
    }
}

impl factory_kernel::FactProvider for Provider {
    type Level = factory_kernel::L4;
}

#[async_trait]
impl Provide<ScopePageFact> for Provider {
    type Query = ScopePagesQuery;
    type Value = Vec<ScopePageFact>;
    type Error = FactoryError;
    async fn get(&self, query: &ScopePagesQuery) -> Result<Vec<ScopePageFact>> {
        let wanted: Option<Vec<String>> = match query.scope.as_deref() {
            None => None,
            Some(scope) => {
                let (_, subtree) = self.scopes.subtree_scopes(Some(scope))?;
                Some(subtree.into_iter().map(|s| s.name.clone()).collect())
            }
        };
        let mut out = Vec::new();
        for (scope, pages) in &self.declarations {
            if wanted.as_ref().is_some_and(|w| !w.contains(scope)) {
                continue;
            }
            for page in pages {
                if query.name.as_ref().is_some_and(|n| *n != page.name) {
                    continue;
                }
                let (current, versions) = self.store.versions(scope, &page.name).await?;
                out.push(ScopePageFact {
                    scope: scope.clone(),
                    name: page.name.clone(),
                    title: page.title(),
                    max_bytes: page.max_bytes(),
                    keep: page.keep(),
                    current,
                    versions,
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-pages-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn new_version(html: &str, data: Option<serde_json::Value>) -> NewPageVersion {
        NewPageVersion {
            scope: "radar/team".into(),
            name: "conference-radar".into(),
            title: "Konferenz-Radar".into(),
            html: html.into(),
            data,
            published_by: "builder (worker) in radar/team".into(),
            run_id: Some("r1".into()),
            task_id: Some("t1".into()),
            keep: 3,
        }
    }

    #[test]
    fn a_size_is_bytes_or_a_number_with_a_unit() {
        assert_eq!(ByteSize::parse("20MB").unwrap().0, 20_000_000);
        assert_eq!(ByteSize::parse("20 MiB").unwrap().0, 20 * 1024 * 1024);
        assert_eq!(ByteSize::parse("512KB").unwrap().0, 512_000);
        assert_eq!(ByteSize::parse("4096").unwrap().0, 4096);
        assert!(ByteSize::parse("20 parsecs").unwrap_err().contains("unit"));
        assert!(ByteSize::parse("MB").is_err());
        let decl: PageDecl = serde_yaml_ng::from_str("name: radar\nmax_bytes: 20MB\n").unwrap();
        assert_eq!(decl.max_bytes(), 20_000_000);
        let decl: PageDecl = serde_yaml_ng::from_str("name: radar\nmax_bytes: 1000\n").unwrap();
        assert_eq!(decl.max_bytes(), 1000);
        assert!(serde_yaml_ng::from_str::<PageDecl>("name: radar\nmax_byte: 1\n").is_err());
    }

    #[test]
    fn declarations_are_slugs_unique_and_bounded() {
        let ok = vec![PageDecl { name: "conference-radar".into(), title: None, max_bytes: None, keep: None }];
        validate_declarations("radar", &ok).unwrap();
        let bad = |name: &str| PageDecl { name: name.into(), title: None, max_bytes: None, keep: None };
        for name in ["../x", "Upper", "a/b", "", "a_b"] {
            let e = validate_declarations("radar", &[bad(name)]).unwrap_err();
            assert!(e.contains("scope \"radar\" pages[0]"), "{e}");
        }
        let e = validate_declarations("radar", &[bad("a"), bad("a")]).unwrap_err();
        assert!(e.contains("twice"), "{e}");
        let big = PageDecl { max_bytes: Some(ByteSize(MAX_BYTES_CEILING + 1)), ..bad("a") };
        assert!(validate_declarations("radar", &[big]).unwrap_err().contains("max_bytes"));
        let none = PageDecl { keep: Some(0), ..bad("a") };
        assert!(validate_declarations("radar", &[none]).unwrap_err().contains("keep"));
    }

    #[test]
    fn a_reference_splits_at_its_last_slash() {
        assert_eq!(split_page_reference("radar"), (None, "radar"));
        assert_eq!(split_page_reference("ingo/conferenz-radar/radar"), (Some("ingo/conferenz-radar"), "radar"));
    }

    #[test]
    fn a_scope_name_is_one_directory_never_a_path() {
        assert_eq!(scope_directory("radar"), "radar");
        assert_eq!(scope_directory("a/b"), "a%2Fb");
        assert_eq!(scope_directory(".."), "%2E.");
        assert_ne!(scope_directory("a/b"), scope_directory("a%2Fb"));
    }

    #[test]
    fn the_preflight_finds_what_the_page_csp_would_refuse() {
        let html = r#"<!doctype html><html><head>
<script src="https://cdn.example/lib.js"></script>
<link rel="stylesheet" href="https://fonts.example/a.css">
<link rel="dns-prefetch" href="//tracker.example">
<style>body { background: url("https://tiles.example/1.png") } .ok { background: url(data:image/png;base64,AAAA) } svg { fill: url(#g) }</style>
</head><body>
<img src="https://tile.openstreetmap.org/1/1/1.png"><img src="data:image/png;base64,AAAA">
<iframe src="/api/status"></iframe>
<div style="background-image: url('/api/x')"></div>
<script>const s = "<img src='https://in.a.string/'>";</script>
<a href="https://example.com/cfp">a link is not a load</a>
</body></html>"#;
        let findings = csp_findings(html);
        let joined = findings.join("\n");
        assert!(joined.contains("line 2: <script src=\"https://cdn.example/lib.js\">"), "{joined}");
        assert!(joined.contains("rel=\"stylesheet\""), "{joined}");
        assert!(joined.contains("dns-prefetch"), "{joined}");
        assert!(joined.contains("url(https://tiles.example/1.png) in <style>"), "{joined}");
        assert!(joined.contains("tile.openstreetmap.org"), "{joined}");
        assert!(joined.contains("<iframe>"), "{joined}");
        assert!(joined.contains("url(/api/x) in a style attribute"), "{joined}");
        assert!(!joined.contains("in.a.string"), "a string inside a script is not a tag: {joined}");
        assert!(!joined.contains("example.com/cfp"), "{joined}");
        assert!(!joined.contains("base64"), "embedded content is fine: {joined}");
        assert_eq!(findings.len(), 7, "{joined}");
    }

    #[test]
    fn an_embedded_self_contained_page_passes_the_preflight() {
        let html = include_str!("../../../examples/pages/conference-radar.html");
        assert_eq!(csp_findings(html), Vec::<String>::new());
    }

    #[test]
    fn webrtc_is_refused_because_no_csp_governs_it() {
        let findings = csp_findings("<script>new RTCPeerConnection({iceServers: []})</script>");
        assert!(findings[0].contains("RTCPeerConnection"), "{findings:?}");
    }

    #[tokio::test]
    async fn versions_are_content_addressed_kept_to_n_and_rolled_back_across_a_reopen() {
        let root = temp_dir("store");
        let store = PageStore::new(&root);
        // A pause between publishes keeps "newest" a matter of time, not of a tie.
        let pause = || tokio::time::sleep(std::time::Duration::from_millis(5));
        let first = store.publish(new_version("<p>one</p>", None)).await.unwrap();
        assert_eq!(first.sha256, version_address(b"<p>one</p>", None));
        let dir = root.join("pages").join("radar%2Fteam").join("conference-radar");
        assert!(dir.join(&first.sha256).join("page.html").is_file(), "under .factory/pages/<scope>/<name>/<sha>/");

        pause().await;
        let data = serde_json::json!({"events": [1, 2]});
        let second = store.publish(new_version("<p>one</p>", Some(data.clone()))).await.unwrap();
        assert_ne!(second.sha256, first.sha256, "new data is a new version");
        assert_eq!(second.data_bytes, Some(serde_json::to_vec(&data).unwrap().len() as u64));
        pause().await;
        let third = store.publish(new_version("<p>three</p>", None)).await.unwrap();

        let (current, versions) = store.versions("radar/team", "conference-radar").await.unwrap();
        assert_eq!(current.unwrap().sha256, third.sha256);
        assert_eq!(versions.iter().map(|v| v.sha256.clone()).collect::<Vec<_>>(), [third.sha256.clone(), second.sha256.clone(), first.sha256.clone()]);

        // Roll back by a prefix, then reopen the store the way a restarted
        // daemon would: the rollback is a file, so it holds.
        store.set_current("radar/team", "conference-radar", &second.sha256[..7]).await.unwrap();
        let reopened = PageStore::new(&root);
        let (current, _) = reopened.versions("radar/team", "conference-radar").await.unwrap();
        assert_eq!(current.unwrap().sha256, second.sha256);
        let content = reopened.content("radar/team", "conference-radar", None, true, true).await.unwrap();
        assert_eq!(content.html.as_deref(), Some("<p>one</p>"));
        assert_eq!(content.data, Some(data));

        // A fourth publish is current again and pushes the oldest past keep: 3.
        pause().await;
        let fourth = store.publish(new_version("<p>four</p>", None)).await.unwrap();
        let (current, versions) = store.versions("radar/team", "conference-radar").await.unwrap();
        assert_eq!(current.unwrap().sha256, fourth.sha256);
        let kept: Vec<_> = versions.iter().map(|v| v.sha256.clone()).collect();
        assert_eq!(kept, [fourth.sha256.clone(), third.sha256.clone(), second.sha256.clone()]);
        assert!(!dir.join(&first.sha256).exists(), "pruned from disk");
        assert!(store.content("radar/team", "conference-radar", Some(&first.sha256), true, false).await.is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_same_content_again_is_the_same_version_made_current() {
        let root = temp_dir("same");
        let store = PageStore::new(&root);
        let a = store.publish(new_version("<p>a</p>", None)).await.unwrap();
        store.publish(new_version("<p>b</p>", None)).await.unwrap();
        let again = store.publish(new_version("<p>a</p>", None)).await.unwrap();
        assert_eq!(again.sha256, a.sha256);
        let (current, versions) = store.versions("radar/team", "conference-radar").await.unwrap();
        assert_eq!(current.unwrap().sha256, a.sha256);
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].sha256, a.sha256, "its record is the newest publish now");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_fact_lists_declared_pages_with_nothing_published_yet() {
        let root = temp_dir("fact");
        let store = PageStore::new(&root);
        store.publish(new_version("<p>x</p>", None)).await.unwrap();
        let tree = ScopeTree {
            scopes: vec![
                factory_kernel::ScopeNode { name: "radar".into(), path: "projects/radar".into() },
                factory_kernel::ScopeNode { name: "radar/team".into(), path: "projects/radar/team".into() },
                factory_kernel::ScopeNode { name: "other".into(), path: "projects/other".into() },
            ],
        };
        let decl = |name: &str| PageDecl { name: name.into(), title: None, max_bytes: None, keep: None };
        let provider = Provider::new(
            store.clone(),
            tree,
            vec![
                ("radar/team".into(), vec![decl("conference-radar"), decl("roadmap")]),
                ("other".into(), vec![decl("elsewhere")]),
            ],
        );
        let pages = provider.get(&ScopePagesQuery { scope: Some("radar".into()), name: None }).await.unwrap();
        let names: Vec<_> = pages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["conference-radar", "roadmap"], "the subtree, not the sibling");
        assert!(pages[0].current.is_some());
        assert!(pages[1].current.is_none(), "declared, not published yet");
        assert_eq!(pages[1].title, "roadmap");
        let one = provider
            .get(&ScopePagesQuery { scope: None, name: Some("elsewhere".into()) })
            .await
            .unwrap();
        assert_eq!(one.len(), 1);
        let unknown = provider.get(&ScopePagesQuery { scope: Some("nope".into()), name: None }).await;
        assert!(matches!(unknown, Err(FactoryError::NoSuchScope(_))));
        let _ = std::fs::remove_dir_all(&root);
    }
}
