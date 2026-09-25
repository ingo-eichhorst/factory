//! Taking a snapshot and proving one would restore. Blocking file work and
//! nothing else -- the engine moves every call here onto `spawn_blocking`,
//! and decides nothing about schedules, retention or warnings.
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
    ManifestInstance, VerifyCheck, AUTHORED, DATABASE_ENTRY, MANIFEST_FILE, MANIFEST_VERSION, OPTIONAL,
};
use factory_core::config::{CONFIG_FILE, FACTORY_DIR};
use factory_core::error::{FactoryError, Result};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

fn fail(message: impl Into<String>) -> FactoryError {
    FactoryError::adapter("backup", message)
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
}

/// A snapshot as written.
#[derive(Debug)]
pub struct Taken {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub files: u64,
    pub database_bytes: u64,
    pub groups: Vec<GroupTotal>,
}

/// A directory removed when this goes out of scope, however that happens.
struct Scratch(PathBuf);

impl Scratch {
    fn new(what: &str) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("factory-{what}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).map_err(|e| fail(format!("making a temporary directory {}: {e}", dir.display())))?;
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
    let mut excluded = Vec::new();
    let mut entries: Vec<(String, PathBuf, Group)> = Vec::new();
    let root_config = format!("{FACTORY_DIR}/{CONFIG_FILE}");
    entries.push((root_config.clone(), plan.root.join(&root_config), Group::Config));
    for scope in &plan.scope_configs {
        if *scope != root_config && !entries.iter().any(|(p, ..)| p == scope) {
            entries.push((scope.clone(), plan.root.join(scope), Group::Config));
        }
    }
    let optional = if plan.include_logs { &OPTIONAL[..] } else { &[] };
    for (group, dir, _) in AUTHORED.iter().chain(optional) {
        walk(&plan.root, dir.trim_end_matches('/'), *group, &mut entries, &mut excluded);
    }

    // -- the archive -------------------------------------------------------
    let partial = destination.join(format!(".{name}.partial"));
    let written = (|| -> Result<(u64, Vec<ManifestFile>)> {
        let file = File::create(&partial).map_err(|e| fail(format!("creating {}: {e}", partial.display())))?;
        let encoder = zstd::Encoder::new(BufWriter::new(file), 3).map_err(|e| fail(format!("zstd: {e}")))?;
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
        let buffered = encoder.finish().map_err(|e| fail(format!("finishing zstd: {e}")))?;
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
    Ok(Taken {
        size_bytes: fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0),
        path: final_path,
        files: files.len() as u64,
        database_bytes,
        groups: groups.into_values().collect(),
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
fn walk(root: &Path, dir: &str, group: Group, out: &mut Vec<(String, PathBuf, Group)>, excluded: &mut Vec<ExcludedFile>) {
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
            walk(root, &relative, group, out, excluded);
        } else if meta.is_file() {
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
pub fn verify(archive: &Path, instance_id: &str) -> Vec<VerifyCheck> {
    let mut checks = Vec::new();
    let scratch = match Scratch::new("verify") {
        Ok(s) => s,
        Err(e) => {
            checks.push(check("archive", CheckStatus::Fail, e.to_string()));
            return checks;
        }
    };
    let tmp = &scratch.0;

    // -- unpack, hashing every entry on its way to disk ---------------------
    let mut unpacked: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut manifest_text: Option<String> = None;
    let unpacking = (|| -> std::result::Result<(), String> {
        let file = File::open(archive).map_err(|e| format!("opening {}: {e}", archive.display()))?;
        let decoder = zstd::Decoder::new(file).map_err(|e| format!("zstd: {e}"))?;
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
        Ok(_) => {}
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
        check("config", CheckStatus::Ok, format!("the root config loads; {} config files parse", configs.len()))
    } else {
        check("config", CheckStatus::Fail, summarize(&bad))
    });

    // -- every authored-content loader ------------------------------------------
    checks.extend(verify_loaders(tmp));
    checks
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
    use factory_core::{goals, knowledge, policy, quality, scenario};
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
            fs::create_dir_all(f.join("logs")).unwrap();
            fs::create_dir_all(root.join("projects/demo/.factory")).unwrap();
            fs::write(f.join("config.yaml"), "version: 1\ninstance:\n  id: inst-1\n  name: Test Instance\n").unwrap();
            fs::write(root.join("projects/demo/.factory/config.yaml"), "version: 1\nscope:\n  name: demo\n").unwrap();
            fs::write(f.join("knowledge/company/README.md"), "---\ntitle: readme\n---\n# Company\n").unwrap();
            fs::write(f.join("knowledge/data/secrets/token.txt"), "hunter2").unwrap();
            fs::write(f.join("knowledge/.env"), "KEY=hunter2").unwrap();
            fs::write(f.join("secrets.yaml"), "api: hunter2").unwrap();
            fs::write(f.join("logs/daemon.log"), "a log line").unwrap();
            fs::write(f.join("policies/broken.yaml"), "framework: [unclosed").unwrap();
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

        fn plan(&self, include_logs: bool) -> Plan {
            Plan {
                root: self.root.clone(),
                database: self.root.join(".factory/factory.sqlite"),
                instance: ManifestInstance { id: "inst-1".into(), name: "Test Instance".into() },
                scope_configs: vec!["projects/demo/.factory/config.yaml".into()],
                include_logs,
            }
        }

        fn take(&self, include_logs: bool) -> Taken {
            let destination = prepare_destination(&self.root, &self.destination).unwrap();
            let name = archive_name("Test Instance", Utc::now());
            take(&self.plan(include_logs), &destination, &name, Utc::now()).unwrap()
        }

        fn manifest(archive: &Path) -> Manifest {
            let decoder = zstd::Decoder::new(File::open(archive).unwrap()).unwrap();
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
                ".factory/config.yaml",
                ".factory/factory.sqlite",
                ".factory/knowledge/company/README.md",
                ".factory/policies/broken.yaml",
                "projects/demo/.factory/config.yaml",
            ]
        );
        assert_eq!(manifest.database.integrity, "ok");
        assert_eq!(manifest.database.user_version, 4);
        assert_eq!(manifest.database.tables, ["tasks"]);
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
        assert_eq!(taken.files, 5);
    }

    #[test]
    fn include_logs_adds_the_logs() {
        let instance = Instance::new("logs");
        let manifest = Instance::manifest(&instance.take(true).path);
        assert!(manifest.files.iter().any(|f| f.path == ".factory/logs/daemon.log" && f.group == Group::Logs));
    }

    #[test]
    fn a_fresh_snapshot_verifies_with_the_broken_policy_as_a_warning_only() {
        let instance = Instance::new("verify");
        let taken = instance.take(false);
        let checks = verify(&taken.path, "inst-1");
        let by_name: BTreeMap<&str, &VerifyCheck> = checks.iter().map(|c| (c.name.as_str(), c)).collect();
        for name in ["archive", "manifest", "checksums", "database", "config", "goals", "scenarios", "quality", "datasets", "knowledge"] {
            assert_eq!(by_name[name].status, CheckStatus::Ok, "{name}: {}", by_name[name].detail);
        }
        assert_eq!(by_name["policies"].status, CheckStatus::Warn);
        assert!(by_name["policies"].detail.contains("broken.yaml"), "{}", by_name["policies"].detail);
        assert!(by_name["database"].detail.contains("1 tasks"), "{}", by_name["database"].detail);
    }

    #[test]
    fn a_damaged_archive_fails_verification_and_says_so() {
        let instance = Instance::new("damaged");
        let taken = instance.take(false);
        let mut bytes = fs::read(&taken.path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        fs::write(&taken.path, &bytes).unwrap();
        let checks = verify(&taken.path, "inst-1");
        assert!(checks.iter().any(|c| c.status == CheckStatus::Fail), "{checks:?}");
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
}
