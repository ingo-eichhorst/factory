//! Taking a snapshot, proving one would restore, and atomically materializing
//! a proved snapshot as a new root. Blocking file work and nothing else --
//! the engine moves every call here onto `spawn_blocking`, and decides
//! nothing about schedules, retention or warnings.
//!
//! **Taking one.** The database is copied with `VACUUM INTO` on a connection
//! of its own: one read transaction, so the copy is a consistent snapshot
//! however busy the daemon's other connections are, and the WAL is folded
//! in rather than left behind. It is never copied as a file. The copy is
//! checked with `integrity_check` before anything is archived. Every other
//! file is read once, hashed and archived from the same bytes, so the
//! manifest -- written last, after every file it describes -- can never
//! disagree with what is in the archive, even if somebody edits a page
//! mid-backup. The archive is written as a hidden `.partial` file in the
//! destination and renamed into place only once it is complete and synced:
//! a half-written archive is never listed, verified or kept.
//!
//! **Verifying one.** Unpacked into a fresh temporary directory, never the
//! instance: every entry's path is checked before it is written, every
//! checksum compared, the database copy opened read-only for
//! `integrity_check`, and each authored-content directory loaded with the
//! loader the daemon itself uses. The directory is removed on every path
//! out.

use chrono::{DateTime, Utc};
use factory_core::backup::{
    is_excluded, CheckStatus, DatabaseFacts, ExcludedFile, Group, GroupTotal, Manifest, ManifestFile,
    ManifestInstance, ScopeIncludeRow, ScopeIncludeTotal, VerifyCheck, AUTHORED, DATABASE_ENTRY, MANIFEST_FILE,
    MANIFEST_VERSION, OPTIONAL,
};
use factory_core::config::{CONFIG_FILE, FACTORY_DIR};
use factory_core::error::{FactoryError, Result};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

fn fail(message: impl Into<String>) -> FactoryError {
    FactoryError::adapter("backup", message)
}

// ============================================================ encryption
//
// `#152`: a snapshot is optionally encrypted to one native X25519 recipient
// with the `age` crate's streaming API. Writing goes straight from the tar/
// zstd stream through an age stream encryptor into the same hidden
// `.partial` file `take` always wrote to -- no plaintext byte ever reaches
// the destination. Reading decrypts the same way, given an owner-supplied
// identity `verify`/`restore` never store past the call that used it.

/// An owner-supplied identity for decrypting one encrypted snapshot: read
/// once from a file the daemon does not otherwise control, kept only in
/// memory for the one call that needed it, and never logged, recorded or
/// echoed back -- see [`OwnerIdentity::read`]. Named apart from
/// `age::Identity` (the trait every identity type implements) to keep the
/// two apart at a glance.
pub struct OwnerIdentity {
    inner: age::x25519::Identity,
    recipient: String,
}

impl OwnerIdentity {
    /// Refuses a path inside the instance's own `.factory/` -- a key the
    /// daemon can read on its own is a standing key, which an owner-supplied
    /// identity is meant never to be (`#116`) -- then reads the file and
    /// accepts exactly one native `AGE-SECRET-KEY-1…` line. A passphrase-
    /// protected identity file (binary, or otherwise not plain identity
    /// lines), a plugin identity (`AGE-PLUGIN-…`), an SSH identity, or more
    /// than one identity are all refused -- every error names the identity's
    /// path and, at most, a line number, never the line's own text.
    pub fn read(path: &Path, instance_root: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(fail(format!("the identity path {} must be absolute", path.display())));
        }
        let factory_dir = instance_root.join(FACTORY_DIR);
        let canonical_factory_dir = factory_dir.canonicalize().unwrap_or(factory_dir);
        let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if canonical_path.starts_with(&canonical_factory_dir) {
            return Err(fail(format!(
                "the identity path {} is inside this instance's own {}; a key the daemon can read on its own is a \
                 standing key, which an owner-supplied identity must never be",
                path.display(),
                canonical_factory_dir.display()
            )));
        }
        let text = fs::read_to_string(path)
            .map_err(|e| fail(format!("reading the identity file {}: {e}", path.display())))?;
        let mut found: Option<age::x25519::Identity> = None;
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parsed = line.parse::<age::x25519::Identity>().map_err(|_| {
                fail(format!(
                    "the identity file {}'s line {} is not a native age identity (AGE-SECRET-KEY-1…); \
                     passphrase-protected, plugin and ssh identities are not supported",
                    path.display(),
                    n + 1
                ))
            })?;
            if found.is_some() {
                return Err(fail(format!(
                    "the identity file {} holds more than one identity; only a single native age identity is \
                     supported",
                    path.display()
                )));
            }
            found = Some(parsed);
        }
        let inner = found.ok_or_else(|| fail(format!("the identity file {} holds no identity", path.display())))?;
        let recipient = inner.to_public().to_string();
        Ok(Self { inner, recipient })
    }

    /// The identity's public recipient (`age1…`), for a check's detail --
    /// never the identity itself.
    pub fn recipient(&self) -> &str {
        &self.recipient
    }
}

/// The innermost writer for a snapshot archive: plain, or wrapped through an
/// age stream encryptor when `Plan::encrypt_to` is set. `finish` -- never
/// `Write::flush` -- is what actually closes the age container's last chunk;
/// skipping it would truncate a file that decrypts into one that does not.
enum Sink<W: Write> {
    Plain(W),
    Encrypted(age::stream::StreamWriter<W>),
}

impl<W: Write> Write for Sink<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Sink::Plain(w) => w.write(buf),
            Sink::Encrypted(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Sink::Plain(w) => w.flush(),
            Sink::Encrypted(w) => w.flush(),
        }
    }
}

impl<W: Write> Sink<W> {
    fn finish(self) -> Result<W> {
        match self {
            Sink::Plain(w) => Ok(w),
            Sink::Encrypted(w) => w.finish().map_err(|e| fail(format!("finishing the age stream: {e}"))),
        }
    }
}

/// What to put in a snapshot.
pub struct Plan {
    pub root: PathBuf,
    pub database: PathBuf,
    pub instance: ManifestInstance,
    /// Every registered scope's own config, relative to the root, `/`-
    /// separated. The root's own `.factory/config.yaml` is always taken.
    pub scope_configs: Vec<String>,
    pub include_logs: bool,
    /// `infrastructure.backup.encrypt_to` (`#152`), already load-time
    /// validated -- `take` still re-parses it, cheaply, rather than trust a
    /// config that could in principle have changed underneath it.
    pub encrypt_to: Option<String>,
    /// `#279`: every scope's own declared `backup.include`, raw -- `(scope
    /// name, the scope's own directory relative to the root, its raw
    /// `backup.include` strings)`, exactly `Config::backup_includes`'s own
    /// shape. Resolved and probed fresh inside `take`, never trusted from a
    /// config that could in principle have changed underneath it, the same
    /// reasoning `encrypt_to` above already follows.
    pub scope_includes: Vec<(String, PathBuf, Vec<String>)>,
}

/// A snapshot as written.
#[derive(Debug)]
pub struct Taken {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub files: u64,
    pub database_bytes: u64,
    pub groups: Vec<GroupTotal>,
    /// `#279`: every declared scope include this snapshot held, archived
    /// or not.
    pub scope_includes: Vec<ScopeIncludeTotal>,
}

/// A directory removed when this goes out of scope, however that happens.
struct Scratch(PathBuf);

impl Scratch {
    fn new(what: &str) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("factory-{what}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).map_err(|e| fail(format!("making a temporary directory {}: {e}", dir.display())))?;
        Ok(Self(dir))
    }

    /// A fresh directory on the destination's own filesystem, so committing
    /// a restore is one rename rather than a cross-device copy.
    fn sibling(target: &Path) -> Result<Self> {
        let parent = target
            .parent()
            .ok_or_else(|| fail(format!("{} has no parent directory", target.display())))?;
        let dir = parent.join(format!(".factory-restore-{}.partial", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).map_err(|e| fail(format!("making restore staging directory {}: {e}", dir.display())))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Make sure `destination` can take an archive, and answer it canonical.
///
/// Only the last component is ever created: a destination whose parent is
/// missing is most likely a disk that is not mounted, and creating
/// `/Volumes/Backup/factory` on the system disk then would be the one
/// backup that looks fine and is not. A destination inside the instance's
/// own `.factory/` is refused -- the backup would be inside what it backs
/// up.
pub fn prepare_destination(root: &Path, destination: &Path) -> Result<PathBuf> {
    if !destination.exists() {
        let parent = destination.parent().unwrap_or(destination);
        if !parent.is_dir() {
            return Err(fail(format!(
                "the destination {} does not exist, and neither does {} -- is the disk mounted?",
                destination.display(),
                parent.display()
            )));
        }
        fs::create_dir(destination)
            .map_err(|e| fail(format!("creating the destination {}: {e}", destination.display())))?;
    }
    if !destination.is_dir() {
        return Err(fail(format!("the destination {} is not a directory", destination.display())));
    }
    let canonical = destination
        .canonicalize()
        .map_err(|e| fail(format!("resolving the destination {}: {e}", destination.display())))?;
    let factory_dir = root.join(FACTORY_DIR).canonicalize().unwrap_or_else(|_| root.join(FACTORY_DIR));
    if canonical.starts_with(&factory_dir) {
        return Err(fail(format!(
            "the destination {} is inside the instance's own {} -- a backup there is inside what it backs up",
            canonical.display(),
            factory_dir.display()
        )));
    }
    Ok(canonical)
}

/// Take a snapshot of `plan` into `destination` (already prepared), named
/// `name`.
pub fn take(plan: &Plan, destination: &Path, name: &str, at: DateTime<Utc>) -> Result<Taken> {
    let final_path = destination.join(name);
    if final_path.exists() {
        return Err(fail(format!("{} already exists", final_path.display())));
    }
    let scratch = Scratch::new("backup")?;
    let recipient = match &plan.encrypt_to {
        Some(s) => Some(super::parse_recipient(s)?),
        None => None,
    };

    // -- the database, consistent and checked -----------------------------
    if !plan.database.is_file() {
        return Err(fail(format!("there is no database at {}", plan.database.display())));
    }
    let copy = scratch.0.join("factory.sqlite");
    {
        let conn = Connection::open(&plan.database)
            .map_err(|e| fail(format!("opening {}: {e}", plan.database.display())))?;
        conn.busy_timeout(std::time::Duration::from_secs(30))
            .map_err(|e| fail(e.to_string()))?;
        let target = copy.to_str().ok_or_else(|| fail("the temporary path is not UTF-8"))?;
        conn.execute("VACUUM INTO ?1", [target])
            .map_err(|e| fail(format!("copying the database with VACUUM INTO: {e}")))?;
    }
    let database = database_facts(&copy)?;
    if database.integrity != "ok" {
        return Err(fail(format!(
            "the database copy failed integrity_check: {} -- no archive was written",
            database.integrity
        )));
    }

    // -- what goes in, decided before anything is read --------------------
    //
    // `seen` tracks every relative path claimed so far, across the config
    // entries, every authored directory and every `#279` scope declaration
    // below, in that order -- so an overlap anywhere (two declarations
    // covering the same file, the same declaration written twice, or a
    // declared directory that happens to contain a nested scope's own
    // `.factory/config.yaml`, already claimed above as `Group::Config`)
    // archives and lists that file exactly once, under whichever claim
    // came first. See `walk`'s own doc comment.
    let mut excluded = Vec::new();
    let mut entries: Vec<(String, PathBuf, Group)> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let root_config = format!("{FACTORY_DIR}/{CONFIG_FILE}");
    seen.insert(root_config.clone());
    entries.push((root_config.clone(), plan.root.join(&root_config), Group::Config));
    for scope in &plan.scope_configs {
        if seen.insert(scope.clone()) {
            entries.push((scope.clone(), plan.root.join(scope), Group::Config));
        }
    }
    let optional = if plan.include_logs { &OPTIONAL[..] } else { &[] };
    for (group, dir, _) in AUTHORED.iter().chain(optional) {
        walk(&plan.root, dir.trim_end_matches('/'), *group, &mut entries, &mut excluded, &mut seen);
    }

    // `#279`: every scope's own declared `backup.include`, resolved and
    // probed fresh -- never trusted from a config that could in principle
    // have changed underneath this call, the same reasoning `encrypt_to`
    // above already follows. A refused or still-missing declaration is
    // recorded in `excluded` exactly like a file `is_excluded` or a
    // symlink `walk` itself finds, and seeds this snapshot's own
    // `scope_includes` with zero files -- never a reason to fail the
    // backup, and never a reason to skip any other declared directory.
    let mut scope_include_totals: Vec<ScopeIncludeTotal> = Vec::new();
    for row in factory_infrastructure::backup_facts::resolve_scope_includes(&plan.root, &plan.scope_includes) {
        let ScopeIncludeRow { scope, declared, path, unavailable, .. } = row;
        match (path, unavailable) {
            (relative, Some(reason)) => {
                let shown = relative.clone().unwrap_or_else(|| declared.clone());
                excluded.push(ExcludedFile { path: format!("{scope}: {shown}"), reason: reason.clone() });
                scope_include_totals.push(ScopeIncludeTotal {
                    scope,
                    declared,
                    path: relative,
                    unavailable: Some(reason),
                    files: 0,
                    bytes: 0,
                });
            }
            (Some(relative), None) => {
                walk(&plan.root, &relative, Group::ScopeData, &mut entries, &mut excluded, &mut seen);
                // Files and bytes are filled in below, once every entry has
                // actually been read and hashed -- never guessed from the
                // walk alone, which only lists what is there, not what
                // `is_excluded`, a symlink or an earlier claim (this
                // declaration's own `seen` check above) leaves out of it.
                // A file a different declaration already claimed still
                // counts here: the prefix match below counts what is
                // archived under this path, never what this call's own
                // walk happened to add.
                scope_include_totals.push(ScopeIncludeTotal {
                    scope,
                    declared,
                    path: Some(relative),
                    unavailable: None,
                    files: 0,
                    bytes: 0,
                });
            }
            (None, None) => unreachable!("resolve_scope_includes always names a path when it is not refused"),
        }
    }

    // -- the archive -------------------------------------------------------
    let partial = destination.join(format!(".{name}.partial"));
    let written = (|| -> Result<(u64, Vec<ManifestFile>)> {
        let file = File::create(&partial).map_err(|e| fail(format!("creating {}: {e}", partial.display())))?;
        let buffered = BufWriter::new(file);
        let sink: Sink<BufWriter<File>> = match &recipient {
            Some(recipient) => {
                let encryptor = age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
                    .map_err(|e| fail(format!("preparing the age recipient: {e}")))?;
                let writer = encryptor
                    .wrap_output(buffered)
                    .map_err(|e| fail(format!("writing the age header: {e}")))?;
                Sink::Encrypted(writer)
            }
            None => Sink::Plain(buffered),
        };
        let encoder = zstd::Encoder::new(sink, 3).map_err(|e| fail(format!("zstd: {e}")))?;
        let mut tar = tar::Builder::new(encoder);
        let mut files = Vec::new();
        let mtime = at.timestamp().max(0) as u64;

        let database_bytes = fs::metadata(&copy).map(|m| m.len()).map_err(|e| fail(e.to_string()))?;
        let sha256 = hash_file(&copy)?;
        let mut header = entry_header(database_bytes, mtime);
        let reader = File::open(&copy).map_err(|e| fail(e.to_string()))?;
        tar.append_data(&mut header, DATABASE_ENTRY, reader)
            .map_err(|e| fail(format!("archiving the database: {e}")))?;
        files.push(ManifestFile { path: DATABASE_ENTRY.into(), size: database_bytes, sha256, group: Group::Database });

        for (relative, absolute, group) in &entries {
            // Read once; hash and archive the same bytes. A file that went
            // away since the walk is not an error -- it is simply not in
            // this snapshot.
            let bytes = match fs::read(absolute) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(fail(format!("reading {relative}: {e}"))),
            };
            let modified = fs::metadata(absolute)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(mtime);
            let mut header = entry_header(bytes.len() as u64, modified);
            tar.append_data(&mut header, relative, bytes.as_slice())
                .map_err(|e| fail(format!("archiving {relative}: {e}")))?;
            files.push(ManifestFile {
                path: relative.clone(),
                size: bytes.len() as u64,
                sha256: hex(&Sha256::digest(&bytes)),
                group: *group,
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));

        let manifest = Manifest {
            version: MANIFEST_VERSION,
            instance: plan.instance.clone(),
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: at,
            database: database.clone(),
            files: files.clone(),
            excluded: std::mem::take(&mut excluded),
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| fail(e.to_string()))?;
        let mut header = entry_header(json.len() as u64, mtime);
        tar.append_data(&mut header, MANIFEST_FILE, json.as_slice())
            .map_err(|e| fail(format!("archiving the manifest: {e}")))?;

        let encoder = tar.into_inner().map_err(|e| fail(format!("finishing the archive: {e}")))?;
        let sink = encoder.finish().map_err(|e| fail(format!("finishing zstd: {e}")))?;
        let buffered = sink.finish()?;
        let file = buffered.into_inner().map_err(|e| fail(format!("flushing the archive: {e}")))?;
        file.sync_all().map_err(|e| fail(format!("syncing the archive: {e}")))?;
        Ok((database_bytes, files))
    })();
    let (database_bytes, files) = match written {
        Ok(done) => done,
        Err(e) => {
            let _ = fs::remove_file(&partial);
            return Err(e);
        }
    };
    if let Err(e) = fs::rename(&partial, &final_path) {
        let _ = fs::remove_file(&partial);
        return Err(fail(format!("moving the archive into place: {e}")));
    }

    let mut groups: BTreeMap<Group, GroupTotal> = BTreeMap::new();
    for f in &files {
        let total = groups.entry(f.group).or_insert(GroupTotal { group: f.group, files: 0, bytes: 0 });
        total.files += 1;
        total.bytes += f.size;
    }
    // `#279`: now that every file has actually been read and hashed, count
    // each declared directory's own share of them -- never from the walk
    // alone, which lists what is there before `is_excluded` or a symlink
    // leaves anything out of it.
    for total in &mut scope_include_totals {
        let Some(relative) = &total.path else { continue };
        let prefix = format!("{relative}/");
        for f in &files {
            if f.path.starts_with(&prefix) {
                total.files += 1;
                total.bytes += f.size;
            }
        }
    }
    Ok(Taken {
        size_bytes: fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0),
        path: final_path,
        files: files.len() as u64,
        database_bytes,
        groups: groups.into_values().collect(),
        scope_includes: scope_include_totals,
    })
}

fn entry_header(size: u64, mtime: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_size(size);
    header.set_mode(0o644);
    header.set_mtime(mtime);
    header.set_entry_type(tar::EntryType::Regular);
    header
}

/// Every regular file under `<root>/<dir>`, sorted, as `(relative, absolute,
/// group)`. A symbolic link is never followed -- it could lead anywhere,
/// a secrets file included -- and an excluded name is never opened; both
/// are written down in the manifest instead.
///
/// `seen` is shared across every call this snapshot makes, in order, so a
/// path already claimed by an earlier one -- the root config, another
/// authored directory, or an overlapping `#279` scope declaration --
/// is silently left alone here rather than archived and listed a second
/// time: the earlier call's group wins, and a config file a scope's own
/// declared directory happens to contain stays `Group::Config`. This is
/// not a new exclusion (nothing goes in `excluded` for it): the file is in
/// the backup, just not claimed twice.
fn walk(root: &Path, dir: &str, group: Group, out: &mut Vec<(String, PathBuf, Group)>, excluded: &mut Vec<ExcludedFile>, seen: &mut BTreeSet<String>) {
    let Ok(read) = fs::read_dir(root.join(dir)) else { return };
    let mut children: Vec<_> = read.flatten().collect();
    children.sort_by_key(|e| e.file_name());
    for child in children {
        let name = child.file_name().to_string_lossy().into_owned();
        let relative = format!("{dir}/{name}");
        let Ok(meta) = fs::symlink_metadata(child.path()) else { continue };
        if let Some(reason) = is_excluded(&relative) {
            excluded.push(ExcludedFile { path: relative, reason: reason.into() });
        } else if meta.file_type().is_symlink() {
            excluded.push(ExcludedFile { path: relative, reason: "a symbolic link: not followed".into() });
        } else if meta.is_dir() {
            walk(root, &relative, group, out, excluded, seen);
        } else if meta.is_file() && seen.insert(relative.clone()) {
            out.push((relative, child.path(), group));
        }
    }
}

fn database_facts(path: &Path) -> Result<DatabaseFacts> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| fail(format!("opening the database copy: {e}")))?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| fail(format!("integrity_check: {e}")))?;
    let user_version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| fail(format!("user_version: {e}")))?;
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .map_err(|e| fail(e.to_string()))?;
    let tables = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
        .map_err(|e| fail(e.to_string()))?;
    Ok(DatabaseFacts { path: DATABASE_ENTRY.into(), user_version, tables, integrity })
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|e| fail(e.to_string()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| fail(e.to_string()))?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ================================================================ verify

/// Writes through to a file, hashing and counting on the way.
struct Hashing<W> {
    inner: W,
    hasher: Sha256,
    size: u64,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn check(name: &str, status: CheckStatus, detail: impl Into<String>) -> VerifyCheck {
    VerifyCheck { name: name.into(), status, detail: detail.into() }
}

/// A path from inside an archive, if it is safe to write under a directory:
/// relative, and made only of ordinary names.
fn safe_relative(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Every step of proving `archive` would restore, in the order they ran.
/// `instance_id` is this instance's own, which the manifest is compared to.
/// `identity` decrypts an encrypted archive (`#152`); `None` for a plaintext
/// one, verified and restored exactly as v1's were.
pub fn verify(archive: &Path, instance_id: &str, identity: Option<&OwnerIdentity>) -> Vec<VerifyCheck> {
    let scratch = match Scratch::new("verify") {
        Ok(s) => s,
        Err(e) => {
            return vec![check("archive", CheckStatus::Fail, e.to_string())];
        }
    };
    verify_into(archive, instance_id, &scratch.0, identity)
}

/// The verification shared by `verify` and restore. `tmp` must be an empty,
/// private directory: archive entries are materialized there while they are
/// hashed, then all loaders inspect those exact bytes.
fn verify_into(archive: &Path, instance_id: &str, tmp: &Path, identity: Option<&OwnerIdentity>) -> Vec<VerifyCheck> {
    let mut checks = Vec::new();

    let file = match File::open(archive) {
        Ok(f) => f,
        Err(e) => {
            checks.push(check("archive", CheckStatus::Fail, format!("opening {}: {e}", archive.display())));
            return checks;
        }
    };

    // -- decrypt, when an identity was supplied (`#152`) ---------------------
    //
    // A separate check from "archive" below: a wrong identity or a corrupt
    // ciphertext is a fact about decryption, not about the tar/zstd stream
    // underneath, which nothing here has looked at yet.
    let reader: Box<dyn Read> = match identity {
        Some(identity) => {
            let decrypted = age::Decryptor::new(file)
                .map_err(|e| e.to_string())
                .and_then(|decryptor| {
                    decryptor
                        .decrypt(std::iter::once(&identity.inner as &dyn age::Identity))
                        .map_err(|e| e.to_string())
                });
            match decrypted {
                Ok(reader) => {
                    checks.push(check(
                        "decrypt",
                        CheckStatus::Ok,
                        format!("decrypted with the identity for {}", identity.recipient()),
                    ));
                    Box::new(reader)
                }
                Err(e) => {
                    checks.push(check(
                        "decrypt",
                        CheckStatus::Fail,
                        format!("could not decrypt with the identity for {}: {e}", identity.recipient()),
                    ));
                    return checks;
                }
            }
        }
        None => Box::new(file),
    };

    // -- unpack, hashing every entry on its way to disk ---------------------
    let mut unpacked: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut manifest_text: Option<String> = None;
    let unpacking = (|| -> std::result::Result<(), String> {
        let decoder = zstd::Decoder::new(reader).map_err(|e| format!("zstd: {e}"))?;
        let mut tar = tar::Archive::new(decoder);
        for entry in tar.entries().map_err(|e| format!("reading the archive: {e}"))? {
            let mut entry = entry.map_err(|e| format!("reading the archive: {e}"))?;
            if entry.header().entry_type() != tar::EntryType::Regular {
                continue;
            }
            let raw = entry.path().map_err(|e| format!("an entry's path: {e}"))?.into_owned();
            let relative = safe_relative(&raw)
                .ok_or_else(|| format!("the archive holds an unsafe path {:?}; nothing was written outside the temporary directory", raw.display().to_string()))?;
            let key = relative.to_string_lossy().replace('\\', "/");
            if key == MANIFEST_FILE {
                let mut text = String::new();
                entry.read_to_string(&mut text).map_err(|e| format!("reading the manifest: {e}"))?;
                manifest_text = Some(text);
                continue;
            }
            let target = tmp.join(&relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("unpacking {key}: {e}"))?;
            }
            let out = File::create(&target).map_err(|e| format!("unpacking {key}: {e}"))?;
            let mut hashing = Hashing { inner: BufWriter::new(out), hasher: Sha256::new(), size: 0 };
            std::io::copy(&mut entry, &mut hashing).map_err(|e| format!("unpacking {key}: {e}"))?;
            hashing.flush().map_err(|e| format!("unpacking {key}: {e}"))?;
            unpacked.insert(key, (hashing.size, hex(&hashing.hasher.finalize())));
        }
        Ok(())
    })();
    match unpacking {
        Ok(()) => checks.push(check(
            "archive",
            CheckStatus::Ok,
            format!("unpacked {} files into a temporary directory", unpacked.len()),
        )),
        Err(e) => {
            checks.push(check("archive", CheckStatus::Fail, e));
            return checks;
        }
    }

    // -- the manifest --------------------------------------------------------
    let manifest: Manifest = match manifest_text.as_deref().map(serde_json::from_str::<Manifest>) {
        Some(Ok(m)) if m.version <= MANIFEST_VERSION => m,
        Some(Ok(m)) => {
            checks.push(check(
                "manifest",
                CheckStatus::Fail,
                format!("manifest version {} is newer than this daemon reads ({MANIFEST_VERSION})", m.version),
            ));
            return checks;
        }
        Some(Err(e)) => {
            checks.push(check("manifest", CheckStatus::Fail, format!("{MANIFEST_FILE} does not parse: {e}")));
            return checks;
        }
        None => {
            checks.push(check("manifest", CheckStatus::Fail, format!("the archive has no {MANIFEST_FILE}")));
            return checks;
        }
    };
    if manifest.instance.id == instance_id {
        checks.push(check(
            "manifest",
            CheckStatus::Ok,
            format!(
                "{} files listed, taken {} by factory {}",
                manifest.files.len(),
                manifest.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
                manifest.daemon_version
            ),
        ));
    } else {
        checks.push(check(
            "manifest",
            CheckStatus::Warn,
            format!("taken by another instance ({} {}), not this one", manifest.instance.name, manifest.instance.id),
        ));
    }

    // -- every checksum ------------------------------------------------------
    let mut problems = Vec::new();
    for f in &manifest.files {
        match unpacked.get(&f.path) {
            None => problems.push(format!("{} is missing", f.path)),
            Some((size, _)) if *size != f.size => problems.push(format!("{} is {size} bytes, not {}", f.path, f.size)),
            Some((_, sha)) if *sha != f.sha256 => problems.push(format!("{} does not match its sha256", f.path)),
            Some(_) => {}
        }
    }
    for path in unpacked.keys() {
        if !manifest.files.iter().any(|f| &f.path == path) {
            problems.push(format!("{path} is not in the manifest"));
        }
    }
    if problems.is_empty() {
        checks.push(check("checksums", CheckStatus::Ok, format!("all {} files match their sha256", manifest.files.len())));
    } else {
        checks.push(check("checksums", CheckStatus::Fail, summarize(&problems)));
    }

    // -- the database ----------------------------------------------------------
    checks.push(verify_database(&tmp.join(DATABASE_ENTRY), &manifest));

    // -- the configs -----------------------------------------------------------
    let configs: Vec<&ManifestFile> = manifest.files.iter().filter(|f| f.group == Group::Config).collect();
    let mut bad = Vec::new();
    match factory_core::config::Factory::load(tmp) {
        Ok(mut factory) => {
            if let Err(e) = crate::discovery::apply(&mut factory) {
                bad.push(format!("scope discovery: {e}"));
            } else if let Err(e) = factory.config.validate() {
                bad.push(format!("instance config: {e}"));
            } else if let Err(e) = super::validate_config(&factory) {
                bad.push(format!("backup config: {e}"));
            }
        }
        Err(e) => bad.push(format!("the root config: {e}")),
    }
    for f in &configs {
        let parsed = fs::read_to_string(tmp.join(&f.path))
            .map_err(|e| e.to_string())
            .and_then(|t| serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&t).map_err(|e| e.to_string()));
        if let Err(e) = parsed {
            bad.push(format!("{}: {e}", f.path));
        }
    }
    checks.push(if bad.is_empty() {
        check(
            "config",
            CheckStatus::Ok,
            format!(
                "the root config loads; {} config files parse; scope discovery and validation pass",
                configs.len()
            ),
        )
    } else {
        check("config", CheckStatus::Fail, summarize(&bad))
    });

    // -- every authored-content loader ------------------------------------------
    checks.extend(verify_loaders(tmp));
    checks
}

/// A staged snapshot that was made visible as a new instance root.
#[derive(Debug)]
pub struct Restored {
    pub into: PathBuf,
    pub files: u64,
    pub checks: Vec<VerifyCheck>,
}

/// Validate, stage and atomically commit a snapshot as a new root -- plain,
/// or encrypted with `identity` supplied (`#152`). Nothing is written under
/// `into` until all of `verify`'s required checks pass. The staging
/// directory is a sibling so the last operation is a rename on one
/// filesystem; its guard removes every failed attempt.
pub fn restore(
    archive: &Path,
    instance_id: &str,
    active_root: &Path,
    into: &Path,
    identity: Option<&OwnerIdentity>,
) -> Result<Restored> {
    let (target, existed) = restore_target(active_root, into)?;
    let stage = Scratch::sibling(&target)?;
    let checks = verify_into(archive, instance_id, &stage.0, identity);
    let failures: Vec<String> = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .map(|c| format!("{}: {}", c.name, c.detail))
        .collect();
    if !failures.is_empty() {
        return Err(fail(format!(
            "verification failed; no restored root was committed: {}",
            summarize(&failures)
        )));
    }

    // Re-check after the potentially long verification. If somebody created
    // or populated the destination meanwhile, preserving it wins over the
    // restore; the staging guard removes our private copy.
    let (commit_target, exists_now) = restore_target(active_root, &target)?;
    if commit_target != target || exists_now != existed {
        return Err(fail(format!(
            "the restore destination {} changed while the snapshot was being verified; nothing was committed",
            target.display()
        )));
    }

    let files = regular_file_count(&stage.0)?;
    if existed {
        fs::remove_dir(&target)
            .map_err(|e| fail(format!("preparing the empty restore destination {}: {e}", target.display())))?;
    }
    if let Err(e) = fs::rename(&stage.0, &target) {
        if existed {
            let _ = fs::create_dir(&target);
        }
        return Err(fail(format!("committing the restored root at {}: {e}", target.display())));
    }
    let into = target.canonicalize().unwrap_or(target);
    Ok(Restored { into, files, checks })
}

/// The canonical destination and whether an empty directory already occupies
/// it. Canonicalizing the parent also catches aliases of the active root even
/// when the last component does not exist yet.
fn restore_target(active_root: &Path, into: &Path) -> Result<(PathBuf, bool)> {
    if !into.is_absolute() {
        return Err(fail(format!(
            "the restore destination {} must be an absolute path",
            into.display()
        )));
    }
    let active = active_root
        .canonicalize()
        .map_err(|e| fail(format!("resolving the running instance root {}: {e}", active_root.display())))?;
    let existing = match fs::symlink_metadata(into) {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(fail(format!(
                "reading the restore destination {}: {e}",
                into.display()
            )))
        }
    };
    let exists = existing.is_some();
    let target = if let Some(meta) = existing {
        if meta.file_type().is_symlink() {
            return Err(fail(format!(
                "the restore destination {} is a symbolic link; name a new or empty directory directly",
                into.display()
            )));
        }
        if !meta.is_dir() {
            return Err(fail(format!("the restore destination {} is not a directory", into.display())));
        }
        let resolved = into
            .canonicalize()
            .map_err(|e| fail(format!("resolving the restore destination {}: {e}", into.display())))?;
        if resolved == active {
            return Err(fail(format!(
                "{} is the running instance root; restore only into a different new root",
                resolved.display()
            )));
        }
        let mut entries = fs::read_dir(into)
            .map_err(|e| fail(format!("reading the restore destination {}: {e}", into.display())))?;
        if entries.next().is_some() {
            return Err(fail(format!(
                "the restore destination {} is not empty; restore only into a new or empty directory",
                into.display()
            )));
        }
        resolved
    } else {
        let parent = into
            .parent()
            .ok_or_else(|| fail(format!("the restore destination {} has no parent", into.display())))?;
        let name = into
            .file_name()
            .ok_or_else(|| fail(format!("the restore destination {} is not a new root path", into.display())))?;
        let parent = parent.canonicalize().map_err(|e| {
            fail(format!(
                "the restore destination's parent {} does not exist or cannot be resolved: {e}",
                parent.display()
            ))
        })?;
        if !parent.is_dir() {
            return Err(fail(format!("the restore destination's parent {} is not a directory", parent.display())));
        }
        parent.join(name)
    };
    if target == active {
        return Err(fail(format!(
            "{} is the running instance root; restore only into a different new root",
            target.display()
        )));
    }
    Ok((target, exists))
}

fn regular_file_count(root: &Path) -> Result<u64> {
    let mut count = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| fail(format!("reading restored files under {}: {e}", dir.display())))?;
        for entry in entries {
            let entry = entry.map_err(|e| fail(format!("reading restored files under {}: {e}", dir.display())))?;
            let ty = entry.file_type().map_err(|e| fail(format!("reading {}: {e}", entry.path().display())))?;
            if ty.is_dir() {
                pending.push(entry.path());
            } else if ty.is_file() {
                count += 1;
            }
        }
    }
    Ok(count)
}

fn verify_database(path: &Path, manifest: &Manifest) -> VerifyCheck {
    if !path.is_file() {
        return check("database", CheckStatus::Fail, format!("the archive has no {DATABASE_ENTRY}"));
    }
    let facts = match database_facts(path) {
        Ok(f) => f,
        Err(e) => return check("database", CheckStatus::Fail, e.to_string()),
    };
    if facts.integrity != "ok" {
        return check("database", CheckStatus::Fail, format!("integrity_check: {}", facts.integrity));
    }
    let counts = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .ok()
        .map(|conn| {
            ["tasks", "runs", "task_entries"]
                .iter()
                .filter(|t| facts.tables.iter().any(|name| name == *t))
                .filter_map(|t| {
                    conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get::<_, i64>(0))
                        .ok()
                        .map(|n| format!("{n} {}", t.replace('_', " ")))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let schema = if facts.user_version == manifest.database.user_version {
        format!("schema {}", facts.user_version)
    } else {
        return check(
            "database",
            CheckStatus::Fail,
            format!("schema {} where the manifest says {}", facts.user_version, manifest.database.user_version),
        );
    };
    if facts.user_version != factory_plugins::SQLITE_SCHEMA_VERSION {
        return check(
            "database",
            CheckStatus::Fail,
            format!(
                "schema {} is incompatible with this daemon (expects {}); restoring it would discard task data on startup",
                facts.user_version,
                factory_plugins::SQLITE_SCHEMA_VERSION
            ),
        );
    }
    check(
        "database",
        CheckStatus::Ok,
        format!("integrity_check ok; {schema}; {} tables{}", facts.tables.len(), if counts.is_empty() { String::new() } else { format!("; {counts}") }),
    )
}

/// `1 page`, `2 pages`.
fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// The first few problems, and how many more there were.
fn summarize(problems: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut text = problems.iter().take(SHOWN).cloned().collect::<Vec<_>>().join("; ");
    if problems.len() > SHOWN {
        text.push_str(&format!("; and {} more", problems.len() - SHOWN));
    }
    text
}

/// A loader's answer: `Warn` naming the files it could not parse, `Ok`
/// with what it loaded otherwise. A parse failure is most likely in the
/// live instance too -- the checksums above already proved the bytes are
/// the ones that were backed up -- so it never fails the verification.
fn loaded(name: &str, what: String, unparsed: Vec<String>) -> VerifyCheck {
    if unparsed.is_empty() {
        check(name, CheckStatus::Ok, what)
    } else {
        check(name, CheckStatus::Warn, format!("{what}; could not parse {}", summarize(&unparsed)))
    }
}

fn verify_loaders(root: &Path) -> Vec<VerifyCheck> {
    use factory_core::dependencies::{validate_document, AttachmentKind};
    use factory_core::{goals, knowledge, policy, quality, ready, scenario};
    let mut out = Vec::new();

    let (catalogues, findings) = policy::load_all(&policy::policies_dir(root));
    let (drafts, draft_findings) = scenario::load_drafts(&scenario::drafts_dir(root));
    let unparsed = findings
        .iter()
        .chain(&draft_findings)
        .filter(|f| f.kind == policy::FindingKind::ParseFailed)
        .map(|f| f.subject.clone())
        .collect();
    let controls: usize = catalogues.iter().map(|c| c.controls.len()).sum();
    out.push(loaded(
        "policies",
        format!("{}, {}, {}", count(catalogues.len(), "catalogue"), count(controls, "control"), count(drafts.len(), "draft")),
        unparsed,
    ));

    let catalogue = goals::load(&goals::goals_dir(root));
    let unparsed = catalogue
        .findings
        .iter()
        .filter(|f| f.kind == goals::FindingKind::ParseFailed)
        .map(|f| f.subject.clone())
        .collect();
    out.push(loaded(
        "goals",
        format!(
            "{} direction, {}",
            if catalogue.direction.is_some() { "a" } else { "no" },
            count(catalogue.cycles.len(), "cycle")
        ),
        unparsed,
    ));

    let (scenarios, findings) = scenario::load(&scenario::scenarios_dir(root));
    let unparsed = findings
        .iter()
        .filter(|f| f.kind == scenario::FindingKind::ParseFailed)
        .map(|f| f.subject.clone())
        .collect();
    out.push(loaded("scenarios", count(scenarios.len(), "scenario"), unparsed));

    let profiles = quality::load(&quality::quality_dir(root));
    let unparsed = profiles
        .findings
        .iter()
        .filter(|f| f.kind == quality::FindingKind::ParseFailed)
        .map(|f| f.subject.clone())
        .collect();
    out.push(loaded("quality", count(profiles.profiles.len(), "profile"), unparsed));

    let readiness = ready::load(&ready::ready_dir(root));
    let unparsed = readiness
        .findings
        .iter()
        .filter(|f| f.kind == ready::FindingKind::ParseFailed)
        .map(|f| f.subject.clone())
        .collect();
    out.push(loaded("intake", count(readiness.files.len(), "definition"), unparsed));

    match factory_core::budget::load(root) {
        Ok(catalogue) => out.push(loaded("budgets", count(catalogue.scopes.len(), "monthly limit"), Vec::new())),
        Err(error) => out.push(loaded("budgets", "authored budget catalogue".into(), vec![error])),
    }

    let vex_dir = root.join(FACTORY_DIR).join("vex");
    let mut vex_paths = Vec::new();
    let mut dirs = vec![vex_dir.clone()];
    while let Some(dir) = dirs.pop() {
        let Ok(read) = fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".cdx.json"))
            {
                vex_paths.push(path);
            }
        }
    }
    vex_paths.sort();
    let mut unparsed = Vec::new();
    for path in &vex_paths {
        let result = fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| validate_document(&bytes, AttachmentKind::Vulnerabilities));
        if let Err(error) = result {
            let relative = path.strip_prefix(&vex_dir).unwrap_or(path);
            unparsed.push(format!("{} ({error})", relative.display()));
        }
    }
    out.push(loaded("vex", count(vex_paths.len(), "document"), unparsed));

    let dir = root.join(FACTORY_DIR).join("datasets");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .map(|read| {
            read.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut cases = 0;
    let mut datasets = 0;
    let mut unparsed = Vec::new();
    for name in &names {
        match factory_core::dataset::Dataset::load(&dir, name) {
            Ok(Some(d)) => {
                datasets += 1;
                cases += d.cases.len();
            }
            Ok(None) => {}
            Err(e) => unparsed.push(format!("{name}.yaml ({e})")),
        }
    }
    out.push(loaded("datasets", format!("{}, {}", count(datasets, "dataset"), count(cases, "case")), unparsed));

    let index = knowledge::index(root);
    let truncated: Vec<String> = index
        .findings
        .iter()
        .filter(|f| f.kind == knowledge::FindingKind::Truncated)
        .map(|f| if f.note.is_empty() { f.detail.clone() } else { f.note.clone() })
        .collect();
    out.push(loaded(
        "knowledge",
        if index.present {
            format!("{}, {}, {}", count(index.pages.len(), "page"), count(index.documents.len(), "document"), count(index.tags.len(), "tag"))
        } else {
            "no knowledge vault in this snapshot".into()
        },
        truncated,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::backup::archive_name;

    /// A throwaway instance on disk: a WAL database with a row in it,
    /// authored content, a secret and a symlink that must stay out.
    struct Instance {
        root: PathBuf,
        destination: PathBuf,
    }

    impl Instance {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!("factory-backup-test-{tag}-{}", uuid::Uuid::new_v4()));
            let root = base.join("instance");
            let destination = base.join("destination");
            let f = root.join(".factory");
            fs::create_dir_all(f.join("knowledge/company")).unwrap();
            fs::create_dir_all(f.join("knowledge/data/secrets")).unwrap();
            fs::create_dir_all(f.join("policies")).unwrap();
            fs::create_dir_all(f.join("goals")).unwrap();
            fs::create_dir_all(f.join("vex/demo")).unwrap();
            fs::create_dir_all(f.join("intake")).unwrap();
            fs::create_dir_all(f.join("budgets")).unwrap();
            fs::create_dir_all(f.join("logs")).unwrap();
            fs::create_dir_all(root.join("projects/demo/.factory")).unwrap();
            fs::write(f.join("config.yaml"), "version: 1\ninstance:\n  id: inst-1\n  name: Test Instance\n").unwrap();
            fs::write(
                root.join("projects/demo/.factory/config.yaml"),
                "version: 1\nscope:\n  id: demo-id\n  name: demo\n",
            )
            .unwrap();
            fs::write(f.join("intake/ready.yaml"), "max_complexity: 8\n").unwrap();
            fs::write(f.join("budgets/limits.yaml"), "version: 1\nscopes: {demo-id: {monthly_usd: 50}}\n").unwrap();
            fs::write(f.join("knowledge/company/README.md"), "---\ntitle: readme\n---\n# Company\n").unwrap();
            fs::write(f.join("knowledge/data/secrets/token.txt"), "hunter2").unwrap();
            fs::write(f.join("knowledge/.env"), "KEY=hunter2").unwrap();
            fs::write(f.join("secrets.yaml"), "api: hunter2").unwrap();
            fs::write(f.join("logs/daemon.log"), "a log line").unwrap();
            fs::write(f.join("policies/broken.yaml"), "framework: [unclosed").unwrap();
            fs::write(
                f.join("vex/demo/review.cdx.json"),
                include_bytes!("../../../factory-environment/tests/fixtures/dependencies/vulnerabilities.cdx.json"),
            )
            .unwrap();
            std::os::unix::fs::symlink(f.join("secrets.yaml"), f.join("knowledge/link.md")).unwrap();
            let conn = Connection::open(f.join("factory.sqlite")).unwrap();
            conn.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA user_version=4; CREATE TABLE tasks (id TEXT); INSERT INTO tasks VALUES ('t1');",
            )
            .unwrap();
            // Left open on purpose: the snapshot must be consistent while
            // another connection holds the database in WAL mode.
            std::mem::forget(conn);
            Self { root, destination }
        }

        fn plan_with(&self, include_logs: bool, encrypt_to: Option<String>) -> Plan {
            self.plan_with_scope_includes(include_logs, encrypt_to, Vec::new())
        }

        fn plan_with_scope_includes(
            &self,
            include_logs: bool,
            encrypt_to: Option<String>,
            scope_includes: Vec<(String, PathBuf, Vec<String>)>,
        ) -> Plan {
            Plan {
                root: self.root.clone(),
                database: self.root.join(".factory/factory.sqlite"),
                instance: ManifestInstance { id: "inst-1".into(), name: "Test Instance".into() },
                scope_configs: vec!["projects/demo/.factory/config.yaml".into()],
                include_logs,
                encrypt_to,
                scope_includes,
            }
        }

        fn take(&self, include_logs: bool) -> Taken {
            self.take_with(include_logs, None)
        }

        fn take_with(&self, include_logs: bool, encrypt_to: Option<String>) -> Taken {
            let destination = prepare_destination(&self.root, &self.destination).unwrap();
            let name = archive_name("Test Instance", Utc::now(), encrypt_to.is_some());
            take(&self.plan_with(include_logs, encrypt_to), &destination, &name, Utc::now()).unwrap()
        }

        /// `#279`: like `take`, with `scope.backup.include` declarations
        /// plugged into the plan -- `(scope name, scope directory relative
        /// to the root, raw `include` strings)`. The "demo" scope's own
        /// directory is `projects/demo`. `at` is explicit, never
        /// `Utc::now()`, so two calls against the same destination (a
        /// directory created between them, say) never collide on an
        /// archive name truncated to the second.
        fn take_with_scope_includes(
            &self,
            declarations: Vec<(String, PathBuf, Vec<String>)>,
            at: DateTime<Utc>,
        ) -> Taken {
            let destination = prepare_destination(&self.root, &self.destination).unwrap();
            let name = archive_name("Test Instance", at, false);
            let plan = self.plan_with_scope_includes(false, None, declarations);
            take(&plan, &destination, &name, at).unwrap()
        }

        fn manifest(archive: &Path) -> Manifest {
            Self::manifest_with(archive, None)
        }

        fn manifest_with(archive: &Path, identity: Option<&OwnerIdentity>) -> Manifest {
            let file = File::open(archive).unwrap();
            let reader: Box<dyn Read> = match identity {
                Some(identity) => {
                    let decryptor = age::Decryptor::new(file).unwrap();
                    Box::new(decryptor.decrypt(std::iter::once(&identity.inner as &dyn age::Identity)).unwrap())
                }
                None => Box::new(file),
            };
            let decoder = zstd::Decoder::new(reader).unwrap();
            let mut tar = tar::Archive::new(decoder);
            for entry in tar.entries().unwrap() {
                let mut entry = entry.unwrap();
                if entry.path().unwrap().to_string_lossy() == MANIFEST_FILE {
                    let mut text = String::new();
                    entry.read_to_string(&mut text).unwrap();
                    return serde_json::from_str(&text).unwrap();
                }
            }
            panic!("no manifest");
        }

        /// `#279`: unpack a plain `source` archive into memory, let `mutate`
        /// change its entries, then write a fresh plain archive to `into` --
        /// for a test that needs a manifest disagreeing with what is
        /// actually archived, which `take` itself can never produce on its
        /// own (it hashes and archives the same bytes).
        fn rewrite_archive(source: &Path, into: &Path, mutate: impl FnOnce(&mut BTreeMap<String, Vec<u8>>)) {
            let file = File::open(source).unwrap();
            let decoder = zstd::Decoder::new(file).unwrap();
            let mut tar_in = tar::Archive::new(decoder);
            let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
            for entry in tar_in.entries().unwrap() {
                let mut entry = entry.unwrap();
                let path = entry.path().unwrap().to_string_lossy().into_owned();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                entries.insert(path, bytes);
            }
            mutate(&mut entries);

            let out = File::create(into).unwrap();
            let encoder = zstd::Encoder::new(out, 3).unwrap();
            let mut tar_out = tar::Builder::new(encoder);
            for (path, bytes) in &entries {
                let mut header = entry_header(bytes.len() as u64, 0);
                tar_out.append_data(&mut header, path, bytes.as_slice()).unwrap();
            }
            let encoder = tar_out.into_inner().unwrap();
            encoder.finish().unwrap();
        }
    }

    impl Drop for Instance {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.root.parent().unwrap());
        }
    }

    #[test]
    fn a_snapshot_holds_the_database_and_authored_content_and_never_a_secret() {
        let instance = Instance::new("take");
        let taken = instance.take(false);
        assert!(taken.path.file_name().unwrap().to_string_lossy().starts_with("factory-backup-test-instance-"));
        let manifest = Instance::manifest(&taken.path);
        let paths: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                ".factory/budgets/limits.yaml",
                ".factory/config.yaml",
                ".factory/factory.sqlite",
                ".factory/intake/ready.yaml",
                ".factory/knowledge/company/README.md",
                ".factory/policies/broken.yaml",
                ".factory/vex/demo/review.cdx.json",
                "projects/demo/.factory/config.yaml",
            ]
        );
        assert_eq!(manifest.database.integrity, "ok");
        assert_eq!(manifest.database.user_version, 4);
        assert_eq!(manifest.database.tables, ["tasks"]);
        assert_eq!(manifest.files.iter().find(|f| f.path == ".factory/budgets/limits.yaml").map(|f| f.group), Some(Group::Budgets));
        assert_eq!(
            manifest
                .files
                .iter()
                .find(|file| file.path == ".factory/vex/demo/review.cdx.json")
                .map(|file| file.group),
            Some(Group::Vex)
        );
        assert_eq!(
            manifest
                .files
                .iter()
                .find(|file| file.path == ".factory/intake/ready.yaml")
                .map(|file| file.group),
            Some(Group::Intake)
        );
        let excluded: Vec<&str> = manifest.excluded.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            excluded,
            [".factory/knowledge/.env", ".factory/knowledge/data/secrets", ".factory/knowledge/link.md"],
            "the secrets directory, the env file and the symlink are named, never read"
        );
        // Nothing but the archive in the destination: no partial left behind.
        let left: Vec<String> = fs::read_dir(&instance.destination)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
        assert_eq!(taken.files, 8);
    }

    #[test]
    fn include_logs_adds_the_logs() {
        let instance = Instance::new("logs");
        fs::create_dir(instance.root.join(".factory/logs/service-evidence")).unwrap();
        fs::write(instance.root.join(".factory/logs/service-evidence/receipt.json"), "{\"partial\":true}").unwrap();
        let manifest = Instance::manifest(&instance.take(true).path);
        assert!(manifest.files.iter().any(|f| f.path == ".factory/logs/daemon.log" && f.group == Group::Logs));
        assert!(manifest.files.iter().any(|f| f.path == ".factory/logs/service-evidence/receipt.json" && f.group == Group::Logs));
        let excluded = take(&instance.plan_with(false, None), &instance.destination, "without-logs.tar.zst", Utc::now()).unwrap();
        assert!(!Instance::manifest(&excluded.path).files.iter().any(|f| f.path.contains("service-evidence")));
    }

    #[test]
    fn a_fresh_snapshot_verifies_with_the_broken_policy_as_a_warning_only() {
        let instance = Instance::new("verify");
        let taken = instance.take(false);
        let checks = verify(&taken.path, "inst-1", None);
        let by_name: BTreeMap<&str, &VerifyCheck> = checks.iter().map(|c| (c.name.as_str(), c)).collect();
        for name in [
            "archive", "manifest", "checksums", "database", "config", "goals", "scenarios", "quality", "vex", "intake",
            "datasets", "knowledge", "budgets",
        ] {
            assert_eq!(by_name[name].status, CheckStatus::Ok, "{name}: {}", by_name[name].detail);
        }
        assert_eq!(by_name["policies"].status, CheckStatus::Warn);
        assert!(by_name["policies"].detail.contains("broken.yaml"), "{}", by_name["policies"].detail);
        assert!(by_name["database"].detail.contains("1 tasks"), "{}", by_name["database"].detail);
    }

    #[test]
    fn malformed_budget_intent_is_a_named_verification_warning_not_an_absent_budget() {
        let instance = Instance::new("broken-budget");
        fs::write(instance.root.join(".factory/budgets/limits.yaml"), "version: 1\nscopes: {demo-id: {monthly_usd: -1}}\n").unwrap();
        let taken = instance.take(false);
        let checks = verify(&taken.path, "inst-1", None);
        let check = checks.iter().find(|c| c.name == "budgets").unwrap();
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains("limits.yaml") && check.detail.contains("nonnegative"), "{}", check.detail);
    }

    #[test]
    fn a_malformed_vex_document_is_named_by_verification() {
        let instance = Instance::new("broken-vex");
        fs::write(
            instance.root.join(".factory/vex/demo/broken.cdx.json"),
            br#"{"bomFormat":"not CycloneDX"}"#,
        )
        .unwrap();
        let taken = instance.take(false);
        let checks = verify(&taken.path, "inst-1", None);
        let vex = checks.iter().find(|check| check.name == "vex").unwrap();
        assert_eq!(vex.status, CheckStatus::Warn);
        assert!(vex.detail.contains("demo/broken.cdx.json"), "{}", vex.detail);
        assert!(vex.detail.contains("bomFormat"), "{}", vex.detail);
    }

    #[test]
    fn a_malformed_definition_of_ready_is_named_by_verification() {
        let instance = Instance::new("broken-intake");
        fs::write(instance.root.join(".factory/intake/ready.yaml"), "checks: [unclosed").unwrap();
        let taken = instance.take(false);
        let checks = verify(&taken.path, "inst-1", None);
        let intake = checks.iter().find(|check| check.name == "intake").unwrap();
        assert_eq!(intake.status, CheckStatus::Warn);
        assert!(intake.detail.contains("ready.yaml"), "{}", intake.detail);
    }

    #[test]
    fn a_damaged_archive_fails_verification_and_says_so() {
        let instance = Instance::new("damaged");
        let taken = instance.take(false);
        let mut bytes = fs::read(&taken.path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        fs::write(&taken.path, &bytes).unwrap();
        let checks = verify(&taken.path, "inst-1", None);
        assert!(checks.iter().any(|c| c.status == CheckStatus::Fail), "{checks:?}");
    }

    #[test]
    fn a_verified_snapshot_restores_into_a_new_or_empty_root() {
        let instance = Instance::new("restore");
        let taken = instance.take(false);
        let base = instance.root.parent().unwrap();

        let into = base.join("restored");
        let restored = restore(&taken.path, "inst-1", &instance.root, &into, None).unwrap();
        assert_eq!(restored.into, into.canonicalize().unwrap());
        assert_eq!(restored.files, 8);
        assert_eq!(fs::read(into.join(".factory/budgets/limits.yaml")).unwrap(), fs::read(instance.root.join(".factory/budgets/limits.yaml")).unwrap());
        assert!(!restored.checks.iter().any(|c| c.status == CheckStatus::Fail));
        assert!(factory_core::config::Factory::load(&into).is_ok());
        assert_eq!(fs::read_to_string(into.join(".factory/knowledge/company/README.md")).unwrap(), "---\ntitle: readme\n---\n# Company\n");
        assert_eq!(
            fs::read(into.join(".factory/vex/demo/review.cdx.json")).unwrap(),
            include_bytes!("../../../factory-environment/tests/fixtures/dependencies/vulnerabilities.cdx.json")
        );
        assert!(into.join("projects/demo/.factory/config.yaml").is_file());
        assert!(!into.join(".factory/secrets.yaml").exists());
        assert!(!into.join(".factory/knowledge/data/secrets/token.txt").exists());
        let conn = Connection::open_with_flags(
            into.join(DATABASE_ENTRY),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get::<_, i64>(0)).unwrap(), 1);

        let empty = base.join("restored-empty");
        fs::create_dir(&empty).unwrap();
        restore(&taken.path, "inst-1", &instance.root, &empty, None).unwrap();
        assert!(empty.join(DATABASE_ENTRY).is_file());
    }

    #[test]
    fn restore_refuses_the_active_or_populated_root_without_changing_it() {
        let instance = Instance::new("restore-refuse");
        let taken = instance.take(false);
        let active = restore(&taken.path, "inst-1", &instance.root, &instance.root, None)
            .unwrap_err()
            .to_string();
        assert!(active.contains("running instance root"), "{active}");

        let occupied = instance.root.parent().unwrap().join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("keep.txt"), "untouched").unwrap();
        let error = restore(&taken.path, "inst-1", &instance.root, &occupied, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("not empty"), "{error}");
        assert_eq!(fs::read_to_string(occupied.join("keep.txt")).unwrap(), "untouched");

        let dangling = instance.root.parent().unwrap().join("dangling");
        std::os::unix::fs::symlink("missing", &dangling).unwrap();
        let error = restore(&taken.path, "inst-1", &instance.root, &dangling, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("symbolic link"), "{error}");
        assert!(fs::symlink_metadata(&dangling).unwrap().file_type().is_symlink());
    }

    #[test]
    fn a_failed_restore_never_commits_a_root_or_leaves_its_staging_directory() {
        let instance = Instance::new("restore-damaged");
        let taken = instance.take(false);
        let mut bytes = fs::read(&taken.path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        fs::write(&taken.path, bytes).unwrap();
        let parent = instance.root.parent().unwrap();
        let into = parent.join("not-committed");
        let error = restore(&taken.path, "inst-1", &instance.root, &into, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("verification failed"), "{error}");
        assert!(!into.exists());
        let staging: Vec<_> = fs::read_dir(parent)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".factory-restore-"))
            .collect();
        assert!(staging.is_empty(), "failed restore left staging directories");
    }

    #[test]
    fn a_database_schema_this_daemon_would_discard_is_not_restorable() {
        let instance = Instance::new("restore-schema");
        let taken = instance.take(false);
        let mut manifest = Instance::manifest(&taken.path);
        let database = instance.root.join(DATABASE_ENTRY);
        let conn = Connection::open(&database).unwrap();
        conn.pragma_update(None, "user_version", 999i64).unwrap();
        manifest.database.user_version = 999;
        let result = verify_database(&database, &manifest);
        assert_eq!(result.status, CheckStatus::Fail);
        assert!(result.detail.contains("incompatible"), "{}", result.detail);
        assert!(result.detail.contains("discard task data"), "{}", result.detail);
    }

    #[test]
    fn a_destination_inside_factory_or_on_an_unmounted_disk_is_refused() {
        let instance = Instance::new("dest");
        let inside = instance.root.join(".factory/backups");
        assert!(prepare_destination(&instance.root, &inside).unwrap_err().to_string().contains("inside"));
        let unmounted = instance.root.join("no-such-volume/factory");
        let e = prepare_destination(&instance.root, &unmounted).unwrap_err().to_string();
        assert!(e.contains("mounted"), "{e}");
        assert!(!unmounted.parent().unwrap().exists(), "nothing was created on the way");
    }

    #[test]
    fn an_archive_path_that_climbs_out_is_never_written() {
        assert!(safe_relative(Path::new("../../etc/passwd")).is_none());
        assert!(safe_relative(Path::new("/etc/passwd")).is_none());
        assert_eq!(safe_relative(Path::new("./.factory/x")), Some(PathBuf::from(".factory/x")));
    }

    // ============================================================ #152: age

    /// A generated identity, written to a file under `base` (never inside an
    /// instance's own `.factory/`), plus its public recipient.
    fn generated_identity(base: &Path) -> (PathBuf, String) {
        use age::secrecy::ExposeSecret;
        let identity = age::x25519::Identity::generate();
        let recipient = identity.to_public().to_string();
        let path = base.join(format!("identity-{}.txt", uuid::Uuid::new_v4()));
        fs::write(&path, identity.to_string().expose_secret()).unwrap();
        (path, recipient)
    }

    /// `OwnerIdentity` deliberately has no `Debug` impl (nothing should be
    /// able to print it, even by accident), so `Result::unwrap_err` -- which
    /// needs `T: Debug` to format the `Ok` case it did not get -- cannot be
    /// used on it directly. This is that assertion instead.
    fn identity_err(result: Result<OwnerIdentity>) -> String {
        match result {
            Ok(_) => panic!("expected the identity to be refused"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn an_encrypted_snapshot_is_named_and_written_with_no_plaintext_byte_in_the_destination() {
        let instance = Instance::new("encrypt-take");
        let (_, recipient) = generated_identity(instance.root.parent().unwrap());
        let taken = instance.take_with(false, Some(recipient));
        let name = taken.path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with(".tar.zst.age"), "{name}");

        let bytes = fs::read(&taken.path).unwrap();
        assert!(
            factory_core::backup::looks_encrypted(&bytes),
            "the archive does not start with the age magic"
        );
        assert_ne!(&bytes[..4], &[0x28, 0xb5, 0x2f, 0xfd], "the first bytes are a zstd frame, not ciphertext");
        let haystack = String::from_utf8_lossy(&bytes);
        for marker in ["manifest.json", "Test Instance", "README", "Company", MANIFEST_FILE] {
            assert!(!haystack.contains(marker), "plaintext marker {marker:?} leaked into the archive");
        }

        // Nothing but the one encrypted archive in the destination: no
        // partial, no plaintext sibling.
        let left: Vec<String> = fs::read_dir(&instance.destination)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, [name]);
    }

    #[test]
    fn an_encrypted_snapshot_verifies_with_the_right_identity_and_fails_cleanly_with_the_wrong_one() {
        let instance = Instance::new("encrypt-verify");
        let base = instance.root.parent().unwrap().to_path_buf();
        let (identity_path, recipient) = generated_identity(&base);
        let taken = instance.take_with(false, Some(recipient.clone()));

        let manifest = Instance::manifest_with(&taken.path, Some(&OwnerIdentity::read(&identity_path, &instance.root).unwrap()));
        assert!(manifest.files.iter().any(|f| f.path == ".factory/config.yaml"), "the manifest decrypts and parses");

        let identity = OwnerIdentity::read(&identity_path, &instance.root).unwrap();
        assert_eq!(identity.recipient(), recipient);
        let checks = verify(&taken.path, "inst-1", Some(&identity));
        assert!(!checks.iter().any(|c| c.status == CheckStatus::Fail), "{checks:?}");
        let decrypt = checks.iter().find(|c| c.name == "decrypt").expect("a decrypt check");
        assert_eq!(decrypt.status, CheckStatus::Ok);
        assert!(decrypt.detail.contains(&recipient), "{}", decrypt.detail);

        let (wrong_path, wrong_recipient) = generated_identity(&base);
        let wrong = OwnerIdentity::read(&wrong_path, &instance.root).unwrap();
        let checks = verify(&taken.path, "inst-1", Some(&wrong));
        assert_eq!(checks.len(), 1, "decryption fails before anything else is even attempted: {checks:?}");
        let decrypt = &checks[0];
        assert_eq!(decrypt.name, "decrypt");
        assert_eq!(decrypt.status, CheckStatus::Fail);
        assert!(decrypt.detail.contains(&wrong_recipient), "{}", decrypt.detail);
    }

    #[test]
    fn an_encrypted_archive_verified_with_no_identity_fails_at_the_archive_step_rather_than_panicking() {
        // The daemon (`backup::mod`) refuses this before it ever calls
        // `verify` at all; this only proves the archive layer itself never
        // panics or silently succeeds if that gate were ever bypassed.
        let instance = Instance::new("encrypt-no-identity");
        let (_, recipient) = generated_identity(instance.root.parent().unwrap());
        let taken = instance.take_with(false, Some(recipient));
        let checks = verify(&taken.path, "inst-1", None);
        assert!(checks.iter().any(|c| c.status == CheckStatus::Fail), "{checks:?}");
    }

    #[test]
    fn an_encrypted_snapshot_restores_with_the_right_identity_and_refuses_without_one() {
        let instance = Instance::new("encrypt-restore");
        let base = instance.root.parent().unwrap().to_path_buf();
        let (identity_path, recipient) = generated_identity(&base);
        let taken = instance.take_with(false, Some(recipient));
        let identity = OwnerIdentity::read(&identity_path, &instance.root).unwrap();

        let into = base.join("restored-encrypted");
        let restored = restore(&taken.path, "inst-1", &instance.root, &into, Some(&identity)).unwrap();
        assert!(factory_core::config::Factory::load(&restored.into).is_ok());
        assert!(!restored.into.join(".factory/secrets.yaml").exists());

        let into_refused = base.join("restored-encrypted-no-identity");
        let error = restore(&taken.path, "inst-1", &instance.root, &into_refused, None).unwrap_err().to_string();
        assert!(error.contains("verification failed"), "{error}");
        assert!(!into_refused.exists());
    }

    #[test]
    fn owner_identity_refuses_a_path_inside_factory_and_anything_but_one_native_key() {
        let instance = Instance::new("identity-refuse");
        let base = instance.root.parent().unwrap().to_path_buf();

        let inside = instance.root.join(".factory/identity.txt");
        fs::write(&inside, "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ\n").unwrap();
        let e = identity_err(OwnerIdentity::read(&inside, &instance.root));
        assert!(e.contains("inside this instance's own"), "{e}");

        let relative = PathBuf::from("relative/identity.txt");
        let e = identity_err(OwnerIdentity::read(&relative, &instance.root));
        assert!(e.contains("absolute"), "{e}");

        let ssh = base.join("ssh-identity.txt");
        fs::write(&ssh, "# a comment, never quoted back\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBogus not-a-real-key\n").unwrap();
        let e = identity_err(OwnerIdentity::read(&ssh, &instance.root));
        assert!(e.contains("line 2"), "{e}");
        assert!(!e.contains("ssh-ed25519"), "the line's own text must never appear in the error: {e}");

        let empty = base.join("empty-identity.txt");
        fs::write(&empty, "# only a comment\n").unwrap();
        let e = identity_err(OwnerIdentity::read(&empty, &instance.root));
        assert!(e.contains("no identity"), "{e}");

        let (one_path, _) = generated_identity(&base);
        let (two_path, _) = generated_identity(&base);
        let both = format!(
            "{}\n{}\n",
            fs::read_to_string(&one_path).unwrap().trim(),
            fs::read_to_string(&two_path).unwrap().trim()
        );
        let multi = base.join("multi-identity.txt");
        fs::write(&multi, &both).unwrap();
        let e = identity_err(OwnerIdentity::read(&multi, &instance.root));
        assert!(e.contains("more than one identity"), "{e}");
    }

    #[test]
    fn a_generated_identitys_secret_never_appears_in_an_owner_identity_error() {
        let instance = Instance::new("identity-secret");
        let base = instance.root.parent().unwrap().to_path_buf();
        let (identity_path, _) = generated_identity(&base);
        let secret_line = fs::read_to_string(&identity_path).unwrap().trim().to_string();
        // A file holding the real secret plus a second, unparsable line: the
        // resulting error must name the line number, never quote either
        // line's own text.
        let corrupted = format!("{secret_line}\nnot-an-identity-at-all\n");
        fs::write(&identity_path, &corrupted).unwrap();
        let e = identity_err(OwnerIdentity::read(&identity_path, &instance.root));
        assert!(e.contains("line 2"), "{e}");
        assert!(!e.contains(&secret_line), "the identity's own secret text leaked into the error: {e}");
        assert!(!e.contains("not-an-identity-at-all"), "the offending line's own text leaked into the error: {e}");
    }

    // ================================================================= #279
    //
    // The "demo" scope `Instance::new` already writes lives at
    // `projects/demo`; `../../data/<x>` from there resolves to the
    // instance-root-level `data/<x>` the issue's own example uses.

    #[test]
    fn a_declared_directory_is_archived_with_sha256_entries_and_counted() {
        let instance = Instance::new("scope-include-take");
        fs::create_dir_all(instance.root.join("data/finance")).unwrap();
        fs::write(instance.root.join("data/finance/ledger.csv"), "2026-01-01,100.00\n").unwrap();
        fs::write(instance.root.join("data/finance/receipt.pdf"), "%PDF-fake").unwrap();
        let declarations = vec![("demo".into(), PathBuf::from("projects/demo"), vec!["../../data/finance".into()])];
        let taken = instance.take_with_scope_includes(declarations, Utc::now());

        let manifest = Instance::manifest(&taken.path);
        let ledger = manifest.files.iter().find(|f| f.path == "data/finance/ledger.csv").expect("ledger archived");
        assert_eq!(ledger.group, Group::ScopeData);
        assert_eq!(ledger.sha256, hex(&Sha256::digest(b"2026-01-01,100.00\n")));
        assert!(manifest.files.iter().any(|f| f.path == "data/finance/receipt.pdf"));

        assert_eq!(taken.scope_includes.len(), 1);
        let total = &taken.scope_includes[0];
        assert_eq!(total.scope, "demo");
        assert_eq!(total.declared, "../../data/finance");
        assert_eq!(total.path.as_deref(), Some("data/finance"));
        assert_eq!(total.unavailable, None);
        assert_eq!(total.files, 2);
        assert_eq!(total.bytes, "2026-01-01,100.00\n".len() as u64 + "%PDF-fake".len() as u64);
    }

    #[test]
    fn verify_fails_on_an_included_file_that_is_missing_changed_or_unlisted() {
        let instance = Instance::new("scope-include-verify-fail");
        fs::create_dir_all(instance.root.join("data/finance")).unwrap();
        fs::write(instance.root.join("data/finance/ledger.csv"), "original\n").unwrap();
        let declarations = vec![("demo".into(), PathBuf::from("projects/demo"), vec!["../../data/finance".into()])];
        let taken = instance.take_with_scope_includes(declarations, Utc::now());
        let checksums_failed =
            |archive: &Path| verify(archive, "inst-1", None).iter().any(|c| c.name == "checksums" && c.status == CheckStatus::Fail);

        // Sanity: the snapshot `take` itself wrote verifies clean.
        assert!(!checksums_failed(&taken.path));

        let changed = instance.destination.join("changed.tar.zst");
        Instance::rewrite_archive(&taken.path, &changed, |entries| {
            entries.insert("data/finance/ledger.csv".into(), b"tampered\n".to_vec());
        });
        assert!(checksums_failed(&changed), "changed bytes must fail checksums");

        let missing = instance.destination.join("missing.tar.zst");
        Instance::rewrite_archive(&taken.path, &missing, |entries| {
            entries.remove("data/finance/ledger.csv");
        });
        assert!(checksums_failed(&missing), "a manifest entry with nothing archived for it must fail checksums");

        let unlisted = instance.destination.join("unlisted.tar.zst");
        Instance::rewrite_archive(&taken.path, &unlisted, |entries| {
            entries.insert("data/finance/extra.csv".into(), b"surprise\n".to_vec());
        });
        assert!(checksums_failed(&unlisted), "an archived file the manifest never named must fail checksums");
    }

    #[test]
    fn overlapping_declared_directories_archive_each_file_once_and_restore_cleanly() {
        let instance = Instance::new("scope-include-overlap");
        fs::create_dir_all(instance.root.join("data/finance")).unwrap();
        fs::write(instance.root.join("data/finance/ledger.csv"), "ledger\n").unwrap();
        fs::write(instance.root.join("data/other.csv"), "other\n").unwrap();
        // The broader declaration is listed first; the narrower one is a
        // subdirectory of it, declared separately (by the same scope here,
        // but the rule is the same across two different scopes).
        let declarations = vec![(
            "demo".into(),
            PathBuf::from("projects/demo"),
            vec!["../../data".into(), "../../data/finance".into()],
        )];
        let taken = instance.take_with_scope_includes(declarations, Utc::now());

        let manifest = Instance::manifest(&taken.path);
        for path in ["data/finance/ledger.csv", "data/other.csv"] {
            let matches: Vec<&ManifestFile> = manifest.files.iter().filter(|f| f.path == path).collect();
            assert_eq!(matches.len(), 1, "{path} must appear exactly once: {:?}", manifest.files);
        }

        // Each declaration's own total still counts a file it covers, even
        // one the *other* declaration's walk was the one that actually
        // archived -- the totals are a prefix match over the final,
        // deduplicated file list, never tied to which call added what.
        assert_eq!(taken.scope_includes.len(), 2);
        let broad = taken.scope_includes.iter().find(|t| t.declared == "../../data").unwrap();
        assert_eq!(broad.files, 2, "both files are under data/: {taken:?}");
        let narrow = taken.scope_includes.iter().find(|t| t.declared == "../../data/finance").unwrap();
        assert_eq!(narrow.files, 1, "only ledger.csv is under data/finance/: {taken:?}");

        let checks = verify(&taken.path, "inst-1", None);
        assert!(checks.iter().all(|c| c.status != CheckStatus::Fail), "{checks:?}");

        let into = instance.root.parent().unwrap().join("restored-overlap");
        let restored = restore(&taken.path, "inst-1", &instance.root, &into, None).unwrap();
        assert!(!restored.checks.iter().any(|c| c.status == CheckStatus::Fail), "{:?}", restored.checks);
        assert_eq!(fs::read(into.join("data/finance/ledger.csv")).unwrap(), b"ledger\n");
        assert_eq!(fs::read(into.join("data/other.csv")).unwrap(), b"other\n");
    }

    #[test]
    fn a_scope_declaring_its_own_directory_does_not_duplicate_its_already_archived_config() {
        // A directory a scope declares can perfectly well contain that
        // scope's (or a nested scope's) own `.factory/config.yaml` --
        // already archived as `Group::Config` by the config loop above,
        // which runs first. Re-finding it while walking the declared
        // directory must not add a second, `Group::ScopeData` copy.
        let instance = Instance::new("scope-include-self");
        let declarations = vec![("demo".into(), PathBuf::from("projects/demo"), vec![".".into()])];
        let taken = instance.take_with_scope_includes(declarations, Utc::now());

        let manifest = Instance::manifest(&taken.path);
        let matches: Vec<&ManifestFile> =
            manifest.files.iter().filter(|f| f.path == "projects/demo/.factory/config.yaml").collect();
        assert_eq!(matches.len(), 1, "{:?}", manifest.files);
        assert_eq!(matches[0].group, Group::Config, "the earlier claim wins, so Config stays Config");

        let checks = verify(&taken.path, "inst-1", None);
        assert!(checks.iter().all(|c| c.status != CheckStatus::Fail), "{checks:?}");
    }

    #[test]
    fn the_same_entry_declared_twice_is_archived_once() {
        let instance = Instance::new("scope-include-duplicate-declaration");
        fs::create_dir_all(instance.root.join("data/finance")).unwrap();
        fs::write(instance.root.join("data/finance/ledger.csv"), "ok\n").unwrap();
        let declarations = vec![(
            "demo".into(),
            PathBuf::from("projects/demo"),
            vec!["../../data/finance".into(), "../../data/finance".into()],
        )];
        let taken = instance.take_with_scope_includes(declarations, Utc::now());

        let manifest = Instance::manifest(&taken.path);
        let matches: Vec<&ManifestFile> = manifest.files.iter().filter(|f| f.path == "data/finance/ledger.csv").collect();
        assert_eq!(matches.len(), 1, "{:?}", manifest.files);
    }

    #[test]
    fn restore_places_a_declared_directory_at_its_relative_path_and_never_touches_the_live_root() {
        let instance = Instance::new("scope-include-restore");
        fs::create_dir_all(instance.root.join("data/finance")).unwrap();
        fs::write(instance.root.join("data/finance/ledger.csv"), "2026-01-01,100.00\n").unwrap();
        let declarations = vec![("demo".into(), PathBuf::from("projects/demo"), vec!["../../data/finance".into()])];
        let taken = instance.take_with_scope_includes(declarations, Utc::now());
        let original = fs::metadata(instance.root.join("data/finance/ledger.csv")).unwrap().modified().unwrap();

        let into = instance.root.parent().unwrap().join("restored-scope-data");
        let restored = restore(&taken.path, "inst-1", &instance.root, &into, None).unwrap();
        assert!(!restored.checks.iter().any(|c| c.status == CheckStatus::Fail), "{:?}", restored.checks);
        assert_eq!(fs::read(into.join("data/finance/ledger.csv")).unwrap(), b"2026-01-01,100.00\n");

        // The live root is untouched -- same bytes, same mtime, still there.
        assert_eq!(fs::read(instance.root.join("data/finance/ledger.csv")).unwrap(), b"2026-01-01,100.00\n");
        assert_eq!(fs::metadata(instance.root.join("data/finance/ledger.csv")).unwrap().modified().unwrap(), original);
    }

    #[test]
    fn secrets_env_files_and_symlinks_inside_an_included_directory_are_never_archived() {
        let instance = Instance::new("scope-include-exclusions");
        fs::create_dir_all(instance.root.join("data/finance/secrets")).unwrap();
        fs::write(instance.root.join("data/finance/secrets/api-key.txt"), "hunter2").unwrap();
        fs::write(instance.root.join("data/finance/.env"), "KEY=hunter2").unwrap();
        fs::write(instance.root.join("data/finance/ledger.csv"), "ok\n").unwrap();
        std::os::unix::fs::symlink(
            instance.root.join("data/finance/ledger.csv"),
            instance.root.join("data/finance/ledger-link.csv"),
        )
        .unwrap();
        fs::create_dir(instance.root.join("data/finance/exports-elsewhere")).unwrap();
        std::os::unix::fs::symlink(
            instance.root.join("data/finance/exports-elsewhere"),
            instance.root.join("data/finance/exports"),
        )
        .unwrap();
        let declarations = vec![("demo".into(), PathBuf::from("projects/demo"), vec!["../../data/finance".into()])];
        let taken = instance.take_with_scope_includes(declarations, Utc::now());

        let manifest = Instance::manifest(&taken.path);
        let scope_data_paths: Vec<&str> =
            manifest.files.iter().filter(|f| f.group == Group::ScopeData).map(|f| f.path.as_str()).collect();
        assert_eq!(scope_data_paths, ["data/finance/ledger.csv"], "only the plain file is archived: {scope_data_paths:?}");
        let excluded: Vec<&str> = manifest.excluded.iter().map(|e| e.path.as_str()).collect();
        assert!(excluded.contains(&"data/finance/.env"), "{excluded:?}");
        assert!(excluded.contains(&"data/finance/secrets"), "{excluded:?}");
        assert!(excluded.contains(&"data/finance/ledger-link.csv"), "{excluded:?}");
        assert!(excluded.contains(&"data/finance/exports"), "{excluded:?}");
        assert_eq!(taken.scope_includes[0].files, 1);
    }

    #[test]
    fn a_declaration_under_secrets_or_escaping_the_root_is_a_finding_not_a_failed_backup() {
        let instance = Instance::new("scope-include-refused");
        let declarations = vec![(
            "demo".into(),
            PathBuf::from("projects/demo"),
            vec!["secrets/dump".into(), "../../../etc".into()],
        )];
        let taken = instance.take_with_scope_includes(declarations, Utc::now());

        assert_eq!(taken.scope_includes.len(), 2);
        let by_declared: BTreeMap<&str, &ScopeIncludeTotal> =
            taken.scope_includes.iter().map(|t| (t.declared.as_str(), t)).collect();
        assert_eq!(by_declared["secrets/dump"].unavailable.as_deref(), Some("a secret: never read, never copied"));
        assert_eq!(by_declared["secrets/dump"].path, None);
        assert_eq!(by_declared["../../../etc"].unavailable.as_deref(), Some("escapes the instance root"));
        assert_eq!(by_declared["../../../etc"].files, 0);

        let manifest = Instance::manifest(&taken.path);
        assert!(manifest.files.iter().all(|f| f.group != Group::ScopeData), "nothing was archived for either");
        let excluded: Vec<&str> = manifest.excluded.iter().map(|e| e.path.as_str()).collect();
        assert!(excluded.iter().any(|p| p.contains("secrets/dump")), "{excluded:?}");
        assert!(excluded.iter().any(|p| p.contains("../../../etc")), "{excluded:?}");

        // The verify/checksum path treats this exactly like any other
        // snapshot: a refused declaration is not a failed backup.
        let checks = verify(&taken.path, "inst-1", None);
        assert!(checks.iter().all(|c| c.status != CheckStatus::Fail), "{checks:?}");
    }

    #[test]
    fn a_missing_declared_directory_is_a_finding_then_included_automatically_once_created() {
        let instance = Instance::new("scope-include-missing-then-created");
        let declarations = vec![("demo".into(), PathBuf::from("projects/demo"), vec!["../../data/finance".into()])];

        let first = instance.take_with_scope_includes(declarations.clone(), Utc::now());
        assert_eq!(first.scope_includes[0].unavailable.as_deref(), Some("declared, but does not exist yet"));
        assert_eq!(first.scope_includes[0].files, 0);
        let first_manifest = Instance::manifest(&first.path);
        assert!(first_manifest.files.iter().all(|f| f.group != Group::ScopeData));
        assert!(
            first_manifest.excluded.iter().any(|e| e.path.contains("data/finance")),
            "{:?}",
            first_manifest.excluded
        );

        // Created later, with no restart and no change to the declaration
        // itself -- `take` resolves it fresh every time it runs.
        fs::create_dir_all(instance.root.join("data/finance")).unwrap();
        fs::write(instance.root.join("data/finance/ledger.csv"), "ok\n").unwrap();
        let second = instance.take_with_scope_includes(declarations, Utc::now() + chrono::Duration::seconds(1));
        assert_eq!(second.scope_includes[0].unavailable, None);
        assert_eq!(second.scope_includes[0].files, 1);
        let second_manifest = Instance::manifest(&second.path);
        assert!(second_manifest.files.iter().any(|f| f.path == "data/finance/ledger.csv"));
    }

    #[test]
    fn a_snapshot_with_no_declared_includes_still_verifies_exactly_as_before() {
        // No `scope.backup.include` at all -- the acceptance case every
        // existing instance is in today. Nothing about the archive, the
        // manifest or verification changes for it.
        let instance = Instance::new("scope-include-none-declared");
        let taken = instance.take(false);
        assert!(taken.scope_includes.is_empty());
        assert!(Instance::manifest(&taken.path).files.iter().all(|f| f.group != Group::ScopeData));
        let checks = verify(&taken.path, "inst-1", None);
        assert!(checks.iter().all(|c| c.status != CheckStatus::Fail), "{checks:?}");
    }
}
