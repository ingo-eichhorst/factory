//! Deterministic compilation of the exact text a harness receives.
//!
//! Design §2.5 fixes the order — company context, ancestor scope contexts,
//! current scope context, agent definition, task prompt — and requires the
//! result to be byte-stable for the same scope and task.
//!
//! Per ADR 0013 this crate is a **pure function**: it opens files for reading
//! and creates nothing. Version 1 generates no harness compatibility file, so
//! the design §4 rule that Factory writes only inside `.factory/` holds here by
//! construction rather than by a check somebody has to remember. The
//! `.factory/generated/` rules stay written down and unimplemented; they matter
//! again the moment an adapter needs a shim.
//!
//! Two policies from ADR 0013 that are easy to get backwards:
//!
//! - **A context source that cannot be read is an error, not an empty
//!   section.** Silently skipping a missing `AGENTS.md` produces an agent
//!   running with less instruction than its operator believes it has — mandates
//!   about secrets and approval gates quietly absent, with no symptom until the
//!   agent does something it should have been told not to do. It would also
//!   break byte-stability, since the output would then depend on which files
//!   happen to exist.
//! - **No `[[link]]` is followed.** Context is the `AGENTS.md` chain, the agent
//!   definition, and the task prompt, and nothing else. An unbounded graph walk
//!   is neither bounded nor byte-stable, and bounding it by depth does not fix
//!   the second problem: adding one link to an unrelated note would silently
//!   change what every agent in that subtree receives.

use std::path::{Path, PathBuf};

mod format;

/// One scope's contribution to the compiled context, in root-to-leaf order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeContext {
    /// The scope's name, as it appears in its `config.yaml`.
    pub scope_name: String,
    /// The scope's `AGENTS.md`.
    pub context_file: PathBuf,
}

/// The compiled text together with an account of where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledContext {
    /// Exactly what the harness receives.
    pub text: String,
    /// Every source that contributed, in the order it contributed.
    pub sources: Vec<SourceReport>,
}

/// One line of `factory context show`'s account of the compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReport {
    pub label: String,
    /// `None` for sections that are not files, such as the agent definition.
    pub path: Option<PathBuf>,
    /// Bytes this source contributed to `text`, so an operator diagnosing a
    /// confused agent can see which file made the context large. ADR 0013
    /// sets no size limit — any constant would be invented, since the real
    /// bound is the harness's context window and Factory does not know which
    /// model a harness runs — but the size is never hidden.
    ///
    /// This counts the **whole rendered section** — its rule lines, header,
    /// and blank line, not only the file's own payload. Two things follow
    /// from that choice: it is what an operator actually wants ("what did
    /// this source add to the text I'm sending"), and because `compile`
    /// concatenates sections with no separator of its own, it makes the
    /// reconciliation exact rather than approximate —
    /// `sources.iter().map(|s| s.bytes).sum::<usize>() == text.len()` always,
    /// with no separator-overhead constant to invent or drift out of sync.
    pub bytes: usize,
}

/// Compile the context for a session or a task.
///
/// `scopes` runs root-to-leaf: the company root first, the agent's own scope
/// last. `task_prompt` is `None` when compiling for a session start rather than
/// a task delivery.
///
/// # Output format
///
/// Sections are separated by a rule that cannot be confused with Markdown a
/// source file might contain, since every source *is* Markdown:
///
/// ```text
/// ================================================================================
/// Context: <scope-name>
/// Source: <absolute path>
/// ================================================================================
///
/// <file contents>
/// ```
///
/// then the agent definition:
///
/// ```text
/// ================================================================================
/// Agent definition
/// ================================================================================
///
/// name: <name>
/// harness: <harness>
/// lifetime: <permanent|temporary>
/// max_sessions: <n>
/// ```
///
/// and finally, when present:
///
/// ```text
/// ================================================================================
/// Task
/// ================================================================================
///
/// <prompt>
/// ```
///
/// Byte-stability requires that file contents be normalised to end in exactly
/// one newline, because whether a human's editor left a trailing newline must
/// not change the compiled bytes.
pub fn compile(
    scopes: &[ScopeContext],
    agent: &factory_config::Agent,
    task_prompt: Option<&str>,
) -> Result<CompiledContext, ContextError> {
    if scopes.is_empty() {
        return Err(ContextError::NoScopes);
    }

    let mut text = String::new();
    let mut sources = Vec::with_capacity(scopes.len() + 2);

    // `scopes` arrives already ordered root-to-leaf (design §2.5); we append
    // in the order given rather than sorting it.
    for scope in scopes {
        let raw = std::fs::read_to_string(&scope.context_file).map_err(|source| {
            ContextError::UnreadableSource {
                scope_name: scope.scope_name.clone(),
                path: scope.context_file.clone(),
                help: format!(
                    "check that `{}` exists and is readable by the Factory process",
                    scope.context_file.display()
                ),
                source,
            }
        })?;

        let section = format::scope_section(&scope.scope_name, &scope.context_file, &raw);
        sources.push(SourceReport {
            label: format!("Context: {}", scope.scope_name),
            path: Some(scope.context_file.clone()),
            bytes: section.len(),
        });
        text.push_str(&section);
    }

    let agent_section = format::agent_section(agent);
    sources.push(SourceReport {
        label: "Agent definition".to_string(),
        path: None,
        bytes: agent_section.len(),
    });
    text.push_str(&agent_section);

    if let Some(prompt) = task_prompt {
        let task_section = format::task_section(prompt);
        sources.push(SourceReport {
            label: "Task".to_string(),
            path: None,
            bytes: task_section.len(),
        });
        text.push_str(&task_section);
    }

    Ok(CompiledContext { text, sources })
}

/// Why a compilation stopped.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("cannot read the context file for scope `{scope_name}`: {path}\n  help: {help}")]
    UnreadableSource {
        scope_name: String,
        path: PathBuf,
        help: String,
        #[source]
        source: std::io::Error,
    },

    #[error("no scopes were given; context always begins at the company root")]
    NoScopes,
}

impl ContextError {
    /// The path this error concerns, if it concerns one.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::UnreadableSource { path, .. } => Some(path),
            Self::NoScopes => None,
        }
    }
}
