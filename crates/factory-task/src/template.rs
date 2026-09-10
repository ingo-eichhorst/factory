//! Task templates — durable intent, kept apart from the runs that execute it
//! (design §11, §12.3; ADR 0021 decision 1).
//!
//! A template is a `task_templates` row. `crate::create::create_from_template`
//! is where a template turns into a run — the crate's own station-11 docs,
//! decision 1: "[a] template sits above a run, in [`template`]; a run is
//! what this crate already manages." Nothing in this module writes to
//! `tasks`, and nothing in `create` writes to `task_templates`; the two
//! tables have exactly one writer each.
//!
//! # `version` is read here, frozen in `create`
//!
//! §12.3's hook. [`revise`] is the only function in this crate that changes
//! it, and it only ever adds one. What happens to the number once a run has
//! captured it is `create::create_from_template`'s doc comment to explain,
//! not this module's — this module never touches `tasks.template_version`
//! and has no opinion on it.

use rusqlite::OptionalExtension;

/// Everything that can go wrong creating, revising, or reading back a task
/// template — in the style of [`crate::TaskError`], but scoped to this
/// module's own table: `lib.rs` is not this slice's file to add a variant
/// to, so this is a second, sibling error enum rather than a case squeezed
/// into the first.
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("store error: {0}")]
    Store(#[from] factory_store::StoreError),

    #[error("no task template with id {0}")]
    NotFound(uuid::Uuid),

    #[error("no task template named {0:?}")]
    NoSuchName(String),

    #[error(
        "a task template named {0:?} already exists\n  help: `task_templates.name` is unique across the instance, not per scope — a schedule names one template by name, and two templates sharing a name would make that reference ambiguous. Choose a different name, or revise the existing template instead of creating a new one"
    )]
    NameTaken(String),
}

/// `task_templates.state`'s exact vocabulary, matching the CHECK constraint
/// in `factory_store::schema` byte for byte — the same shape as
/// [`crate::TaskStatus`], this module's nearest neighbour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateState {
    Open,
    Paused,
    Closed,
}

impl TemplateState {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Paused => "paused",
            Self::Closed => "closed",
        }
    }

    /// `task_templates.state` is constrained by CHECK to exactly these three
    /// strings, so an unrecognised value read back from a row this module
    /// itself just selected is a broken invariant, not an input to handle —
    /// the same stance `TaskStatus::from_db_str` takes, for the same reason.
    fn from_db_str(s: &str) -> Self {
        match s {
            "open" => Self::Open,
            "paused" => Self::Paused,
            "closed" => Self::Closed,
            other => unreachable!(
                "task_templates.state is constrained by CHECK to open/paused/closed; read {other:?}"
            ),
        }
    }
}

impl std::fmt::Display for TemplateState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// One `task_templates` row, as read back by [`list`], [`get_by_id`], and
/// [`get_by_name`]. A plain data record, mirroring `create::Task`'s own
/// stance: nothing here talks back to the database, and every identifier
/// column this module itself wrote is parsed into its typed form on the way
/// out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub id: uuid::Uuid,
    pub name: String,
    /// Read back as an `Option` because the column is nullable, and written
    /// only as `Some` because [`create`] requires one.
    ///
    /// `None` is **not** design §11's run that "remains queued for the
    /// central agent to assign." That sentence is about the **agent**
    /// ([`Template::target_agent_name`]), not the scope. A run's own
    /// `tasks.target_scope_id` is `NOT NULL`, so a template without a scope
    /// could never produce a run at all: an operator would be able to create
    /// one that silently never fires. The column stays nullable so a later
    /// station can relax the rule without a migration; version 1 does not.
    pub target_scope_id: Option<uuid::Uuid>,
    pub target_agent_name: Option<String>,
    pub prompt: String,
    /// §12.1's hook. Prose, not a machine gate — version 1 stores the
    /// criterion and leaves the gate to a human.
    pub acceptance_criteria: Option<String>,
    pub version: i64,
    pub state: TemplateState,
    pub created_at: String,
    pub updated_at: String,
}

const TEMPLATE_COLUMNS: &str = "id, name, target_scope_id, target_agent_name, prompt, \
     acceptance_criteria, version, state, created_at, updated_at";

fn row_to_template(row: &rusqlite::Row<'_>) -> rusqlite::Result<Template> {
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let target_scope_id: Option<String> = row.get(2)?;
    let target_agent_name: Option<String> = row.get(3)?;
    let prompt: String = row.get(4)?;
    let acceptance_criteria: Option<String> = row.get(5)?;
    let version: i64 = row.get(6)?;
    let state: String = row.get(7)?;
    let created_at: String = row.get(8)?;
    let updated_at: String = row.get(9)?;

    Ok(Template {
        id: uuid::Uuid::parse_str(&id)
            .unwrap_or_else(|e| panic!("task_templates.id is a UUID; read {id:?}: {e}")),
        name,
        target_scope_id: target_scope_id.as_deref().map(|s| {
            uuid::Uuid::parse_str(s).unwrap_or_else(|e| {
                panic!("task_templates.target_scope_id is a UUID; read {s:?}: {e}")
            })
        }),
        target_agent_name,
        prompt,
        acceptance_criteria,
        version,
        state: TemplateState::from_db_str(&state),
        created_at,
        updated_at,
    })
}

/// Commit a new template and return the id it was written under.
///
/// `name` must be unique (`task_templates_name`). The check runs inside this
/// function's own transaction, which takes the write lock immediately
/// (`Store::transaction`'s `BEGIN IMMEDIATE`) before either the uniqueness
/// read or the insert — the same race-freedom argument
/// `factory_session::begin_start` makes for its own pre-insert scan — so no
/// concurrent `create` can land a same-named row between the check and the
/// write. [`TemplateError::NameTaken`] is the typed result; a duplicate name
/// is a caller error worth naming, not a panic and not a raw constraint
/// error leaking through.
///
/// `state` is left at its schema default, `'open'`: nothing pauses or closes
/// a template at the moment it is created.
///
/// `id` is supplied by the caller, mirroring `create::create`'s own `id`
/// parameter and for the identical reason recorded on that function's doc
/// comment: the `uuid` crate is pinned workspace-wide without the `v4`
/// feature.
#[allow(clippy::too_many_arguments)]
pub fn create(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    name: &str,
    target_scope_id: uuid::Uuid,
    target_agent_name: Option<&str>,
    prompt: &str,
    acceptance_criteria: Option<&str>,
) -> Result<uuid::Uuid, TemplateError> {
    let tx = store.transaction()?;

    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM task_templates WHERE name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    if existing.is_some() {
        return Err(TemplateError::NameTaken(name.to_string()));
    }

    tx.execute(
        "INSERT INTO task_templates \
         (id, name, target_scope_id, target_agent_name, prompt, acceptance_criteria) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            id.to_string(),
            name,
            target_scope_id.to_string(),
            target_agent_name,
            prompt,
            acceptance_criteria,
        ),
    )
    .map_err(factory_store::StoreError::from)?;

    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(id)
}

/// One template by id. Takes `&Store`, not `&mut Store` — per
/// `Store::connection`'s own doc comment, a read path must never go through
/// `Store::transaction`, which takes the write lock and would needlessly
/// contend with real writers under WAL. Mirrors `create::show`.
pub fn get_by_id(store: &factory_store::Store, id: uuid::Uuid) -> Result<Template, TemplateError> {
    store
        .connection()
        .query_row(
            &format!("SELECT {TEMPLATE_COLUMNS} FROM task_templates WHERE id = ?1"),
            [id.to_string()],
            row_to_template,
        )
        .optional()
        .map_err(factory_store::StoreError::from)?
        .ok_or(TemplateError::NotFound(id))
}

/// One template by its unique name — the handle an operator and
/// `factory schedule create` use (`factory_store::schema`'s own comment on
/// `task_templates.name`).
pub fn get_by_name(store: &factory_store::Store, name: &str) -> Result<Template, TemplateError> {
    store
        .connection()
        .query_row(
            &format!("SELECT {TEMPLATE_COLUMNS} FROM task_templates WHERE name = ?1"),
            [name],
            row_to_template,
        )
        .optional()
        .map_err(factory_store::StoreError::from)?
        .ok_or_else(|| TemplateError::NoSuchName(name.to_string()))
}

/// Every template, oldest first — read-only inspection before automation
/// (AGENTS.md: "Design read-only inspection... before automation"), mirroring
/// `create::list`.
pub fn list(store: &factory_store::Store) -> Result<Vec<Template>, TemplateError> {
    let mut stmt = store
        .connection()
        .prepare(&format!(
            "SELECT {TEMPLATE_COLUMNS} FROM task_templates ORDER BY created_at, id"
        ))
        .map_err(factory_store::StoreError::from)?;
    let rows = stmt
        .query_map([], row_to_template)
        .map_err(factory_store::StoreError::from)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| factory_store::StoreError::from(e).into())
}

/// Change `prompt` and/or `acceptance_criteria` and add one to `version`.
///
/// Either argument may be `None`, meaning "leave this field as it is," not
/// "clear it" — there is no way through this function to null out
/// `acceptance_criteria` once set. Passing `None` for both still bumps the
/// version: `factory_store::schema`'s comment on `task_templates.version`
/// says it "increases when the prompt or the criteria change," and calling
/// `revise` at all *is* that event from this module's point of view — an
/// operator invoking it is asserting a change happened (re-affirming the
/// same wording after a review counts), and nothing here can tell that case
/// apart from a caller mistake, so it does not try to.
///
/// Returns the new version, so a caller can see the number without a second
/// read — mirrors `create::cancel`'s own "so a caller can tell the two
/// outcomes apart without a second read."
pub fn revise(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    prompt: Option<&str>,
    acceptance_criteria: Option<&str>,
) -> Result<i64, TemplateError> {
    let tx = store.transaction()?;

    let current: Option<(String, Option<String>, i64)> = tx
        .query_row(
            "SELECT prompt, acceptance_criteria, version FROM task_templates WHERE id = ?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(factory_store::StoreError::from)?;
    let Some((current_prompt, current_criteria, current_version)) = current else {
        return Err(TemplateError::NotFound(id));
    };

    let new_prompt = prompt.unwrap_or(&current_prompt);
    let new_criteria = acceptance_criteria.or(current_criteria.as_deref());
    let new_version = current_version + 1;

    tx.execute(
        "UPDATE task_templates SET prompt = ?2, acceptance_criteria = ?3, version = ?4, \
         updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        (id.to_string(), new_prompt, new_criteria, new_version),
    )
    .map_err(factory_store::StoreError::from)?;

    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(new_version)
}

/// Move a template to `state` — open, paused, or closed.
///
/// `state` is [`TemplateState`], not a string: the outside-the-vocabulary
/// case this function might otherwise need to reject is unrepresentable
/// through this signature, the same way [`crate::TaskStatus`] and
/// [`crate::BlockedReason`] keep an invalid value out of `tasks.status` and
/// `tasks.blocked_reason`. The schema's own CHECK is still the backstop for
/// anything that reaches this table by a path other than this function —
/// see `state_round_trips_through_the_typed_api_and_the_schema_check_is_a_backstop`
/// in `tests/template.rs`.
pub fn set_state(
    store: &mut factory_store::Store,
    id: uuid::Uuid,
    state: TemplateState,
) -> Result<(), TemplateError> {
    let tx = store.transaction()?;
    let changed = tx
        .execute(
            "UPDATE task_templates SET state = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
            (id.to_string(), state.as_db_str()),
        )
        .map_err(factory_store::StoreError::from)?;
    if changed == 0 {
        return Err(TemplateError::NotFound(id));
    }
    tx.commit().map_err(factory_store::StoreError::from)?;
    Ok(())
}
