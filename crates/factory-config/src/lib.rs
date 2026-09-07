//! Parsing and validation of `.factory/config.yaml`.
//!
//! Implements design §2.1 (the scope configuration file) and §2.2 (the agent
//! block, the `agents` list, and `lifetime`) under the policy fixed by ADR 0009:
//!
//! 1. Factory owns exactly `version`, `scope`, and `agent`/`agents`.
//! 2. Unknown **top-level** keys are preserved and ignored — they belong to
//!    other tools. The live `assistant` and `model-lab` scopes carry a
//!    `runtime:` block owned by `ensure_assistant_agents.py`; rejecting it would
//!    make two production scopes unloadable.
//! 3. Unknown fields **inside** Factory-owned mappings are rejected, so
//!    `max_sesions: 4` fails loudly instead of silently meaning `1`.
//! 4. `version` gates only the Factory-owned blocks; an unsupported version is
//!    an actionable error, never a guess.
//! 6. Factory never rewrites a scope config. This crate performs no filesystem
//!    writes at all — see `tests/no_writes.rs`.
//!
//! Values are used verbatim: `${HOME}` and `${REPO_ROOT}` are *not* expanded.
//! That is a feature of `ensure_assistant_agents.py`, and ADR 0009's closing
//! open item keeps it out of Factory version 1.
//!
//! Error rendering is specified in `docs/slice-1-error-corpus.md`, which is the
//! authority for every message this crate produces.

use std::path::{Path, PathBuf};

mod error;
mod harness;
mod raw;
mod validate;

pub use harness::Harness;

/// The configuration version this build understands.
pub const SUPPORTED_VERSION: u32 = 1;

/// A validated scope configuration.
///
/// Both the `agent:` shorthand and the `agents:` list produce this shape: the
/// distinction is a spelling in the file, not a difference in the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeConfig {
    pub version: u32,
    pub scope: Scope,
    /// At least one, in file order. Names are unique within a scope.
    pub agents: Vec<Agent>,
    /// The file this was read from, for diagnostics and provenance.
    pub origin: PathBuf,
    /// Where `scope.id` was declared.
    ///
    /// Carried on the validated config, not just during parsing, because a
    /// duplicate-ID report must point at both files — and Slice 3 will want
    /// the same location when it reports registry drift for a scope whose
    /// config moved. Provenance is part of what a loaded config is, alongside
    /// `origin`.
    pub scope_id_location: Location,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub id: uuid::Uuid,
    pub name: String,
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

/// Read and validate the configuration at `path`.
///
/// `path` is used verbatim as the error-reporting origin: this crate does not
/// canonicalize it (see the crate root docs) — that is the caller's concern.
pub fn load(path: impl AsRef<Path>) -> Result<ScopeConfig, ConfigError> {
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

/// Validate configuration text that was already read.
///
/// `origin` is used for diagnostics only and is never opened, which is what
/// lets the whole test corpus run without touching the filesystem.
pub fn parse(source: &str, origin: impl AsRef<Path>) -> Result<ScopeConfig, ConfigError> {
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
            help: "a scope configuration needs at least `version`, `scope`, and one agent"
                .to_string(),
        });
    }

    let document: raw::RawDocument = serde_saphyr::from_str(source)
        .map_err(|parse_err| error::wrap_parse_error(&parse_err, origin))?;

    validate::validate(document, origin, source)
}

/// Reject two scopes claiming the same UUID.
///
/// The one Slice 1 check that spans files. A scope ID is permanent identity, so
/// a copied `.factory/config.yaml` — the Git-worktree case of design §4 — is
/// caught here, at the point the files are read, rather than in the Slice 3
/// registry.
pub fn validate_unique_ids(configs: &[ScopeConfig]) -> Result<(), ConfigError> {
    for later in 1..configs.len() {
        for earlier in 0..later {
            if configs[later].scope.id == configs[earlier].scope.id {
                return Err(ConfigError {
                    summary: format!(
                        "two scopes share the ID `{}`",
                        configs[later].scope.id
                    ),
                    location: configs[later].scope_id_location.clone(),
                    notes: vec![Note {
                        message: format!(
                            "also used by scope `{}`",
                            configs[earlier].scope.name
                        ),
                        location: Some(configs[earlier].scope_id_location.clone()),
                    }],
                    help: "a scope ID is permanent identity; if this file was copied, generate a new ID with `uuidgen`".to_string(),
                });
            }
        }
    }
    Ok(())
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
