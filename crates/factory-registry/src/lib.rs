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

use std::collections::{HashMap, HashSet};
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
/// table in ADR 0016. Four of them are deliberately never applied
/// automatically: `ProjectedNotDeclared`, `PathChangedDifferentIdentity`,
/// `MissingPath`, and `UnreadableContext`. The rest — including every
/// variant added after Slice 3 shipped — carry no identity ambiguity of
/// their own and are applied.
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

    /// The configuration renamed the scope; the table still has the old name.
    ///
    /// Applying updates it. `name` carries no identity — the UUID (`id`) does
    /// — so unlike a path there is no "this might now be a different scope"
    /// question to ask before writing the new value. The rejected
    /// alternative is treating a rename as cosmetic and leaving it
    /// unreported: `reconcile` would say "clean" while a human reading the
    /// `name` column saw something the configuration no longer says, which is
    /// exactly the silent disagreement this crate exists to surface.
    NameChanged {
        id: uuid::Uuid,
        from: String,
        to: String,
    },

    /// The configuration's `git` reference — added, changed, or removed —
    /// disagrees with what the table has recorded.
    ///
    /// Applying updates it. ADR 0016 decision 4 is explicit that `git` is
    /// *recorded, not verified*: Factory never contacts a remote to check it,
    /// because that would make registration require network access for a
    /// field nothing yet consumes. But "recorded" is a promise that the
    /// stored value tracks what was declared, not that it is frozen at
    /// whatever was declared on first registration. Like `name`, `git`
    /// carries no identity, so applying it automatically raises none of the
    /// "is this still the same scope" question that makes
    /// `PathChangedDifferentIdentity` unsafe to apply.
    GitChanged {
        id: uuid::Uuid,
        name: String,
        from: Option<String>,
        to: Option<String>,
    },

    /// The declared path's *text* changed in the configuration while it
    /// still resolves to the same directory: `(dev, ino)` unchanged, and a
    /// fresh canonicalization is unchanged too.
    ///
    /// Never reported alongside `PathChangedSameIdentity` or
    /// `PathChangedDifferentIdentity` for the same scope: those already
    /// cover, and their `apply` (in the same-identity case) already
    /// rewrites, `declared_path` for every case where the canonicalization
    /// itself moved. This variant exists only for the narrower case they
    /// cannot see — two spellings of the configuration that resolve to one
    /// place, such as a symlink alias inside the instance root, or a
    /// relative path rewritten to an equivalent form (`child` to
    /// `./child`). Skipping this check whenever a `PathChanged*` already
    /// fired is not mere de-duplication: `PathChangedDifferentIdentity` is
    /// deliberately never applied because the row may now name a different
    /// directory — `a_replaced_directory_is_not_recognised_as_the_same_scope`
    /// asserts no *applicable* drift exists for such a scope in that
    /// scenario — and without the guard this variant, which *is* applicable,
    /// would write `declared_path` on that same identity-uncertain row.
    /// (`NameChanged` and `GitChanged` carry no such guard and stay
    /// applicable even then, but that is not a gap in this reasoning: unlike
    /// `declared_path`, neither column is something a session, lease, or
    /// task ever resolves through, so writing them raises no "did this just
    /// re-point something live" question regardless of identity certainty.)
    ///
    /// Reported only while the scope currently resolves (`canonical_path` is
    /// not `None`). A scope with a currently-missing path is `MissingPath`'s
    /// concern, and `MissingPath` is never applied — there is no silent
    /// disagreement to close by also comparing declared-path text there,
    /// since reconcile can already never report "clean" for that scope.
    ///
    /// Applying updates the stored text. This raises no identity question
    /// either: the directory identified by `(dev, ino)` has not changed, only
    /// how the configuration spells its path, so it is safe by the same
    /// reasoning as `NameChanged`.
    DeclaredPathChanged {
        id: uuid::Uuid,
        name: String,
        from: PathBuf,
        to: PathBuf,
    },

    /// The nearest registered ancestor recomputed differently: this scope's
    /// own path did not move, but *another* scope's declaration did — a new
    /// ancestor was declared over it, an existing one was removed from the
    /// configuration, or an ancestor's own path changed — so [`resolve`]'s
    /// parent computation now names a different scope, or none.
    ///
    /// Reported only while the scope currently resolves, and — for the same
    /// reason as `DeclaredPathChanged` — only when no `PathChanged*` drift
    /// already fired for it: when this scope's own canonicalization moved,
    /// `PathChangedSameIdentity`'s `apply` already rewrites `parent_id`
    /// alongside `declared_path` and `canonical_path` in one statement, so a
    /// second report of the same underlying event would be noise, not new
    /// information.
    ///
    /// Applying updates it. Unlike a path, `parent_id` is not a value a human
    /// writes in configuration at all — it is Factory's own computed value,
    /// derived entirely from the already-identity-checked canonical paths of
    /// other registered scopes. There is nothing here for
    /// `PathChangedDifferentIdentity`'s caution to apply to: recomputing it
    /// is re-running arithmetic Factory already trusts over already-verified
    /// inputs, not re-pointing a foreign key at an unverified directory.
    ParentChanged {
        id: uuid::Uuid,
        name: String,
        from: Option<uuid::Uuid>,
        to: Option<uuid::Uuid>,
    },
}

impl Drift {
    /// Whether `apply` acts on this, or only reports it.
    #[must_use]
    pub fn is_applicable(&self) -> bool {
        matches!(
            self,
            Self::DeclaredNotProjected { .. }
                | Self::PathChangedSameIdentity { .. }
                | Self::NameChanged { .. }
                | Self::GitChanged { .. }
                | Self::DeclaredPathChanged { .. }
                | Self::ParentChanged { .. }
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
    git: Option<String>,
    file_id: Option<FileId>,
    parent_id: Option<uuid::Uuid>,
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
        .prepare(
            "SELECT id, name, declared_path, canonical_path, git, dev, ino, parent_id \
             FROM scopes",
        )
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
        let git: Option<String> = row.get(4).map_err(factory_store::StoreError::from)?;
        let dev: Option<i64> = row.get(5).map_err(factory_store::StoreError::from)?;
        let ino: Option<i64> = row.get(6).map_err(factory_store::StoreError::from)?;
        let parent_id: Option<String> = row.get(7).map_err(factory_store::StoreError::from)?;

        stored.push(StoredScope {
            id: uuid::Uuid::parse_str(&id)
                .expect("scopes.id is a UUID: only this crate ever writes it"),
            name,
            declared_path: PathBuf::from(declared_path),
            canonical_path: canonical_path.map(PathBuf::from),
            git,
            file_id: dev
                .zip(ino)
                .map(|(dev, ino)| FileId::from_parts(dev as u64, ino as u64)),
            parent_id: parent_id.map(|id| {
                uuid::Uuid::parse_str(&id)
                    .expect("scopes.parent_id is a UUID: only this crate ever writes it")
            }),
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
                // Set once a `PathChanged*` drift is reported for this scope,
                // so `DeclaredPathChanged` and `ParentChanged` below can skip
                // themselves. Both of those are applicable, and
                // `PathChangedDifferentIdentity` is deliberately never
                // applied because the row may now name a different directory
                // entirely (ADR 0009) — `apply` must not write to that row
                // regardless of the drift's name. Gating on "did a
                // `PathChanged*` already fire" rather than re-deriving the
                // identity question here keeps that a single decision made
                // in one place.
                let mut path_drift_emitted = false;

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
                            path_drift_emitted = true;
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
                        path_drift_emitted = true;
                    }
                }

                // `name` and `git` are declarative fields with no identity of
                // their own — the UUID is identity — so, unlike a path, there
                // is no "is this still the same scope" question to ask before
                // reporting (and, per `is_applicable`, applying) a change.
                // Checked regardless of whether the scope currently resolves:
                // a rename or a `git` edit is real drift even for a scope
                // whose directory has also gone missing.
                if row.name != scope.name {
                    items.push(Drift::NameChanged {
                        id: scope.id,
                        from: row.name.clone(),
                        to: scope.name.clone(),
                    });
                }
                if row.git != scope.git {
                    items.push(Drift::GitChanged {
                        id: scope.id,
                        name: scope.name.clone(),
                        from: row.git.clone(),
                        to: scope.git.clone(),
                    });
                }

                // `declared_path` and `parent_id` are checked only while the
                // scope currently resolves and only when no `PathChanged*`
                // already fired — see both variants' doc comments for why:
                // in short, a currently-missing path is `MissingPath`'s
                // concern (never applied, so already never "clean"), and a
                // `PathChanged*` already reports — and, in the
                // same-identity case, already applies — the same underlying
                // event.
                if scope.canonical_path.is_some() && !path_drift_emitted {
                    if row.declared_path != scope.declared_path {
                        items.push(Drift::DeclaredPathChanged {
                            id: scope.id,
                            name: scope.name.clone(),
                            from: row.declared_path.clone(),
                            to: scope.declared_path.clone(),
                        });
                    }
                    if row.parent_id != scope.parent_id {
                        items.push(Drift::ParentChanged {
                            id: scope.id,
                            name: scope.name.clone(),
                            from: row.parent_id,
                            to: scope.parent_id,
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

/// The scope a `Drift` names, regardless of variant.
///
/// Every variant carries `id` as its first field — this is the one place
/// that leans on that uniformity, so [`apply`] can order statements by the
/// scope each one writes without a second `match` per call site.
fn drift_scope_id(drift: &Drift) -> uuid::Uuid {
    match drift {
        Drift::DeclaredNotProjected { id, .. }
        | Drift::ProjectedNotDeclared { id, .. }
        | Drift::PathChangedSameIdentity { id, .. }
        | Drift::PathChangedDifferentIdentity { id, .. }
        | Drift::MissingPath { id, .. }
        | Drift::UnreadableContext { id, .. }
        | Drift::NameChanged { id, .. }
        | Drift::GitChanged { id, .. }
        | Drift::DeclaredPathChanged { id, .. }
        | Drift::ParentChanged { id, .. } => *id,
    }
}

/// How many path components deep `scope`'s canonical path is — a proxy for
/// its depth in the registered-scope tree, used by [`apply`] to write a
/// parent's row before any child that names it.
///
/// This is sound because of how [`resolve`] computes `parent_id`, not by
/// construction here: a registered parent is the *deepest* other scope whose
/// canonical path is an ancestor of this one's (`resolve`'s second pass), and
/// [`is_descendant`] treats a path as never its own descendant — it skips the
/// candidate itself before walking `Path::ancestors()`, so an ancestor's path
/// is always a strictly shorter prefix, never equal. A parent's component
/// count is therefore always strictly less than its child's, for every
/// parent/child pair `resolve` can produce.
///
/// A scope with no `canonical_path` (its declared directory does not
/// currently exist) gets depth `0`. That is never wrong, on both sides of the
/// relation `apply` cares about:
/// - As a *parent*: `resolve`'s candidate loop only considers scopes with a
///   `canonical_path` when computing *other* scopes' `parent_id`, so an
///   unresolved scope can never be named as anyone's parent — nothing ever
///   needs its row to exist first.
/// - As the *subject* of a drift: `resolve` leaves `parent_id: None` on a
///   scope whose own `canonical_path` is `None` (it has no ancestor to
///   compute one against), so the one applicable drift that can name such a
///   scope — `DeclaredNotProjected` — writes `parent_id = NULL`. A NULL
///   foreign key has nothing to check, so this scope's own sort position
///   carries no FK dependency to get right, and depth `0` is as good as any
///   other value.
fn nesting_depth(scope: &RegisteredScope) -> usize {
    scope
        .canonical_path
        .as_ref()
        .map_or(0, |canonical| canonical.as_path().components().count())
}

/// Apply the applicable drift, in one transaction.
///
/// Returns what it changed, ordered by the same parent-before-child pass
/// described below — not by `report.items` order, which is declaration order
/// (see [`reconcile`]) and is exactly the order that is unsafe to execute in.
/// Drift that is not applicable is left untouched and remains in a
/// subsequent report.
///
/// **Ordering guarantee:** `scopes.parent_id` is `REFERENCES scopes (id)`
/// with `PRAGMA foreign_keys = ON` at runtime (`factory_store::pragma`),
/// enforced immediately rather than deferred to commit. Two applicable
/// variants write `parent_id` — `DeclaredNotProjected`'s `INSERT` and
/// `ParentChanged`'s `UPDATE` — so a naive `report.items`-order execution can
/// write a scope's row before the row it names as parent, failing that
/// statement's FK check and rolling back the whole transaction (confirmed by
/// `a_child_declared_above_its_new_parent_still_projects_both` and
/// `a_parent_changed_update_above_its_new_parents_insert_still_projects_both`
/// in `tests/registry.rs`, both against the unfixed code first). Before
/// executing anything, `applicable` is stably sorted by [`nesting_depth`] of
/// the scope each drift names, ascending, so every statement that writes a
/// child's `parent_id` runs after the statement that put its parent's row in
/// place.
///
/// A **topological sort over explicit parent edges** — Kahn's algorithm, with
/// cycle detection — was rejected in favor of the depth sort above. Scope
/// parentage is not an arbitrary graph a human declares: `resolve` derives
/// `parent_id` entirely from filesystem nesting (`is_descendant` over
/// canonical paths), which makes the parent relation a forest by
/// construction. A cycle would require some scope's canonical path to be a
/// strict ancestor of itself, which `is_descendant`'s own skip-the-candidate
/// rule makes unrepresentable — so cycle-handling code here would defend
/// against a case `resolve` cannot produce, and a depth comparison is both
/// sufficient and simpler than reconstructing the tree just to sort it.
///
/// A parent may be entirely absent from `applicable` — already projected,
/// with nothing about it currently drifted — and that needs no special
/// handling either: the sort only orders *writes*, and an already-projected
/// parent's row is already in place before this transaction starts, same as
/// if it had a row from a previous `apply` call. The FK is satisfied by the
/// row existing, not by that row's drift (if any) having just been applied.
pub fn apply(
    store: &mut factory_store::Store,
    scopes: &[RegisteredScope],
    report: &DriftReport,
) -> Result<Vec<Drift>, RegistryError> {
    let scopes_by_id: HashMap<uuid::Uuid, &RegisteredScope> =
        scopes.iter().map(|scope| (scope.id, scope)).collect();

    let mut applicable: Vec<&Drift> = report
        .items
        .iter()
        .filter(|drift| drift.is_applicable())
        .collect();
    if applicable.is_empty() {
        return Ok(Vec::new());
    }

    // Stable: two drifts at the same depth (siblings, or unrelated scopes
    // whose applied columns carry no `parent_id` dependency at all) keep
    // `report.items`' relative order — the only order available to choose
    // between them, since depth alone does not — so execution order, and the
    // `Vec` this function returns, stay a deterministic function of `report`
    // rather than an accident of the sort's internals. (This is not what
    // keeps `apply_rolls_back_completely_on_a_midway_failure` passing: both
    // of its drifts name the same scope id, so both land at the same depth
    // regardless of stability, and any comparison sort keeps a two-element
    // equal-key slice in place.)
    applicable.sort_by_key(|drift| {
        let scope = scopes_by_id
            .get(&drift_scope_id(drift))
            .expect("an applicable drift names a scope resolve() produced");
        nesting_depth(scope)
    });

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
            Drift::NameChanged { id, .. } => {
                let scope = scopes_by_id
                    .get(id)
                    .expect("a NameChanged drift names a scope resolve() produced");
                tx.execute(
                    "UPDATE scopes SET name = ?2 WHERE id = ?1",
                    (scope.id.to_string(), scope.name.clone()),
                )
                .map_err(factory_store::StoreError::from)?;
            }
            Drift::GitChanged { id, .. } => {
                let scope = scopes_by_id
                    .get(id)
                    .expect("a GitChanged drift names a scope resolve() produced");
                tx.execute(
                    "UPDATE scopes SET git = ?2 WHERE id = ?1",
                    (scope.id.to_string(), scope.git.clone()),
                )
                .map_err(factory_store::StoreError::from)?;
            }
            Drift::DeclaredPathChanged { id, .. } => {
                let scope = scopes_by_id
                    .get(id)
                    .expect("a DeclaredPathChanged drift names a scope resolve() produced");
                tx.execute(
                    "UPDATE scopes SET declared_path = ?2 WHERE id = ?1",
                    (
                        scope.id.to_string(),
                        scope.declared_path.to_string_lossy().into_owned(),
                    ),
                )
                .map_err(factory_store::StoreError::from)?;
            }
            Drift::ParentChanged { id, .. } => {
                let scope = scopes_by_id
                    .get(id)
                    .expect("a ParentChanged drift names a scope resolve() produced");
                let parent_id = scope.parent_id.map(|pid| pid.to_string());
                tx.execute(
                    "UPDATE scopes SET parent_id = ?2 WHERE id = ?1",
                    (scope.id.to_string(), parent_id),
                )
                .map_err(factory_store::StoreError::from)?;
            }
            Drift::ProjectedNotDeclared { .. }
            | Drift::PathChangedDifferentIdentity { .. }
            | Drift::MissingPath { .. }
            | Drift::UnreadableContext { .. } => {
                unreachable!("is_applicable() only allows the variants matched above")
            }
        }
    }
    tx.commit().map_err(factory_store::StoreError::from)?;

    Ok(applicable.into_iter().cloned().collect())
}

// ---------------------------------------------------------------------
// Kinship — the delegation trust rule's ancestry primitive.
// ---------------------------------------------------------------------

/// How `target_scope_id` stands relative to `sender_scope_id` — named from
/// the target's point of view.
///
/// This is the ancestry check design §6's delegation rule needs: "An agent
/// may target a registered descendant or sibling scope, never an ancestor
/// and never itself." Backlog §8 sharpens the boundary further — "a scope
/// that is neither a descendant nor a sibling... [a] nephew or cousin
/// included, which must be reached through its parent" — which is why this
/// has five variants instead of collapsing "shares some ancestor" into one:
/// a cousin and a sibling both have *an* ancestor in common, but only one of
/// them is targetable. Nothing here decides policy; a caller enforcing the
/// rule matches on the variant, this only answers "what is this pair,
/// structurally."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kinship {
    SameScope,
    /// The target is an ancestor of the sender, at any depth.
    Ancestor,
    /// The target is a descendant of the sender, at any depth.
    Descendant,
    /// The target and the sender share the same non-NULL parent.
    Sibling,
    Unrelated,
}

/// `scopes.parent_id` for `id`, or a typed error when `id` names no row.
///
/// This is [`kinship`]'s existence check, not just a column read: an id
/// that names no row must surface as [`RegistryError::UnknownScope`], never
/// as a silently-computed `Unrelated` — a caller cannot otherwise tell
/// "these two scopes are unrelated" from "I passed a made-up id," and the
/// delegation rule this feeds (design §6) needs to reject the second case
/// outright rather than fail open into the first.
fn scope_parent(
    conn: &rusqlite::Connection,
    id: uuid::Uuid,
) -> Result<Option<uuid::Uuid>, RegistryError> {
    let parent: Option<String> = conn
        .query_row(
            "SELECT parent_id FROM scopes WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => RegistryError::UnknownScope { id },
            other => RegistryError::Store(factory_store::StoreError::from(other)),
        })?;

    Ok(parent.map(|parent| {
        uuid::Uuid::parse_str(&parent)
            .expect("scopes.parent_id is a UUID: only this crate ever writes it")
    }))
}

/// Whether `candidate` appears anywhere in the parent chain above `start`
/// (`start` itself is never considered a match).
///
/// Carries its own visited set rather than trusting the walk to terminate
/// at a `NULL` `parent_id`. [`nesting_depth`]'s doc comment explains why
/// [`resolve`]'s *own* output can never cycle — a registered parent is
/// always a strictly shorter canonical path than its child, so
/// `A.parent = B, B.parent = A` is unrepresentable by construction there.
/// But this function reads `scopes.parent_id` back from the database, not
/// `resolve`'s output, and nothing in the schema stops a hand-edited row
/// from writing exactly that cycle — `parent_id` is a plain self-referencing
/// foreign key with no acyclicity constraint SQLite can express. Without the
/// `visited.insert` check below, such a row would make this loop spin
/// forever; with it, [`kinship`] returns
/// [`RegistryError::CyclicParentage`] instead of hanging.
fn is_ancestor(
    conn: &rusqlite::Connection,
    candidate: uuid::Uuid,
    start: uuid::Uuid,
) -> Result<bool, RegistryError> {
    let mut current = start;
    let mut visited = HashSet::from([current]);
    loop {
        let Some(parent) = scope_parent(conn, current)? else {
            return Ok(false); // Reached a root without finding `candidate`.
        };
        if parent == candidate {
            return Ok(true);
        }
        if !visited.insert(parent) {
            return Err(RegistryError::CyclicParentage { id: parent });
        }
        current = parent;
    }
}

/// How `target_scope_id` stands relative to `sender_scope_id`, per
/// [`Kinship`].
///
/// Takes `&rusqlite::Connection`, not `&Store` or `&Transaction`, so a
/// caller can pass either `store.connection()` (a read, per
/// [`Store::connection`](factory_store::Store::connection)) or a live
/// `Transaction` — it derefs to `Connection`, so no adapter is needed to
/// check kinship as one step inside a larger write.
///
/// Four things this deliberately gets right, each covered by its own test
/// in `tests/registry.rs`:
///
/// 1. **Descendant is any depth; sibling is exactly one shared parent.**
///    Flattening both into "shares any ancestor" would wrongly admit a
///    cousin as kin — backlog §8 names cousin and nephew explicitly as
///    *rejected*, reachable only through their parent.
/// 2. **Two root scopes are not siblings.** Both have `parent_id IS NULL`,
///    and `Option<Uuid> == Option<Uuid>` would say `None == None` and call
///    that a shared parent — wrong, since a `NULL` parent is the *absence*
///    of one, not a value two roots can share. The `Some(..) == Some(..)`
///    match below (not a bare `sender_parent == target_parent`) is what
///    makes that explicit rather than accidental.
/// 3. **The walk cannot hang on a hand-edited cycle** — see
///    [`is_ancestor`]'s doc comment.
/// 4. **An unknown scope id is a typed error**, not a silent `Unrelated` —
///    see [`scope_parent`].
///
/// A caller that wants *only* the existence check in point 4 — with no
/// interest in how the two ids relate — should call [`require_registered`]
/// instead of encoding "does this exist" as `kinship(conn, id, id)`. Both
/// existence checks below still run unconditionally, before the
/// `SameScope` short-circuit, so `kinship(conn, id, id)` remains correct for
/// an unregistered `id` — but a reader of a call site should not have to
/// know that to see what the call means.
pub fn kinship(
    conn: &rusqlite::Connection,
    sender_scope_id: uuid::Uuid,
    target_scope_id: uuid::Uuid,
) -> Result<Kinship, RegistryError> {
    // Both existence checks run unconditionally, before the `SameScope`
    // short-circuit below, so an unknown id is always a typed error — even
    // one compared against itself — rather than an error that only surfaces
    // for some argument orderings.
    let sender_parent = scope_parent(conn, sender_scope_id)?;
    let target_parent = scope_parent(conn, target_scope_id)?;

    if sender_scope_id == target_scope_id {
        return Ok(Kinship::SameScope);
    }

    if is_ancestor(conn, target_scope_id, sender_scope_id)? {
        return Ok(Kinship::Ancestor);
    }
    if is_ancestor(conn, sender_scope_id, target_scope_id)? {
        return Ok(Kinship::Descendant);
    }

    // Point 2 above: only a `Some(..) == Some(..)` match counts as a shared
    // parent. Two `None`s falls through to `Unrelated` below.
    if let (Some(sender_parent), Some(target_parent)) = (sender_parent, target_parent) {
        if sender_parent == target_parent {
            return Ok(Kinship::Sibling);
        }
    }

    Ok(Kinship::Unrelated)
}

/// Refuse an id that names no registered scope.
///
/// Existence is a narrower question than [`kinship`] answers, and it earns
/// its own name: a caller that wants only "does this scope exist" and
/// expresses that as `kinship(conn, id, id)` has to decode a trick to get a
/// plain existence check, and is now coupled to `kinship`'s internal
/// ordering — specifically, to its existence checks running *before* the
/// `SameScope` short-circuit — as if that ordering were part of its public
/// contract rather than an implementation detail one caller once relied on.
/// `require_registered` is what that caller should have named instead. It
/// is built on the same [`scope_parent`] lookup `kinship` itself uses for
/// exactly this check, so there remains exactly one place that knows how
/// `scopes` answers "is this id registered" — this function just gives that
/// answer its own name and return type instead of asking a question that
/// happens to also answer it.
pub fn require_registered(
    conn: &rusqlite::Connection,
    id: uuid::Uuid,
) -> Result<(), RegistryError> {
    scope_parent(conn, id)?;
    Ok(())
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

    /// [`kinship`] was asked about an id absent from `scopes`. Never
    /// returned for a *stale but once-valid* id — this crate never deletes
    /// rows — only for one that was never registered at all.
    #[error("scope `{id}` is not registered")]
    UnknownScope { id: uuid::Uuid },

    /// [`kinship`]'s walk revisited a scope id already seen in the current
    /// parent-chain traversal. [`resolve`] cannot produce this (see
    /// [`nesting_depth`]'s doc comment) — it means some `scopes.parent_id`
    /// value was written by a hand edit, not by this crate.
    #[error(
        "scope `{id}`'s parent_id cycles back to a scope already seen while walking its ancestry — parent_id was hand-edited; resolve() cannot produce a cycle"
    )]
    CyclicParentage { id: uuid::Uuid },
}
