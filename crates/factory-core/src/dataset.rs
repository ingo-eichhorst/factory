//! A dataset is a set of cases you can run a bench against. It lives as one
//! file, `<root>/.factory/datasets/<name>.yaml`, authored content like the
//! knowledge vault -- the file is the source of truth, re-parsed on every
//! read, so a hand edit shows up on the next request without anything having
//! to notice it changed.
//!
//! Everything in this module is pure: parsing, validation, the bulk
//! importers, and turning a recorded task into a case all work on strings and
//! structs, never touching a socket or a git repository. The daemon
//! (`factory-daemon/src/datasets.rs`) owns the file itself -- reading it,
//! writing it back via a temp file and a rename, and the one lock per dataset
//! that keeps two writers from tearing it in half -- and calls into this
//! module to do the actual work.
//!
//! **`base` needs `git merge-base`,** which is a subprocess call this module
//! cannot make. `case_from_task` takes an already-resolved `base` rather than
//! computing one itself; the daemon resolves it (when the newest run's
//! worktree branch still exists) before calling in.

use crate::error::{FactoryError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn default_version() -> u32 {
    1
}

/// Where a case came from, when it was generated from a recorded task.
/// Informational only -- the old outcome is never scored, and a generated
/// case always starts life with no gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseOrigin {
    pub task: String,
    pub run: String,
    pub outcome: String,
    pub recorded_at: DateTime<Utc>,
}

/// One case: a fixture a bench run attempts, with an optional reset and gate.
/// `gate` and `reset` are shell commands that run as the owner inside the
/// attempt's worktree -- the same trust as the `shell` agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// Unique within the dataset. `[a-z0-9][a-z0-9-]*`, the same pattern as
    /// a dataset's own `name`.
    pub id: String,
    pub title: String,
    /// Where every attempt of this case runs. Accepted even when it names no
    /// scope in the current config -- see `findings`' `unknown_scope`.
    pub scope: String,
    #[serde(default)]
    pub instructions: String,
    /// A commit to branch attempts from. Absent means the scope's HEAD when
    /// the bench run starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Runs in the attempt worktree before the agent starts. A non-zero exit
    /// skips the attempt without dispatching the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<String>,
    /// Runs in the attempt worktree after the attempt ends. Its exit status
    /// is the verdict. Absent means the case runs `unverified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Set only when this case was generated from a recorded task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<CaseOrigin>,
}

/// A dataset, exactly as it is written to
/// `<root>/.factory/datasets/<name>.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Equals the file stem. `[a-z0-9][a-z0-9-]*`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Bumped by Factory on every write it makes. A hand edit that skips this
    /// is honest -- nothing here pretends to know a file changed underneath
    /// it, only what its own writes did.
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub cases: Vec<Case>,
}

impl Dataset {
    pub fn new(name: impl Into<String>, description: Option<String>) -> Self {
        Self {
            version: default_version(),
            name: name.into(),
            description,
            revision: 0,
            cases: Vec::new(),
        }
    }

    /// Parse a dataset from its YAML text. An unknown top-level field is
    /// refused (`deny_unknown_fields`), not silently dropped.
    pub fn parse(text: &str) -> Result<Self> {
        serde_yaml_ng::from_str(text)
            .map_err(|e| FactoryError::BadRequest(format!("parsing dataset: {e}")))
    }

    /// Read `<dir>/<name>.yaml`. `None` when the file does not exist -- an
    /// absent dataset is a fact for the caller to decide what to do with,
    /// not an error.
    ///
    /// Refuses a `name` that is not a slug before it ever touches a path --
    /// defense in depth behind the daemon's own check, since `name` here
    /// becomes a path component and `../elsewhere` is otherwise a valid one.
    pub fn load(dir: &Path, name: &str) -> Result<Option<Self>> {
        refuse_bad_name(name)?;
        let path = dir.join(format!("{name}.yaml"));
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(Self::parse(&text)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(FactoryError::Other(anyhow::anyhow!(
                "reading {}: {e}",
                path.display()
            ))),
        }
    }

    /// Write to `<dir>/<name>.yaml` via a temp file and a rename, so a crash
    /// mid-write never leaves half a YAML file. The caller (the daemon) is
    /// responsible for holding this dataset's write lock and bumping
    /// `revision` before calling this.
    ///
    /// Refuses a `self.name` that is not a slug -- same defense in depth as
    /// `load`, since `validate()` is a separate call a caller could forget.
    pub fn write_atomic(&self, dir: &Path) -> Result<()> {
        refuse_bad_name(&self.name)?;
        std::fs::create_dir_all(dir)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("making {}: {e}", dir.display())))?;
        let path = dir.join(format!("{}.yaml", self.name));
        let tmp = dir.join(format!(".{}.yaml.tmp-{}", self.name, std::process::id()));
        let text = serde_yaml_ng::to_string(self)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("serializing dataset: {e}")))?;
        std::fs::write(&tmp, text)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("writing {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            FactoryError::Other(anyhow::anyhow!(
                "renaming {} to {}: {e}",
                tmp.display(),
                path.display()
            ))
        })?;
        Ok(())
    }

    /// Every problem with this dataset that refuses the write it would take
    /// to produce it: a bad `name` or case `id`, a missing required field, or
    /// a duplicate case id. `unknown_scope` is not among these -- that is a
    /// finding, not a refusal, because scopes come and go.
    pub fn validate(&self) -> Result<()> {
        let mut problems = Vec::new();
        if !is_slug(&self.name) {
            problems.push(format!(
                "name {:?} must match [a-z0-9][a-z0-9-]*",
                self.name
            ));
        }
        let mut seen = BTreeSet::new();
        for case in &self.cases {
            problems.extend(case_problems(case).into_iter().map(|(field, detail)| {
                format!("case {:?}: {field}: {detail}", case.id)
            }));
            if !seen.insert(case.id.clone()) {
                problems.push(format!("duplicate case id {:?}", case.id));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(FactoryError::BadRequest(problems.join("; ")))
        }
    }
}

/// `^[a-z0-9][a-z0-9-]*$` -- a dataset's `name`, and a case's `id`.
pub fn is_slug(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A dataset `name` becomes a path component the moment it reaches `load` or
/// `write_atomic` -- `is_slug` is what stands between that and `../elsewhere`
/// or an absolute path escaping `<root>/.factory/datasets/` entirely.
fn refuse_bad_name(name: &str) -> Result<()> {
    if is_slug(name) {
        Ok(())
    } else {
        Err(FactoryError::BadRequest(format!(
            "dataset name {name:?} must match [a-z0-9][a-z0-9-]*"
        )))
    }
}

/// What is wrong with one case, as `(field, detail)` pairs -- everything a
/// write refuses. `id`'s pattern is checked here too; duplicate detection
/// needs the whole dataset (or the whole imported batch) and stays with the
/// caller.
fn case_problems(case: &Case) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !is_slug(&case.id) {
        out.push(("id".to_string(), format!("{:?} must match [a-z0-9][a-z0-9-]*", case.id)));
    }
    if case.title.trim().is_empty() {
        out.push(("title".to_string(), "title is required".to_string()));
    }
    if case.scope.trim().is_empty() {
        out.push(("scope".to_string(), "scope is required".to_string()));
    }
    if case.instructions.trim().is_empty() {
        out.push(("instructions".to_string(), "instructions is required".to_string()));
    }
    if let Some(base) = &case.base {
        if !is_git_revish(base) {
            out.push((
                "base".to_string(),
                format!(
                    "{base:?} is not a safe git revision (letters, digits, `.` `_` `/` `-`, \
                     no leading `-`, no `..`)"
                ),
            ));
        }
    }
    out
}

/// A conservative shape for a case's `base`: this is never trusted to *name
/// an existing commit* here -- that needs a live git repository, which this
/// module never touches -- only to be a string git's own argument parser
/// cannot mistake for an option or a path-escaping revision range. No
/// leading `-` (refuses `--detach`, `-b`, and the like, which git would
/// otherwise parse as flags) and no `..` (refuses a range like `a..b` where
/// a single commit is meant). `factory-daemon/src/bench/engine.rs` checks
/// this again itself before ever building a `git` command line -- a
/// hand-edited dataset file bypasses this check entirely, since it never
/// goes through `validate()`.
pub fn is_git_revish(s: &str) -> bool {
    if s.is_empty() || s.contains("..") {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
}

/// The one finding kind a dataset carries today: a case names a scope the
/// current config does not have. Never a refusal -- scopes come and go, and a
/// bench run refuses the case later, by name, if it is still missing then.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DatasetFindingKind {
    UnknownScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatasetFinding {
    pub kind: DatasetFindingKind,
    pub case: String,
    pub detail: String,
}

/// Findings against the live config, sorted by case then kind. Computed
/// fresh on every read -- a scope can appear and disappear between two
/// requests, and nothing here is cached.
pub fn findings(dataset: &Dataset, known_scopes: &BTreeSet<String>) -> Vec<DatasetFinding> {
    let mut out: Vec<DatasetFinding> = dataset
        .cases
        .iter()
        .filter(|case| !known_scopes.contains(&case.scope))
        .map(|case| DatasetFinding {
            kind: DatasetFindingKind::UnknownScope,
            case: case.id.clone(),
            detail: format!("scope {:?} is not in the current config", case.scope),
        })
        .collect();
    out.sort_by(|a, b| a.case.cmp(&b.case).then_with(|| a.kind.cmp(&b.kind)));
    out
}

/// One row of `GET /api/datasets`: enough to list every dataset without
/// shipping every case of every one of them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatasetSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub revision: u64,
    pub cases: usize,
    /// How many of `cases` set a `gate`. The rest run `unverified` until one
    /// is added.
    pub gated: usize,
    pub findings: Vec<DatasetFinding>,
}

pub fn summarize(dataset: &Dataset, known_scopes: &BTreeSet<String>) -> DatasetSummary {
    DatasetSummary {
        name: dataset.name.clone(),
        description: dataset.description.clone(),
        revision: dataset.revision,
        cases: dataset.cases.len(),
        gated: dataset.cases.iter().filter(|c| c.gate.is_some()).count(),
        findings: findings(dataset, known_scopes),
    }
}

// -- bulk import ---------------------------------------------------------

/// One thing wrong with an imported file: the line or row it was on (1-based;
/// for a whole-dataset or case-list YAML/JSON document, the case's position
/// in the list, since neither format hands back a real line for one item),
/// the field, and what was wrong.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportProblem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub detail: String,
}

impl std::fmt::Display for ImportProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, &self.field) {
            (Some(line), Some(field)) => write!(f, "line {line}, {field}: {}", self.detail),
            (Some(line), None) => write!(f, "line {line}: {}", self.detail),
            (None, Some(field)) => write!(f, "{field}: {}", self.detail),
            (None, None) => write!(f, "{}", self.detail),
        }
    }
}

/// Every accepted import format, dispatched by file extension.
pub fn import(format: &str, content: &str) -> std::result::Result<Vec<Case>, Vec<ImportProblem>> {
    match format {
        "jsonl" => import_jsonl(content),
        "json" => import_json(content),
        "yaml" | "yml" => import_yaml(content),
        "csv" => import_csv(content),
        other => Err(vec![ImportProblem {
            line: None,
            field: None,
            detail: format!("unrecognised import format {other:?}; use jsonl, json, yaml, yml, or csv"),
        }]),
    }
}

/// One case per line.
pub fn import_jsonl(content: &str) -> std::result::Result<Vec<Case>, Vec<ImportProblem>> {
    let mut rows = Vec::new();
    let mut problems = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let line_no = i + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Case>(trimmed) {
            Ok(case) => rows.push((line_no, case)),
            Err(e) => problems.push(ImportProblem {
                line: Some(line_no),
                field: extract_field(&e.to_string()),
                detail: e.to_string(),
            }),
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    finalize(rows)
}

/// An array of cases.
pub fn import_json(content: &str) -> std::result::Result<Vec<Case>, Vec<ImportProblem>> {
    match serde_json::from_str::<Vec<Case>>(content) {
        Ok(cases) => finalize(cases.into_iter().enumerate().map(|(i, c)| (i + 1, c)).collect()),
        Err(e) => Err(vec![ImportProblem {
            line: Some(e.line()),
            field: extract_field(&e.to_string()),
            detail: e.to_string(),
        }]),
    }
}

/// Either a whole dataset document or a bare list of cases.
pub fn import_yaml(content: &str) -> std::result::Result<Vec<Case>, Vec<ImportProblem>> {
    let looks_like_dataset = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(content)
        .ok()
        .and_then(|v| v.as_mapping().map(|m| {
            m.contains_key(serde_yaml_ng::Value::String("cases".into()))
        }))
        .unwrap_or(false);

    if looks_like_dataset {
        return match serde_yaml_ng::from_str::<Dataset>(content) {
            Ok(ds) => finalize(ds.cases.into_iter().enumerate().map(|(i, c)| (i + 1, c)).collect()),
            Err(e) => Err(vec![yaml_problem(e)]),
        };
    }
    match serde_yaml_ng::from_str::<Vec<Case>>(content) {
        Ok(cases) => finalize(cases.into_iter().enumerate().map(|(i, c)| (i + 1, c)).collect()),
        Err(e) => Err(vec![yaml_problem(e)]),
    }
}

fn yaml_problem(e: serde_yaml_ng::Error) -> ImportProblem {
    ImportProblem {
        line: e.location().map(|l| l.line()),
        field: extract_field(&e.to_string()),
        detail: e.to_string(),
    }
}

/// The header row names case fields; `origin` is not one of them -- a
/// generated case's origin has no natural flat-row shape, and a bulk import
/// of recorded outcomes belongs to `from-tasks`, not a spreadsheet.
const CSV_FIELDS: &[&str] = &[
    "id",
    "title",
    "scope",
    "instructions",
    "base",
    "reset",
    "gate",
    "timeout_seconds",
];

pub fn import_csv(content: &str) -> std::result::Result<Vec<Case>, Vec<ImportProblem>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(content.as_bytes());
    let headers = match reader.headers() {
        Ok(h) => h.clone(),
        Err(e) => {
            return Err(vec![ImportProblem {
                line: Some(1),
                field: None,
                detail: format!("reading the header row: {e}"),
            }])
        }
    };
    if headers.iter().any(|h| h == "origin") {
        return Err(vec![ImportProblem {
            line: Some(1),
            field: Some("origin".to_string()),
            detail: "origin is not accepted in a CSV import".to_string(),
        }]);
    }
    for h in headers.iter() {
        if !CSV_FIELDS.contains(&h) {
            return Err(vec![ImportProblem {
                line: Some(1),
                field: Some(h.to_string()),
                detail: format!("unknown case field {h:?}"),
            }]);
        }
    }

    let mut rows = Vec::new();
    let mut problems = Vec::new();
    for (i, record) in reader.records().enumerate() {
        // Row 1 is the header, so the first data row is row 2.
        let row = i + 2;
        let record = match record {
            Ok(r) => r,
            Err(e) => {
                problems.push(ImportProblem {
                    line: Some(row),
                    field: None,
                    detail: e.to_string(),
                });
                continue;
            }
        };
        let get = |name: &str| -> Option<String> {
            headers
                .iter()
                .position(|h| h == name)
                .and_then(|idx| record.get(idx))
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        };

        let id = get("id");
        let title = get("title");
        let scope = get("scope");
        for (field, value) in [("id", &id), ("title", &title), ("scope", &scope)] {
            if value.is_none() {
                problems.push(ImportProblem {
                    line: Some(row),
                    field: Some(field.to_string()),
                    detail: format!("{field} is required"),
                });
            }
        }
        let timeout_seconds = match get("timeout_seconds") {
            Some(s) => match s.parse::<u64>() {
                Ok(v) => Some(v),
                Err(_) => {
                    problems.push(ImportProblem {
                        line: Some(row),
                        field: Some("timeout_seconds".to_string()),
                        detail: format!("{s:?} is not a whole number of seconds"),
                    });
                    None
                }
            },
            None => None,
        };

        if let (Some(id), Some(title), Some(scope)) = (id, title, scope) {
            rows.push((
                row,
                Case {
                    id,
                    title,
                    scope,
                    instructions: get("instructions").unwrap_or_default(),
                    base: get("base"),
                    reset: get("reset"),
                    gate: get("gate"),
                    timeout_seconds,
                    origin: None,
                },
            ));
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    finalize(rows)
}

/// Grab the first backtick-quoted identifier out of a serde error message
/// (`unknown field \`x\`[…]`, `missing field \`y\`` and the like), so a
/// problem's `field` is filled in without a second, hand-rolled parser
/// alongside serde's own. `None` for a message with no such quoting -- a
/// human still reads it in `detail`, but the field is left unnamed rather
/// than guessed at.
fn extract_field(message: &str) -> Option<String> {
    let start = message.find('`')? + 1;
    let end = message[start..].find('`')? + start;
    Some(message[start..end].to_string())
}

/// Shared by every importer: validate each case (its own field-level
/// problems, reported against the position it was found at), refuse a
/// duplicate id anywhere in the batch, and refuse the whole import -- never
/// part of it -- the moment anything is wrong.
fn finalize(rows: Vec<(usize, Case)>) -> std::result::Result<Vec<Case>, Vec<ImportProblem>> {
    let mut problems = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (line, case) in &rows {
        for (field, detail) in case_problems(case) {
            problems.push(ImportProblem {
                line: Some(*line),
                field: Some(field),
                detail,
            });
        }
        match seen.get(&case.id) {
            Some(first) => problems.push(ImportProblem {
                line: Some(*line),
                field: Some("id".to_string()),
                detail: format!("duplicate id {:?}; first seen at line {first}", case.id),
            }),
            None => {
                seen.insert(case.id.clone(), *line);
            }
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(rows.into_iter().map(|(_, case)| case).collect())
}

// -- generating from recorded tasks ---------------------------------------

/// A slug of `title`, de-duplicated against `taken` with `-2`, `-3`, and so
/// on. `taken` is updated in place, so a batch of many tasks de-duplicates
/// against itself as well as whatever ids the dataset already has -- pass in
/// the dataset's existing case ids before the first call.
pub fn next_case_id(title: &str, taken: &mut BTreeSet<String>) -> String {
    let base = slugify(title);
    let base = if base.is_empty() { "case".to_string() } else { base };
    let mut candidate = base.clone();
    let mut n = 2;
    while taken.contains(&candidate) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

/// Lowercase ascii, hyphen-joined, no leading, trailing, or doubled hyphens,
/// capped well short of anything absurd -- the same shape
/// `factory-daemon/src/worktree.rs`'s own `slugify` produces for a branch
/// name, duplicated here since `factory-core` cannot depend on the daemon
/// crate to share it.
fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(40);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// One case, built from a recorded task. Pure: `base` is already resolved by
/// the caller (`git merge-base` needs a subprocess, which this module never
/// runs), and `id` already de-duplicated via `next_case_id`. Never sets
/// `gate` -- a generated case runs `unverified` until a person adds one.
#[allow(clippy::too_many_arguments)]
pub fn case_from_task(
    id: String,
    title: &str,
    scope: &str,
    instructions: &str,
    task_id: &str,
    run_id: &str,
    outcome: &str,
    recorded_at: DateTime<Utc>,
    base: Option<String>,
) -> Case {
    Case {
        id,
        title: title.to_string(),
        scope: scope.to_string(),
        instructions: instructions.to_string(),
        base,
        reset: None,
        gate: None,
        timeout_seconds: None,
        origin: Some(CaseOrigin {
            task: task_id.to_string(),
            run: run_id.to_string(),
            outcome: outcome.to_string(),
            recorded_at,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(id: &str) -> Case {
        Case {
            id: id.to_string(),
            title: "A case".into(),
            scope: "demo".into(),
            instructions: "do the thing".into(),
            base: None,
            reset: None,
            gate: None,
            timeout_seconds: None,
            origin: None,
        }
    }

    #[test]
    fn a_dataset_round_trips_through_yaml() {
        let mut ds = Dataset::new("registration", Some("desc".into()));
        ds.cases.push(case("add-scope"));
        let text = serde_yaml_ng::to_string(&ds).unwrap();
        let back = Dataset::parse(&text).unwrap();
        assert_eq!(back, ds);
    }

    #[test]
    fn an_unknown_top_level_field_is_refused() {
        let e = Dataset::parse("name: x\ncases: []\nbogus: true\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("bogus"), "{e}");
    }

    #[test]
    fn an_unknown_case_field_is_refused() {
        let yaml = "name: x\ncases:\n  - id: a\n    title: A\n    scope: demo\n    bogus: 1\n";
        let e = Dataset::parse(yaml).unwrap_err().to_string();
        assert!(e.contains("bogus"), "{e}");
    }

    #[test]
    fn name_and_id_patterns_are_enforced() {
        let mut ds = Dataset::new("Bad Name", None);
        ds.cases.push(case("Also Bad"));
        let e = ds.validate().unwrap_err().to_string();
        assert!(e.contains("Bad Name"), "{e}");
        assert!(e.contains("Also Bad"), "{e}");
    }

    #[test]
    fn required_case_fields_are_enforced() {
        let mut ds = Dataset::new("demo", None);
        ds.cases.push(Case {
            id: "a".into(),
            title: "".into(),
            scope: "".into(),
            instructions: "".into(),
            base: None,
            reset: None,
            gate: None,
            timeout_seconds: None,
            origin: None,
        });
        let e = ds.validate().unwrap_err().to_string();
        assert!(e.contains("title"), "{e}");
        assert!(e.contains("scope"), "{e}");
        assert!(e.contains("instructions"), "{e}");
    }

    /// Reported by QA, reproduced live: `"base":"--detach"` reached `git`
    /// unvalidated and was parsed as a flag rather than a revision. A `base`
    /// must be shaped like a revision `git` could only ever read as one --
    /// checked at write time here; `factory-daemon/src/bench/engine.rs`
    /// checks it again itself, right before ever building a `git` command
    /// line, since a hand-edited dataset file skips `validate()` entirely.
    #[test]
    fn a_base_shaped_like_a_git_option_or_a_range_is_refused() {
        let mut ds = Dataset::new("demo", None);
        let mut bad = case("a");
        bad.base = Some("--detach".into());
        ds.cases.push(bad);
        let e = ds.validate().unwrap_err().to_string();
        assert!(e.contains("base"), "{e}");
        assert!(e.contains("--detach"), "{e}");

        let mut ds = Dataset::new("demo", None);
        let mut bad = case("a");
        bad.base = Some("main..feature".into());
        ds.cases.push(bad);
        let e = ds.validate().unwrap_err().to_string();
        assert!(e.contains("base"), "{e}");

        // A bad shape is refused at write time; an unresolvable-but-well-
        // shaped one (a commit that plausibly never existed) is not this
        // module's problem to catch -- `resolve_commit` at bench-run start
        // is what checks a `base` actually names a commit, since only that
        // needs a live git repository.
        let mut ds = Dataset::new("demo", None);
        let mut ok = case("a");
        ok.base = Some("deadbeef".into());
        ds.cases.push(ok);
        ds.validate().unwrap();
    }

    #[test]
    fn a_duplicate_case_id_is_refused() {
        let mut ds = Dataset::new("demo", None);
        ds.cases.push(case("dup"));
        ds.cases.push(case("dup"));
        let e = ds.validate().unwrap_err().to_string();
        assert!(e.contains("duplicate"), "{e}");
        assert!(e.contains("dup"), "{e}");
    }

    #[test]
    fn a_scope_not_in_the_config_is_a_finding_not_a_refusal() {
        let mut ds = Dataset::new("demo", None);
        ds.cases.push(case("a"));
        ds.validate().unwrap(); // accepted despite the unknown scope
        let known: BTreeSet<String> = BTreeSet::new();
        let f = findings(&ds, &known);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, DatasetFindingKind::UnknownScope);
        assert_eq!(f[0].case, "a");

        let known: BTreeSet<String> = ["demo".to_string()].into_iter().collect();
        assert!(findings(&ds, &known).is_empty());
    }

    #[test]
    fn summarize_counts_cases_gated_cases_and_findings() {
        let mut ds = Dataset::new("demo", Some("d".into()));
        let mut gated = case("a");
        gated.gate = Some("./check.sh".into());
        ds.cases.push(gated);
        ds.cases.push(case("b"));
        let known: BTreeSet<String> = BTreeSet::new();
        let summary = summarize(&ds, &known);
        assert_eq!(summary.cases, 2);
        assert_eq!(summary.gated, 1);
        assert_eq!(summary.findings.len(), 2, "both cases name an unknown scope");
    }

    #[test]
    fn a_dataset_survives_a_write_then_read_round_trip() {
        let dir = std::env::temp_dir().join(format!("factory-dataset-test-{}", uuid::Uuid::new_v4()));
        let mut ds = Dataset::new("demo", Some("d".into()));
        ds.cases.push(case("a"));
        ds.write_atomic(&dir).unwrap();
        let loaded = Dataset::load(&dir, "demo").unwrap().unwrap();
        assert_eq!(loaded, ds);
        assert!(Dataset::load(&dir, "missing").unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `load` and `write_atomic` take `name` (or `self.name`) as a path
    /// component -- `../elsewhere` would otherwise read or, worse, let a
    /// caller further up write outside `<root>/.factory/datasets/` entirely.
    /// This is the defense-in-depth layer behind the daemon's own
    /// `refuse_bad_name` (`factory-daemon/src/datasets.rs`); this test is
    /// what proves this module refuses it even if that layer is ever
    /// bypassed or forgotten at a new call site.
    #[test]
    fn load_and_write_atomic_refuse_a_name_that_would_escape_the_datasets_directory() {
        let dir = std::env::temp_dir().join(format!("factory-dataset-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // A file a path-traversing `load`/`delete` could otherwise reach.
        std::fs::write(dir.parent().unwrap().join("outside.yaml"), "name: outside\ncases: []\n").ok();

        let e = Dataset::load(&dir, "../outside").unwrap_err().to_string();
        assert!(e.contains("must match"), "{e}");

        let bad = Dataset::new("../outside", None);
        let e = bad.write_atomic(&dir).unwrap_err().to_string();
        assert!(e.contains("must match"), "{e}");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_file(dir.parent().unwrap().join("outside.yaml")).ok();
    }

    // -- importers -----------------------------------------------------

    #[test]
    fn jsonl_imports_one_case_per_line() {
        let content = "{\"id\":\"a\",\"title\":\"A\",\"scope\":\"demo\",\"instructions\":\"x\"}\n{\"id\":\"b\",\"title\":\"B\",\"scope\":\"demo\",\"instructions\":\"y\"}\n";
        let cases = import_jsonl(content).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].id, "a");
        assert_eq!(cases[1].id, "b");
    }

    #[test]
    fn json_imports_an_array() {
        let content = r#"[{"id":"a","title":"A","scope":"demo","instructions":"x"}]"#;
        let cases = import_json(content).unwrap();
        assert_eq!(cases.len(), 1);
    }

    #[test]
    fn yaml_imports_a_bare_list_or_a_whole_dataset() {
        let list = "- id: a\n  title: A\n  scope: demo\n  instructions: x\n";
        assert_eq!(import_yaml(list).unwrap().len(), 1);

        let whole = "name: demo\ncases:\n  - id: a\n    title: A\n    scope: demo\n    instructions: x\n";
        assert_eq!(import_yaml(whole).unwrap().len(), 1);
    }

    #[test]
    fn csv_imports_from_a_header_row() {
        let content = "id,title,scope,instructions\na,A,demo,do it\nb,B,demo,do it too\n";
        let cases = import_csv(content).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].id, "a");
        assert_eq!(cases[1].scope, "demo");
    }

    #[test]
    fn csv_refuses_an_origin_column() {
        let content = "id,title,scope,origin\na,A,demo,x\n";
        let problems = import_csv(content).unwrap_err();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field.as_deref(), Some("origin"));
    }

    #[test]
    fn csv_refuses_an_unknown_column() {
        let content = "id,title,scope,bogus\na,A,demo,x\n";
        let problems = import_csv(content).unwrap_err();
        assert_eq!(problems[0].field.as_deref(), Some("bogus"));
    }

    #[test]
    fn import_is_all_or_nothing_and_names_the_line_and_field() {
        // Row 2 duplicates row 3's id -- both individually well-formed, but
        // the batch as a whole is refused, and the duplicate names its line.
        let content = "id,title,scope\na,A,demo\na,A2,demo\n";
        let problems = import_csv(content).unwrap_err();
        assert!(problems.iter().any(|p| p.line == Some(3) && p.field.as_deref() == Some("id")));

        // A row missing a required field is refused the same way, and no
        // case is accepted even though every other row in the file is fine.
        let content = "id,title,scope\na,A,\nb,B,demo\n";
        let problems = import_csv(content).unwrap_err();
        assert!(problems.iter().any(|p| p.line == Some(2) && p.field.as_deref() == Some("scope")));
        assert!(
            import_csv(content).is_err(),
            "the whole import is refused, not just the bad row"
        );
    }

    #[test]
    fn a_duplicate_id_across_json_rows_is_refused_all_or_nothing() {
        let content = r#"[{"id":"a","title":"A","scope":"demo","instructions":"x"},{"id":"a","title":"A2","scope":"demo","instructions":"y"}]"#;
        let problems = import_json(content).unwrap_err();
        assert!(problems.iter().any(|p| p.detail.contains("duplicate")));
    }

    #[test]
    fn an_unknown_field_in_a_jsonl_row_names_the_line() {
        let content = "{\"id\":\"a\",\"title\":\"A\",\"scope\":\"demo\",\"bogus\":1}\n";
        let problems = import_jsonl(content).unwrap_err();
        assert_eq!(problems[0].line, Some(1));
    }

    #[test]
    fn missing_dataset_import_creates_it_is_a_daemon_concern_not_this_module() {
        // Nothing to assert here beyond the module boundary itself: this
        // module never touches the filesystem to decide whether a dataset
        // exists. The daemon's own test in `datasets.rs` covers create-on-
        // import.
    }

    // -- from-tasks ------------------------------------------------------

    #[test]
    fn case_ids_are_slugged_and_deduplicated() {
        let mut taken = BTreeSet::new();
        assert_eq!(next_case_id("Register a new scope", &mut taken), "register-a-new-scope");
        assert_eq!(next_case_id("Register a new scope", &mut taken), "register-a-new-scope-2");
        assert_eq!(next_case_id("Register a new scope", &mut taken), "register-a-new-scope-3");
    }

    #[test]
    fn next_case_id_avoids_ids_already_in_the_dataset() {
        let mut taken: BTreeSet<String> = ["reindex".to_string()].into_iter().collect();
        assert_eq!(next_case_id("Reindex", &mut taken), "reindex-2");
    }

    #[test]
    fn a_case_built_from_a_task_records_its_origin_and_never_sets_a_gate() {
        let at = DateTime::parse_from_rfc3339("2026-09-10T14:02:11Z").unwrap().with_timezone(&Utc);
        let case = case_from_task(
            "add-scope".into(),
            "Register a new scope",
            "projects/demo",
            "Register projects/demo/tools",
            "7f02b1c4",
            "9a1c33e0",
            "done",
            at,
            Some("3f9c2e1".into()),
        );
        assert_eq!(case.id, "add-scope");
        assert_eq!(case.base.as_deref(), Some("3f9c2e1"));
        assert!(case.gate.is_none());
        let origin = case.origin.unwrap();
        assert_eq!(origin.task, "7f02b1c4");
        assert_eq!(origin.run, "9a1c33e0");
        assert_eq!(origin.outcome, "done");
        assert_eq!(origin.recorded_at, at);
    }
}
