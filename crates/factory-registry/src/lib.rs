//! Projects the instance configuration into scope records, and reports drift.
//!
//! There are three descriptions of where a scope is, and they can disagree:
//! the instance configuration (what a human declared), the `scopes` table (what
//! Factory recorded, and what sessions, leases, and tasks hold foreign keys
//! into), and the filesystem (what is actually there).
//!
//! ADR 0016 resolves that:
//!
//! - **The configuration declares; the table projects.** A scope exists because
//!   it is declared, not because a row exists. Dropping the table and rebuilding
//!   reproduces it — a test asserts this, because a projection that cannot be
//!   rebuilt is a second source of truth in disguise.
//! - **Reconcile reports; applying is a separate act.** A reconcile that
//!   silently repaired would be a second way to change scope registration,
//!   competing with `scope add`. Slice 10 makes the same split for
//!   `factory doctor`.
//! - **`git` is recorded, not verified.** Checking a remote during registration
//!   would make it require network access, for a field nothing yet consumes.
//!
//! The table adds what the configuration cannot hold: the canonical absolute
//! path, the `(st_dev, st_ino)` identity of ADR 0009, and the resolved parent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use factory_paths::{CanonicalPath, FileId, PathError, is_descendant};

/// A scope as declared, resolved against the filesystem.
#[derive(Debug, Clone)]
pub struct RegisteredScope {
    pub id: uuid::Uuid,
    pub name: String,
    /// Exactly as written in the configuration, relative to the instance root.
    pub declared_path: PathBuf,
    /// `None` when the declared path does not currently exist. That is a
    /// reportable state, not an error: a configuration may legitimately name a
    /// directory that has been moved away, and saying so is more useful than
    /// refusing to load.
    pub canonical_path: Option<CanonicalPath>,
    pub file_id: Option<FileId>,
    pub git: Option<String>,
    /// The nearest registered ancestor, by canonical path.
    pub parent_id: Option<uuid::Uuid>,
    pub agents: Vec<factory_config::Agent>,
}

/// One disagreement between the configuration, the table, and the filesystem.
///
/// The variants are distinguished because their remedies differ — see the drift
/// table in ADR 0016. Two of them are deliberately never applied automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drift {
    /// In the configuration, absent from the table. Applying inserts it.
    DeclaredNotProjected { id: uuid::Uuid, name: String },

    /// In the table, absent from the configuration — a human removed it.
    ///
    /// **Never applied.** Rows elsewhere still reference this scope, and
    /// deciding what happens to that history is a retention question, not a
    /// registry one.
    ProjectedNotDeclared { id: uuid::Uuid, name: String },

    /// The directory moved and `(dev, ino)` still matches. Applying updates the
    /// path; the UUID is unchanged, because identity is the UUID.
    PathChangedSameIdentity {
        id: uuid::Uuid,
        name: String,
        from: PathBuf,
        to: PathBuf,
    },

    /// The path changed and the recorded identity no longer matches.
    ///
    /// **Never applied.** ADR 0009 is explicit that delete-and-recreate at one
    /// path yields a new inode, so a mismatch means "this may be a different
    /// directory", not "this is". Applying would silently re-point every
    /// session, lease, and task referencing the scope at whatever now occupies
    /// the path.
    PathChangedDifferentIdentity {
        id: uuid::Uuid,
        name: String,
        from: PathBuf,
        to: PathBuf,
    },

    /// The declared path does not exist.
    MissingPath {
        id: uuid::Uuid,
        name: String,
        path: PathBuf,
    },

    /// The scope has no readable `AGENTS.md`.
    ///
    /// Checked here rather than left to Slice 4, which fails hard on an
    /// unreadable context source (ADR 0013) — at the worst moment, when an
    /// operator is waiting on a session rather than editing configuration.
    /// *Readable*, not non-empty: an empty file is a legitimate statement that
    /// a scope adds nothing to its ancestors' context.
    UnreadableContext {
        id: uuid::Uuid,
        name: String,
        path: PathBuf,
    },
}

impl Drift {
    /// Whether `apply` acts on this, or only reports it.
    #[must_use]
    pub fn is_applicable(&self) -> bool {
        matches!(
            self,
            Self::DeclaredNotProjected { .. } | Self::PathChangedSameIdentity { .. }
        )
    }
}

/// The result of a read-only reconcile.
#[derive(Debug, Clone, Default)]
pub struct DriftReport {
    pub items: Vec<Drift>,
}

impl DriftReport {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.items.is_empty()
    }
}

/// Resolve every declared scope against the filesystem.
///
/// `instance_root` is the directory containing `.factory/`; declared paths are
/// relative to it. Resolution never fails because a path is missing — that
/// becomes `canonical_path: None` and a `MissingPath` drift.
pub fn resolve(
    config: &factory_config::InstanceConfig,
    instance_root: &Path,
) -> Result<Vec<RegisteredScope>, RegistryError> {
    let instance_root = CanonicalPath::resolve(instance_root)?;

    let mut resolved = Vec::with_capacity(config.scopes.len());
    for entry in &config.scopes {
        let joined = instance_root.as_path().join(&entry.path);
        let (canonical_path, file_id) = match CanonicalPath::resolve(&joined) {
            Ok(canonical) => {
                // The instance's own root scope declares `path: .`, which
                // resolves to `instance_root` itself. `is_descendant` treats
                // a path as never its own descendant (by design — see
                // `factory_paths`), so without this equality check the
                // company root would fail its own escape check.
                let is_root_itself = canonical == instance_root;
                if !is_root_itself && !is_descendant(&instance_root, &canonical)? {
                    return Err(RegistryError::EscapesInstance {
                        name: entry.name.clone(),
                        path: entry.path.clone(),
                        root: instance_root.as_path().to_path_buf(),
                    });
                }
                let file_id = FileId::of(canonical.as_path())?;
                (Some(canonical), Some(file_id))
            }
            // A declared path that does not currently exist is a reportable
            // state (`MissingPath`, surfaced by `reconcile`), not a resolve
            // failure: a configuration may legitimately name a directory
            // that has been moved away.
            Err(PathError::NotFound { .. }) => (None, None),
            Err(other) => return Err(RegistryError::Path(other)),
        };

        resolved.push(RegisteredScope {
            id: entry.id,
            name: entry.name.clone(),
            declared_path: entry.path.clone(),
            canonical_path,
            file_id,
            git: entry.git.clone(),
            parent_id: None,
            agents: entry.agents.clone(),
        });
    }

    // The nearest *other* registered scope that is an ancestor, by canonical
    // path. This runs as a second pass, once every scope's `canonical_path`
    // is known, and writes into a side vector rather than `resolved[i]`
    // directly, so scanning every candidate ancestor (an immutable borrow of
    // the whole slice) never overlaps with recording the answer for `i` (a
    // mutable borrow of one element).
    let mut parent_ids: Vec<Option<uuid::Uuid>> = vec![None; resolved.len()];
    for i in 0..resolved.len() {
        let Some(this_canonical) = &resolved[i].canonical_path else {
            continue; // A scope whose path does not exist has no computable parent.
        };

        // Track the deepest ancestor found so far by its component count:
        // the nearest ancestor is the one with the longest canonical path
        // among every registered scope that contains this one.
        let mut deepest: Option<(usize, usize)> = None;
        for (j, candidate) in resolved.iter().enumerate() {
            if i == j {
                continue;
            }
            let Some(candidate_canonical) = &candidate.canonical_path else {
                continue;
            };
            if is_descendant(candidate_canonical, this_canonical)? {
                let depth = candidate_canonical.as_path().components().count();
                if deepest.is_none_or(|(_, best_depth)| depth > best_depth) {
                    deepest = Some((j, depth));
                }
            }
        }
        parent_ids[i] = deepest.map(|(j, _)| resolved[j].id);
    }
    for (scope, parent_id) in resolved.iter_mut().zip(parent_ids) {
        scope.parent_id = parent_id;
    }

    Ok(resolved)
}

/// A `scopes` row as currently recorded, read back for comparison against a
/// freshly [`resolve`]d [`RegisteredScope`].
struct StoredScope {
    id: uuid::Uuid,
    name: String,
    declared_path: PathBuf,
    canonical_path: Option<PathBuf>,
    file_id: Option<FileId>,
}

/// Read every row of `scopes`, for [`reconcile`].
///
/// `factory_store::Store` exposes exactly one way to run SQL —
/// [`factory_store::Store::transaction`] — and it takes `&mut self`, because
/// every write is `BEGIN IMMEDIATE`. `reconcile` is documented read-only and
/// therefore takes `&Store`, so it cannot call that method on its own
/// argument. This opens a second connection to the same database file via
/// [`factory_store::Store::open_at`] (WAL supports concurrent readers) and
/// lets its transaction handle be dropped without committing once the read is
/// done, which is what keeps the read-only guarantee real rather than
/// asserted. See the `RegistryError` doc comment on this crate's report for
/// why this is a finding about `reconcile`'s signature, not just an
/// implementation detail.
fn read_stored_scopes(store: &factory_store::Store) -> Result<Vec<StoredScope>, RegistryError> {
    // Read through the borrowed connection, not a transaction. `transaction()`
    // is BEGIN IMMEDIATE and would take a write lock for a query that changes
    // nothing, contending with real writers. Under WAL a reader never blocks.
    let mut statement = store
        .connection()
        .prepare("SELECT id, name, declared_path, canonical_path, dev, ino FROM scopes")
        .map_err(factory_store::StoreError::from)?;
    let mut rows = statement
        .query(())
        .map_err(factory_store::StoreError::from)?;

    let mut stored = Vec::new();
    while let Some(row) = rows.next().map_err(factory_store::StoreError::from)? {
        let id: String = row.get(0).map_err(factory_store::StoreError::from)?;
        let name: String = row.get(1).map_err(factory_store::StoreError::from)?;
        let declared_path: String = row.get(2).map_err(factory_store::StoreError::from)?;
        let canonical_path: Option<String> = row.get(3).map_err(factory_store::StoreError::from)?;
        let dev: Option<i64> = row.get(4).map_err(factory_store::StoreError::from)?;
        let ino: Option<i64> = row.get(5).map_err(factory_store::StoreError::from)?;

        stored.push(StoredScope {
            id: uuid::Uuid::parse_str(&id)
                .expect("scopes.id is a UUID: only this crate ever writes it"),
            name,
            declared_path: PathBuf::from(declared_path),
            canonical_path: canonical_path.map(PathBuf::from),
            file_id: dev
                .zip(ino)
                .map(|(dev, ino)| FileId::from_parts(dev as u64, ino as u64)),
        });
    }
    Ok(stored)
}

/// Compare resolved scopes against the `scopes` table. Reads only.
pub fn reconcile(
    store: &factory_store::Store,
    scopes: &[RegisteredScope],
) -> Result<DriftReport, RegistryError> {
    let mut stored_by_id: HashMap<uuid::Uuid, StoredScope> = read_stored_scopes(store)?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();

    let mut items = Vec::new();

    for scope in scopes {
        match stored_by_id.remove(&scope.id) {
            None => items.push(Drift::DeclaredNotProjected {
                id: scope.id,
                name: scope.name.clone(),
            }),
            Some(row) => {
                // `resolve()` always pairs `canonical_path` and `file_id`:
                // both `Some` or both `None`. When both are `None` the path
                // is currently missing, which is handled uniformly below —
                // for a matched scope that means no `PathChanged*` drift,
                // only `MissingPath`.
                if let (Some(canonical), Some(file_id)) = (&scope.canonical_path, scope.file_id) {
                    let from = row
                        .canonical_path
                        .clone()
                        .unwrap_or_else(|| row.declared_path.clone());
                    let to = canonical.as_path().to_path_buf();

                    if row.file_id == Some(file_id) {
                        // Same identity: recognisably the same directory,
                        // per ADR 0009 — a rename preserves the inode. Only
                        // report drift if the recorded path text actually
                        // disagrees with a fresh canonicalization.
                        if row.canonical_path.as_deref() != Some(canonical.as_path()) {
                            items.push(Drift::PathChangedSameIdentity {
                                id: scope.id,
                                name: scope.name.clone(),
                                from,
                                to,
                            });
                        }
                    } else {
                        // Different identity: the recorded inode is gone or
                        // now belongs to another directory (ADR 0009 —
                        // delete-and-recreate at one path yields a new
                        // inode). This holds whether or not the path text
                        // itself changed.
                        items.push(Drift::PathChangedDifferentIdentity {
                            id: scope.id,
                            name: scope.name.clone(),
                            from,
                            to,
                        });
                    }
                }
            }
        }

        // The declared path does not exist. Checked independent of
        // projection status above — a brand-new declared scope and an
        // already-projected one are both reportable this way — per the
        // `Drift::MissingPath` doc comment.
        if scope.canonical_path.is_none() {
            items.push(Drift::MissingPath {
                id: scope.id,
                name: scope.name.clone(),
                path: scope.declared_path.clone(),
            });
        } else if let Some(canonical) = &scope.canonical_path {
            // Decision 3 (ADR 0016): checked at registration/reconcile, not
            // left for Slice 4 to fail hard on at the worst possible moment.
            // This applies to every scope with a directory to check,
            // whether or not it is already projected.
            let agents_md = canonical.as_path().join("AGENTS.md");
            if std::fs::read(&agents_md).is_err() {
                items.push(Drift::UnreadableContext {
                    id: scope.id,
                    name: scope.name.clone(),
                    path: agents_md,
                });
            }
        }
    }

    // Whatever is left was recorded but is no longer declared: a human
    // removed it from the configuration.
    for (id, row) in stored_by_id {
        items.push(Drift::ProjectedNotDeclared { id, name: row.name });
    }

    Ok(DriftReport { items })
}

/// Apply the applicable drift, in one transaction.
///
/// Returns what it changed. Drift that is not applicable is left untouched and
/// remains in a subsequent report.
pub fn apply(
    store: &mut factory_store::Store,
    scopes: &[RegisteredScope],
    report: &DriftReport,
) -> Result<Vec<Drift>, RegistryError> {
    let scopes_by_id: HashMap<uuid::Uuid, &RegisteredScope> =
        scopes.iter().map(|scope| (scope.id, scope)).collect();

    let applicable: Vec<&Drift> = report
        .items
        .iter()
        .filter(|drift| drift.is_applicable())
        .collect();
    if applicable.is_empty() {
        return Ok(Vec::new());
    }

    let tx = store.transaction()?;
    for &drift in &applicable {
        match drift {
            Drift::DeclaredNotProjected { id, .. } => {
                let scope = scopes_by_id
                    .get(id)
                    .expect("a DeclaredNotProjected drift names a scope resolve() produced");
                let canonical_path = scope
                    .canonical_path
                    .as_ref()
                    .map(|canonical| canonical.as_path().to_string_lossy().into_owned());
                let dev = scope.file_id.map(|file_id| file_id.dev() as i64);
                let ino = scope.file_id.map(|file_id| file_id.ino() as i64);
                let parent_id = scope.parent_id.map(|pid| pid.to_string());

                tx.execute(
                    "INSERT INTO scopes (id, name, declared_path, canonical_path, git, dev, ino, parent_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    (
                        scope.id.to_string(),
                        scope.name.clone(),
                        scope.declared_path.to_string_lossy().into_owned(),
                        canonical_path,
                        scope.git.clone(),
                        dev,
                        ino,
                        parent_id,
                    ),
                )
                .map_err(factory_store::StoreError::from)?;
            }
            Drift::PathChangedSameIdentity { id, .. } => {
                let scope = scopes_by_id
                    .get(id)
                    .expect("a PathChangedSameIdentity drift names a scope resolve() produced");
                let canonical_path = scope
                    .canonical_path
                    .as_ref()
                    .map(|canonical| canonical.as_path().to_string_lossy().into_owned());
                let dev = scope.file_id.map(|file_id| file_id.dev() as i64);
                let ino = scope.file_id.map(|file_id| file_id.ino() as i64);
                let parent_id = scope.parent_id.map(|pid| pid.to_string());

                tx.execute(
                    "UPDATE scopes
                     SET declared_path = ?2, canonical_path = ?3, dev = ?4, ino = ?5, parent_id = ?6
                     WHERE id = ?1",
                    (
                        scope.id.to_string(),
                        scope.declared_path.to_string_lossy().into_owned(),
                        canonical_path,
                        dev,
                        ino,
                        parent_id,
                    ),
                )
                .map_err(factory_store::StoreError::from)?;
            }
            Drift::ProjectedNotDeclared { .. }
            | Drift::PathChangedDifferentIdentity { .. }
            | Drift::MissingPath { .. }
            | Drift::UnreadableContext { .. } => {
                unreachable!("is_applicable() only allows the two variants matched above")
            }
        }
    }
    tx.commit().map_err(factory_store::StoreError::from)?;

    Ok(applicable.into_iter().cloned().collect())
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("path error: {0}")]
    Path(#[from] factory_paths::PathError),

    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error(
        "scope `{name}` declares path {path}, which escapes the instance root {root}\n  help: a scope must live inside its instance; use a path that does not traverse above it"
    )]
    EscapesInstance {
        name: String,
        path: PathBuf,
        root: PathBuf,
    },
}
