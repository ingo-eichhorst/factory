//! Parsing and validation of the Factory instance's `.factory/config.yaml`.
//!
//! Implements ADR 0015 (a Factory instance holds one configuration file
//! listing every registered scope) and design §2.2 (the agent block, the
//! `agents` list, and `lifetime`) under the policy fixed by ADR 0009:
//!
//! 1. Factory owns exactly `version`, `instance`, `scopes`, and — within each
//!    scope entry — `id`, `name`, `path`, `git`, and `agent`/`agents`.
//! 2. Unknown **top-level** keys are preserved and ignored — they belong to
//!    other tools. The live `assistant` scope's own file carries a `runtime:`
//!    block owned by `ensure_assistant_agents.py`, and that block is a
//!    top-level sibling of `version`/`instance`/`scopes` wherever it appears;
//!    rejecting it would make that tool's configuration unloadable.
//! 3. Unknown fields **inside** Factory-owned mappings are rejected, so
//!    `max_sesions: 4` fails loudly instead of silently meaning `1`. This
//!    applies to a scope entry itself as well as to `agent`/`agents`: a scope
//!    entry cannot carry an ad-hoc extra key such as `runtime:` the way the
//!    old per-scope file's top level could.
//! 4. `version` gates only the Factory-owned blocks; an unsupported version is
//!    an actionable error, never a guess.
//! 5. Factory never writes a scope's `path`. This crate performs no
//!    filesystem writes at all — see `tests/no_writes.rs`.
//!
//! `path` is kept exactly as written and never canonicalized or resolved
//! against a filesystem: this crate does no filesystem access beyond reading
//! the one instance file. Resolving a scope's `path` to a real, existing
//! directory is Slice 3's job (`factory-paths` already owns that mechanism),
//! so two scopes naming textually different but filesystem-equivalent paths
//! (`projects/x` and `./projects/x`, say) are not caught here.
//!
//! Values are used verbatim: `${HOME}` and `${REPO_ROOT}` are *not* expanded.
//! That is a feature of `ensure_assistant_agents.py`, and ADR 0009's closing
//! open item keeps it out of Factory version 1.
//!
//! Error rendering is specified in `docs/slice-1-error-corpus.md`, which is the
//! authority for every message this crate produces. ADR 0015 amended it:
//! errors about one scope entry name that scope, and duplicate-ID detection
//! (plus a new duplicate-`path` check) is now an intra-file, every-load check
//! rather than a separate cross-file entry point.
//!
//! ## Validation order
//!
//! `parse` checks, in this order: the configuration `version`; `instance.id`
//! as a UUID; each scope entry in file order (its `id` as a UUID, its
//! `agent`/`agents` resolution, then each agent's `harness`/`lifetime`/
//! `max_sessions`, then duplicate agent names within that entry); and finally,
//! once every entry is individually valid, duplicate scope IDs and duplicate
//! scope paths across the whole file. Every corpus fixture triggers exactly
//! one problem, so this order is not exercised by the tests — it is recorded
//! here because *some* order has to be picked and future fixtures may depend
//! on it.

use std::path::{Path, PathBuf};

mod error;
mod harness;
mod raw;
mod validate;

pub use harness::Harness;

/// The configuration version this build understands.
pub const SUPPORTED_VERSION: u32 = 1;

/// A validated Factory instance configuration: the instance itself and every
/// scope registered with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceConfig {
    pub version: u32,
    pub instance: Instance,
    /// In file order. May be empty: a freshly initialized instance has no
    /// scopes yet, and nothing about the shape requires at least one.
    pub scopes: Vec<ScopeEntry>,
    /// The file this was read from, for diagnostics and provenance.
    pub origin: PathBuf,
}

/// The Factory instance itself, per ADR 0015.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub id: uuid::Uuid,
    pub name: String,
}

/// One entry in the instance's `scopes:` list.
///
/// Both the `agent:` shorthand and the `agents:` list produce the same
/// `agents` shape: the distinction is a spelling in the file, not a
/// difference in the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeEntry {
    pub id: uuid::Uuid,
    pub name: String,
    /// Relative to the instance root, exactly as written — never
    /// canonicalized (see the crate root docs).
    pub path: PathBuf,
    /// Present when the project is its own repository, checked out at
    /// `path`; absent when the project's files live in the instance's own
    /// repository. ADR 0015 validates only that this is present or absent —
    /// version 1 never contacts a remote or parses the URL.
    pub git: Option<String>,
    /// At least one, in file order. Names are unique within a scope.
    pub agents: Vec<Agent>,
    /// Where this entry's `id` was declared.
    ///
    /// Carried on the validated entry, not just during parsing, because a
    /// duplicate-ID report must point at both entries — and Slice 3 will want
    /// the same location when it reports registry drift for a scope whose
    /// entry moved. Provenance is part of what a loaded entry is.
    pub id_location: Location,
    /// Where this entry's `path` was declared, for the same reason.
    pub path_location: Location,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// Unique within the scope: `factory task send` addresses an agent by name.
    pub name: String,
    pub harness: Harness,
    /// Bounds this agent's parallelism. Defaults to 1, never 0.
    ///
    /// Per Slice 1 this is per *agent*: version 1 has no scope-level aggregate
    /// cap, so a scope's session bound is the sum of its agents' limits. For a
    /// `temporary` agent it caps concurrently live instances.
    pub max_sessions: u32,
    pub lifetime: Lifetime,
    /// The model this agent's harness is started with, passed straight
    /// through to the harness as its own `--model` argument. `None` leaves
    /// the choice where it was before this field existed: with the harness's
    /// own configuration.
    ///
    /// Deliberately a plain `String` and never an enum. Factory names a
    /// model; the harness resolves it. A catalogue here would be a second
    /// copy of every provider's model list, out of date the week it was
    /// written, and it would reject a name the harness would have accepted.
    /// ADR 0024.
    pub model: Option<String>,
}

/// Design §2.2. ADR 0010 records that these name the same distinction as
/// ADR 0001's "thread agent" and "task agent".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lifetime {
    /// A standing part of the scope, restarted after a crash.
    #[default]
    Permanent,
    /// Created for one task and torn down when that task reaches a terminal
    /// state. Carries no identity across tasks.
    Temporary,
}

/// Read and validate the instance configuration at `path`.
///
/// `path` is used verbatim as the error-reporting origin: this crate does not
/// canonicalize it (see the crate root docs) — that is the caller's concern.
pub fn load(path: impl AsRef<Path>) -> Result<InstanceConfig, ConfigError> {
    let path = path.as_ref();
    let source = std::fs::read_to_string(path).map_err(|io_err| ConfigError {
        summary: format!("could not read `{}`: {io_err}", path.display()),
        location: Location {
            file: path.to_path_buf(),
            line: 1,
            column: 1,
        },
        notes: Vec::new(),
        help: "check that the file exists and is readable".to_string(),
    })?;
    parse(&source, path)
}

/// Validate instance configuration text that was already read.
///
/// `origin` is used for diagnostics only and is never opened, which is what
/// lets the whole test corpus run without touching the filesystem.
pub fn parse(source: &str, origin: impl AsRef<Path>) -> Result<InstanceConfig, ConfigError> {
    let origin = origin.as_ref();

    if source.trim().is_empty() {
        return Err(ConfigError {
            summary: "the file is empty".to_string(),
            location: Location {
                file: origin.to_path_buf(),
                line: 1,
                column: 1,
            },
            notes: Vec::new(),
            help: "an instance configuration needs at least `version`, `instance`, and `scopes`"
                .to_string(),
        });
    }

    let document: raw::RawDocument = serde_saphyr::from_str(source)
        .map_err(|parse_err| error::wrap_parse_error(&parse_err, origin))?;

    validate::validate(document, origin, source)
}

/// A configuration problem, rendered per `docs/slice-1-error-corpus.md`.
///
/// `Display` produces exactly:
///
/// ```text
/// error: <summary>
///   --> <file>:<line>:<column>
///   note: <secondary location>
///   help: <corrective action>
/// ```
///
/// The file is always the real path. `serde-saphyr` renders its source as
/// `<input>`, so parse errors are wrapped with the origin rather than
/// forwarded — Slice 1 requires every failure to name the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub summary: String,
    pub location: Location,
    /// Additional locations belonging to the same problem. A file setting both
    /// `agent` and `agents`, or two agents sharing a name, names both places.
    pub notes: Vec<Note>,
    /// Always present: an action, not a restatement of the problem.
    pub help: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub file: PathBuf,
    /// 1-based, as reported by `serde-saphyr`.
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub message: String,
    /// `None` when the note has no second location of its own.
    pub location: Option<Location>,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "error: {}", self.summary)?;
        writeln!(
            f,
            "  --> {}:{}:{}",
            self.location.file.display(),
            self.location.line,
            self.location.column
        )?;
        for note in &self.notes {
            match &note.location {
                Some(location) => writeln!(
                    f,
                    "  note: {} at {}:{}:{}",
                    note.message,
                    location.file.display(),
                    location.line,
                    location.column
                )?,
                None => writeln!(f, "  note: {}", note.message)?,
            }
        }
        write!(f, "  help: {}", self.help)
    }
}

impl std::error::Error for ConfigError {}
