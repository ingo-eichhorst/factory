//! Mutations of the local, scope-owned Factory configuration.
//!
//! Discovery reads these files at startup. Roster edits use the same files as
//! their source of truth, replace one atomically, and then update the Engine's
//! configuration snapshot so the declaration is usable without a restart.

use factory_core::agent::Lifetime;
use factory_core::config::{
    refuse_misplaced_scope_policies, refuse_misplaced_scope_roles, AgentRef, Config, Scope, ScopeAgent,
    CONFIG_FILE, FACTORY_DIR,
};
use factory_core::error::{FactoryError, Result};
use factory_core::role::{Role, RoleOrigin, RoleSpec};
use serde::Deserialize;
use serde_yaml_ng::{Mapping, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::engine::Engine;

#[derive(Deserialize)]
struct ScopeFile {
    scope: Scope,
}

fn bad(message: impl Into<String>) -> FactoryError {
    FactoryError::BadRequest(message.into())
}

fn read_document(path: &Path) -> Result<(String, Value, Scope)> {
    let text = fs::read_to_string(path).map_err(|error| {
        FactoryError::Other(anyhow::anyhow!(
            "reading scope config {}: {error}",
            path.display()
        ))
    })?;
    let document: Value = serde_yaml_ng::from_str(&text)
        .map_err(|error| bad(format!("parsing scope config {}: {error}", path.display())))?;
    refuse_misplaced_scope_roles(&document, path)?;
    refuse_misplaced_scope_policies(&document, path)?;
    // Deliberately no `refuse_misplaced_scope_infrastructure` here, unlike
    // discovery's `read_scope`: this also reads the instance root's own file
    // when the roster edits the root scope, and that file is exactly where
    // `infrastructure:` belongs. Discovery never reads the root through its
    // reader, so it can refuse the block outright.
    let parsed: ScopeFile = serde_yaml_ng::from_str(&text)
        .map_err(|error| bad(format!("parsing scope config {}: {error}", path.display())))?;
    Ok((text, document, parsed.scope))
}

fn mapping<'a>(value: &'a mut Value, what: &str, path: &Path) -> Result<&'a mut Mapping> {
    value.as_mapping_mut().ok_or_else(|| {
        bad(format!(
            "{what} in scope config {} must be a mapping",
            path.display()
        ))
    })
}

fn append_agent(document: &mut Value, agent: &ScopeAgent, path: &Path) -> Result<()> {
    let root = mapping(document, "the document", path)?;
    let scope_key = Value::String("scope".into());
    let scope = root.get_mut(&scope_key).ok_or_else(|| {
        bad(format!(
            "scope config {} has no scope block",
            path.display()
        ))
    })?;
    let scope = mapping(scope, "scope", path)?;
    let agents_key = Value::String("agents".into());
    let encoded = serde_yaml_ng::to_value(agent).map_err(|error| {
        FactoryError::Other(anyhow::anyhow!("encoding agent declaration: {error}"))
    })?;
    match scope.get_mut(&agents_key) {
        Some(Value::Sequence(agents)) => agents.push(encoded),
        Some(_) => {
            return Err(bad(format!(
                "scope.agents in {} must be a list",
                path.display()
            )))
        }
        None => {
            scope.insert(agents_key, Value::Sequence(vec![encoded]));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum AgentLocation {
    Singular,
    List(usize),
}

fn find_agent(scope: &Scope, name: &str) -> Option<(ScopeAgent, AgentLocation)> {
    if matches!(scope.agent, Some(AgentRef::Declared { .. })) {
        if let Some(agent) = scope.declared_agents().into_iter().next() {
            if agent.name() == name {
                return Some((agent, AgentLocation::Singular));
            }
        }
    }
    scope
        .agents
        .iter()
        .enumerate()
        .find(|(_, agent)| agent.name() == name)
        .map(|(index, agent)| (agent.clone(), AgentLocation::List(index)))
}

fn remove_agent(document: &mut Value, location: AgentLocation, path: &Path) -> Result<()> {
    let root = mapping(document, "the document", path)?;
    let scope = root.get_mut(Value::String("scope".into())).ok_or_else(|| {
        bad(format!(
            "scope config {} has no scope block",
            path.display()
        ))
    })?;
    let scope = mapping(scope, "scope", path)?;
    match location {
        AgentLocation::Singular => {
            scope.remove(Value::String("agent".into()));
        }
        AgentLocation::List(index) => {
            let key = Value::String("agents".into());
            let agents = scope
                .get_mut(&key)
                .and_then(Value::as_sequence_mut)
                .ok_or_else(|| bad(format!("scope.agents in {} must be a list", path.display())))?;
            if index >= agents.len() {
                return Err(bad(format!(
                    "scope.agents in {} changed while it was being edited",
                    path.display()
                )));
            }
            agents.remove(index);
            if agents.is_empty() {
                scope.remove(&key);
            }
        }
    }
    Ok(())
}

fn indentation(line: &str) -> Option<usize> {
    let prefix = line.len() - line.trim_start_matches(' ').len();
    (!line[prefix..].starts_with('\t')).then_some(prefix)
}

fn key_rest<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let line = line.trim_end_matches(['\r', '\n']);
    let content = line.trim_start_matches(' ');
    let rest = content.strip_prefix(key)?.strip_prefix(':')?;
    Some(rest)
}

fn insert_at(text: &str, offset: usize, addition: &str) -> String {
    let mut out = String::with_capacity(text.len() + addition.len() + 1);
    out.push_str(&text[..offset]);
    if offset > 0 && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(addition);
    out.push_str(&text[offset..]);
    out
}

fn rendered_item(agent: &ScopeAgent, indent: usize) -> Result<String> {
    let yaml = serde_yaml_ng::to_string(agent).map_err(|error| {
        FactoryError::Other(anyhow::anyhow!("encoding agent declaration: {error}"))
    })?;
    let mut lines = yaml.lines();
    let Some(first) = lines.next() else {
        return Err(FactoryError::Other(anyhow::anyhow!(
            "encoding agent declaration produced no YAML"
        )));
    };
    let mut out = format!("{}- {first}\n", " ".repeat(indent));
    for line in lines {
        out.push_str(&" ".repeat(indent + 2));
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

fn rendered_scope(scope: &Value, comment: &str, path: &Path) -> Result<String> {
    let yaml = serde_yaml_ng::to_string(scope).map_err(|error| {
        FactoryError::Other(anyhow::anyhow!(
            "encoding scope config {}: {error}",
            path.display()
        ))
    })?;
    let mut replacement = format!("scope:{comment}\n");
    for line in yaml.lines() {
        replacement.push_str("  ");
        replacement.push_str(line);
        replacement.push('\n');
    }
    Ok(replacement)
}

/// Add to an ordinary block-style `scope.agents` list without rewriting the
/// rest of a file a person owns. Flow-style scope declarations are uncommon
/// but valid; for those, only the one `scope:` line is expanded to a block.
fn edit_text(text: &str, document: &Value, agent: &ScopeAgent, path: &Path) -> Result<String> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        lines.push((offset, offset + line.len(), line));
        offset += line.len();
    }
    if offset < text.len() || text.is_empty() {
        lines.push((offset, text.len(), &text[offset..]));
    }

    let scope_index = lines
        .iter()
        .position(|(_, _, line)| indentation(line) == Some(0) && key_rest(line, "scope").is_some())
        .ok_or_else(|| {
            bad(format!(
                "scope config {} has no top-level scope block",
                path.display()
            ))
        })?;
    let scope_rest = key_rest(lines[scope_index].2, "scope").unwrap_or_default();
    let block_scope = scope_rest.trim().is_empty() || scope_rest.trim_start().starts_with('#');

    if !block_scope {
        let root = document.as_mapping().ok_or_else(|| {
            bad(format!(
                "the document in {} must be a mapping",
                path.display()
            ))
        })?;
        let scope = root.get(Value::String("scope".into())).ok_or_else(|| {
            bad(format!(
                "scope config {} has no scope block",
                path.display()
            ))
        })?;
        let comment = scope_rest
            .find('#')
            .map(|at| format!(" {}", scope_rest[at..].trim()))
            .unwrap_or_default();
        let replacement = rendered_scope(scope, &comment, path)?;
        let (start, end, _) = lines[scope_index];
        let mut out = String::with_capacity(text.len() + replacement.len());
        out.push_str(&text[..start]);
        out.push_str(&replacement);
        out.push_str(&text[end..]);
        return Ok(out);
    }

    let scope_end = lines
        .iter()
        .enumerate()
        .skip(scope_index + 1)
        .find(|(_, (_, _, line))| {
            let trimmed = line.trim();
            !trimmed.is_empty() && indentation(line) == Some(0)
        })
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let child_indent = lines[scope_index + 1..scope_end]
        .iter()
        .filter_map(|(_, _, line)| {
            let trimmed = line.trim();
            (!trimmed.is_empty() && !trimmed.starts_with('#'))
                .then(|| indentation(line))
                .flatten()
        })
        .min()
        .unwrap_or(2);
    let agents_index = lines[scope_index + 1..scope_end]
        .iter()
        .position(|(_, _, line)| {
            indentation(line) == Some(child_indent) && key_rest(line, "agents").is_some()
        })
        .map(|index| index + scope_index + 1);

    let Some(agents_index) = agents_index else {
        let at = if scope_end < lines.len() {
            lines[scope_end].0
        } else {
            text.len()
        };
        let addition = format!(
            "{}agents:\n{}",
            " ".repeat(child_indent),
            rendered_item(agent, child_indent + 2)?
        );
        return Ok(insert_at(text, at, &addition));
    };

    let agents_rest = key_rest(lines[agents_index].2, "agents").unwrap_or_default();
    if !agents_rest.trim().is_empty() && !agents_rest.trim_start().starts_with('#') {
        // Expand an inline list locally, preserving every line outside it.
        let root = document.as_mapping().unwrap();
        let scope = root
            .get(Value::String("scope".into()))
            .and_then(Value::as_mapping)
            .unwrap();
        let agents = scope
            .get(Value::String("agents".into()))
            .and_then(Value::as_sequence)
            .ok_or_else(|| bad(format!("scope.agents in {} must be a list", path.display())))?;
        let mut replacement = format!("{}agents:\n", " ".repeat(child_indent));
        for value in agents {
            let decoded: ScopeAgent =
                serde_yaml_ng::from_value(value.clone()).map_err(|error| {
                    bad(format!(
                        "parsing scope.agents in {}: {error}",
                        path.display()
                    ))
                })?;
            replacement.push_str(&rendered_item(&decoded, child_indent + 2)?);
        }
        let (start, end, _) = lines[agents_index];
        let mut out = String::with_capacity(text.len() + replacement.len());
        out.push_str(&text[..start]);
        out.push_str(&replacement);
        out.push_str(&text[end..]);
        return Ok(out);
    }

    let list_end = lines
        .iter()
        .enumerate()
        .take(scope_end)
        .skip(agents_index + 1)
        .find(|(_, (_, _, line))| {
            let trimmed = line.trim();
            !trimmed.is_empty() && indentation(line).is_some_and(|indent| indent <= child_indent)
        })
        .map(|(index, _)| index)
        .unwrap_or(scope_end);
    let item_indent = lines[agents_index + 1..list_end]
        .iter()
        .find_map(|(_, _, line)| {
            line.trim_start()
                .starts_with('-')
                .then(|| indentation(line))
                .flatten()
        })
        .unwrap_or(child_indent + 2);
    let at = if list_end < lines.len() {
        lines[list_end].0
    } else {
        text.len()
    };
    Ok(insert_at(text, at, &rendered_item(agent, item_indent)?))
}

/// Remove exactly one declaration while leaving sibling settings and agent
/// declarations byte-for-byte alone. A flow-style scope or inline list has no
/// independent line range to remove, so only that local value is expanded.
fn remove_text(
    text: &str,
    document: &Value,
    location: AgentLocation,
    path: &Path,
) -> Result<String> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        lines.push((offset, offset + line.len(), line));
        offset += line.len();
    }
    if offset < text.len() || text.is_empty() {
        lines.push((offset, text.len(), &text[offset..]));
    }

    let scope_index = lines
        .iter()
        .position(|(_, _, line)| indentation(line) == Some(0) && key_rest(line, "scope").is_some())
        .ok_or_else(|| {
            bad(format!(
                "scope config {} has no top-level scope block",
                path.display()
            ))
        })?;
    let scope_rest = key_rest(lines[scope_index].2, "scope").unwrap_or_default();
    let block_scope = scope_rest.trim().is_empty() || scope_rest.trim_start().starts_with('#');
    if !block_scope {
        let scope = document
            .as_mapping()
            .and_then(|root| root.get(Value::String("scope".into())))
            .ok_or_else(|| {
                bad(format!(
                    "scope config {} has no scope block",
                    path.display()
                ))
            })?;
        let comment = scope_rest
            .find('#')
            .map(|at| format!(" {}", scope_rest[at..].trim()))
            .unwrap_or_default();
        let replacement = rendered_scope(scope, &comment, path)?;
        let (start, end, _) = lines[scope_index];
        return Ok(format!("{}{}{}", &text[..start], replacement, &text[end..]));
    }

    let scope_end = lines
        .iter()
        .enumerate()
        .skip(scope_index + 1)
        .find(|(_, (_, _, line))| !line.trim().is_empty() && indentation(line) == Some(0))
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let child_indent = lines[scope_index + 1..scope_end]
        .iter()
        .filter_map(|(_, _, line)| {
            let trimmed = line.trim();
            (!trimmed.is_empty() && !trimmed.starts_with('#'))
                .then(|| indentation(line))
                .flatten()
        })
        .min()
        .unwrap_or(2);
    let key = match location {
        AgentLocation::Singular => "agent",
        AgentLocation::List(_) => "agents",
    };
    let key_index = lines[scope_index + 1..scope_end]
        .iter()
        .position(|(_, _, line)| {
            indentation(line) == Some(child_indent) && key_rest(line, key).is_some()
        })
        .map(|index| index + scope_index + 1)
        .ok_or_else(|| bad(format!("scope.{key} is missing from {}", path.display())))?;

    let value_end = lines
        .iter()
        .enumerate()
        .take(scope_end)
        .skip(key_index + 1)
        .find(|(_, (_, _, line))| {
            !line.trim().is_empty()
                && indentation(line).is_some_and(|indent| indent <= child_indent)
        })
        .map(|(index, _)| index)
        .unwrap_or(scope_end);

    if matches!(location, AgentLocation::Singular) {
        let start = lines[key_index].0;
        let end = if value_end < lines.len() {
            lines[value_end].0
        } else {
            text.len()
        };
        return Ok(format!("{}{}", &text[..start], &text[end..]));
    }

    let AgentLocation::List(target) = location else {
        unreachable!()
    };
    let rest = key_rest(lines[key_index].2, "agents").unwrap_or_default();
    if !rest.trim().is_empty() && !rest.trim_start().starts_with('#') {
        let agents = document
            .as_mapping()
            .and_then(|root| root.get(Value::String("scope".into())))
            .and_then(Value::as_mapping)
            .and_then(|scope| scope.get(Value::String("agents".into())))
            .and_then(Value::as_sequence);
        let replacement = if let Some(agents) = agents {
            let comment = rest
                .find('#')
                .map(|at| format!(" {}", rest[at..].trim()))
                .unwrap_or_default();
            let mut rendered = format!("{}agents:{comment}\n", " ".repeat(child_indent));
            for value in agents {
                let decoded: ScopeAgent =
                    serde_yaml_ng::from_value(value.clone()).map_err(|error| {
                        bad(format!(
                            "parsing scope.agents in {}: {error}",
                            path.display()
                        ))
                    })?;
                rendered.push_str(&rendered_item(&decoded, child_indent + 2)?);
            }
            rendered
        } else {
            String::new()
        };
        let (start, end, _) = lines[key_index];
        return Ok(format!("{}{}{}", &text[..start], replacement, &text[end..]));
    }

    let item_indent = lines[key_index + 1..value_end]
        .iter()
        .find_map(|(_, _, line)| {
            line.trim_start()
                .starts_with('-')
                .then(|| indentation(line))
                .flatten()
        })
        .ok_or_else(|| {
            bad(format!(
                "scope.agents in {} has no list items",
                path.display()
            ))
        })?;
    let items: Vec<usize> = (key_index + 1..value_end)
        .filter(|index| {
            indentation(lines[*index].2) == Some(item_indent)
                && lines[*index].2.trim_start().starts_with('-')
        })
        .collect();
    let item = *items.get(target).ok_or_else(|| {
        bad(format!(
            "scope.agents in {} changed while it was being edited",
            path.display()
        ))
    })?;
    if items.len() == 1 {
        let rest = key_rest(lines[key_index].2, "agents").unwrap_or_default();
        let comment = rest
            .find('#')
            .map(|at| format!(" {}", rest[at..].trim()))
            .unwrap_or_default();
        let replacement = format!("{}agents: []{comment}\n", " ".repeat(child_indent));
        let start = lines[key_index].0;
        let end = if value_end < lines.len() {
            lines[value_end].0
        } else {
            text.len()
        };
        return Ok(format!("{}{}{}", &text[..start], replacement, &text[end..]));
    }

    let next = items
        .iter()
        .copied()
        .find(|index| *index > item)
        .unwrap_or(value_end);
    let start = lines[item].0;
    let end = if next < lines.len() {
        lines[next].0
    } else {
        text.len()
    };
    Ok(format!("{}{}", &text[..start], &text[end..]))
}

/// Replace `path` without ever exposing a truncated or half-written config.
fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        FactoryError::Other(anyhow::anyhow!(
            "scope config {} has no parent",
            path.display()
        ))
    })?;
    let temporary = parent.join(format!(".{CONFIG_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    let permissions = fs::metadata(path)
        .map(|metadata| metadata.permissions())
        .map_err(|error| {
            FactoryError::Other(anyhow::anyhow!(
                "reading permissions for {}: {error}",
                path.display()
            ))
        })?;

    let written = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.set_permissions(permissions)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(FactoryError::Other(anyhow::anyhow!(
            "writing scope config {}: {error}",
            path.display()
        )));
    }
    Ok(())
}

impl Engine {
    /// Append one declaration to a scope's own config and make it immediately
    /// visible to the running daemon. The edit mutex keeps simultaneous roster
    /// submissions from both reading the same old file and losing one another.
    pub(crate) fn configure_agent(
        &self,
        scope_name: &str,
        mut agent: ScopeAgent,
    ) -> Result<(String, ScopeAgent)> {
        let _edit = self
            .configuration_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let factory = self.factory_snapshot();
        let current = factory.scope(scope_name)?.clone();
        let directory = factory.scope_path(&current.name)?;
        let path = directory.join(FACTORY_DIR).join(CONFIG_FILE);
        let (text, mut document, mut from_file) = read_document(&path)?;

        // The path came from the discovered scope, and the identity in that
        // marker must still name the same scope. An external rename needs a
        // full rediscovery, not an edit applied under the old identity.
        if from_file.id != current.id || from_file.name != current.name {
            return Err(bad(format!(
                "scope config {} changed identity since startup; restart Factory before editing it",
                path.display()
            )));
        }
        from_file.path = current.path.clone();

        agent.harness = agent.harness.trim().to_string();
        if agent.harness.is_empty() {
            return Err(bad("an agent needs a harness"));
        }
        agent.name = match agent.name.take() {
            Some(name) if !name.trim().is_empty() => Some(name.trim().to_string()),
            _ => None,
        };
        let name = agent.name();
        if name.contains('/') || name.chars().any(char::is_control) {
            return Err(bad(
                "an agent name cannot contain `/` or control characters",
            ));
        }
        self.registry.agent(&agent.harness)?;
        let roles = factory.config.roles_for_scope(&from_file)?;
        if !roles.contains(&agent.role) {
            return Err(bad(format!(
                "no role named {:?} in {}. The roles available there are: {}",
                agent.role.as_str(),
                current.name,
                roles.names().join(", ")
            )));
        }
        if agent.harness == "shell" && !agent.args.is_empty() {
            return Err(bad(
                "the shell agent runs task instructions directly and cannot use CLI arguments",
            ));
        }
        // The same check the next start would make, made before the file is
        // written: a roster edit must never leave a config the daemon then
        // refuses to load.
        agent.provider = agent
            .provider
            .take()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());
        factory
            .config
            .infrastructure
            .refuse_unknown_provider(&from_file, &agent)?;
        if agent.lifetime == Lifetime::Task && agent.autostart.is_some() {
            return Err(bad("a task agent cannot set autostart"));
        }
        if from_file
            .agents_with(&factory.config.daemon.foreman)
            .iter()
            .any(|declared| declared.name() == name)
        {
            return Err(bad(format!(
                "scope {:?} already declares an agent named {:?}",
                current.name, name
            )));
        }

        append_agent(&mut document, &agent, &path)?;
        from_file.agents.push(agent.clone());
        let serialized = edit_text(&text, &document, &agent, &path)?;
        atomic_write(&path, &serialized)?;
        self.replace_scope(&current.id, from_file);
        Ok((current.name, agent))
    }

    /// Remove one declaration from a scope's own config and live snapshot.
    /// The caller reconciles standing sessions after the file is safely on
    /// disk, so a failed edit can never stop an agent it did not delete.
    pub(crate) fn delete_agent_declaration(
        &self,
        scope_name: &str,
        name: &str,
    ) -> Result<(String, ScopeAgent)> {
        if name.is_empty() {
            return Err(bad("an agent deletion needs a name"));
        }
        let _edit = self
            .configuration_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let factory = self.factory_snapshot();
        let current = factory.scope(scope_name)?.clone();
        let directory = factory.scope_path(&current.name)?;
        let path = directory.join(FACTORY_DIR).join(CONFIG_FILE);
        let (text, mut document, mut from_file) = read_document(&path)?;
        if from_file.id != current.id || from_file.name != current.name {
            return Err(bad(format!(
                "scope config {} changed identity since startup; restart Factory before editing it",
                path.display()
            )));
        }
        from_file.path = current.path.clone();

        let (removed, location) = find_agent(&from_file, name).ok_or_else(|| {
            bad(format!(
                "scope {:?} has no local agent declaration named {:?}",
                current.name, name
            ))
        })?;
        remove_agent(&mut document, location, &path)?;
        match location {
            AgentLocation::Singular => from_file.agent = None,
            AgentLocation::List(index) => {
                from_file.agents.remove(index);
            }
        }
        let serialized = remove_text(&text, &document, location, &path)?;
        atomic_write(&path, &serialized)?;
        self.replace_scope(&current.id, from_file);
        Ok((current.name, removed))
    }
}

// -- roles ------------------------------------------------------------------
//
// A role layer is one mapping in one file: `scope.roles` in a nested scope's
// own config, or the top-level `roles:` in the instance root's. Edits splice
// exactly one entry of that mapping and leave every other line a person wrote
// where it was, the same promise the agent edits above make. Every splice is
// parsed back before it is written, and refused unless the file then says
// exactly what was asked: a text edit that guessed wrong about somebody's
// YAML must fail loudly, never land.

/// Which of the two places a layer is written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RoleFile {
    /// The instance root's config: top-level `roles:`.
    Root,
    /// A nested scope's own config: `scope.roles`.
    Scope,
}

/// Names an agent's `role:` is matched against exactly, typed by people into
/// YAML, into the CLI and into a URL. Kept to what never needs quoting.
fn valid_role_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn line_table(text: &str) -> Vec<(usize, usize, &str)> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        lines.push((offset, offset + line.len(), line));
        offset += line.len();
    }
    lines
}

/// A line that says something: not blank, and not only a comment.
fn is_content(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && !trimmed.starts_with('#')
}

fn offset_of(lines: &[(usize, usize, &str)], text: &str, index: usize) -> usize {
    lines.get(index).map(|(start, _, _)| *start).unwrap_or(text.len())
}

/// The last line in `from..to` that says something, if any does.
fn last_content(lines: &[(usize, usize, &str)], from: usize, to: usize) -> Option<usize> {
    (from..to).rev().find(|index| is_content(lines[*index].2))
}

/// The first line after `start`, before `to`, that says something at or left
/// of `indent` -- where the block `start` opens ends.
fn block_end(lines: &[(usize, usize, &str)], start: usize, to: usize, indent: usize) -> usize {
    (start + 1..to)
        .find(|index| {
            let line = lines[*index].2;
            is_content(line) && indentation(line).is_some_and(|at| at <= indent)
        })
        .unwrap_or(to)
}

/// Whether `line` opens the mapping entry `name`, bare or quoted.
fn opens_entry(line: &str, name: &str) -> bool {
    [name.to_string(), format!("\"{name}\""), format!("'{name}'")]
        .iter()
        .any(|key| key_rest(line, key).is_some())
}

fn rendered_role(name: &str, spec: &RoleSpec, indent: usize) -> Result<String> {
    let yaml = serde_yaml_ng::to_string(&BTreeMap::from([(name, spec)])).map_err(|error| {
        FactoryError::Other(anyhow::anyhow!("encoding role {name:?}: {error}"))
    })?;
    let pad = " ".repeat(indent);
    Ok(yaml.lines().map(|line| format!("{pad}{line}\n")).collect())
}

fn rendered_roles(roles: &BTreeMap<String, RoleSpec>, indent: usize) -> Result<String> {
    let mut out = format!("{}roles:\n", " ".repeat(indent));
    for (name, spec) in roles {
        out.push_str(&rendered_role(name, spec, indent + 2)?);
    }
    Ok(out)
}

/// Set (`Some`) or remove (`None`) one entry of the `roles:` mapping that sits
/// at `indent` among `lines[from..to]`. `after` is the whole mapping once the
/// change is made, for the one case with no line of its own to edit: a
/// flow-style `roles: { … }`, which is expanded to a block.
#[allow(clippy::too_many_arguments)]
fn splice_roles_block(
    text: &str,
    lines: &[(usize, usize, &str)],
    from: usize,
    to: usize,
    indent: usize,
    name: &str,
    spec: Option<&RoleSpec>,
    after: &BTreeMap<String, RoleSpec>,
) -> Result<String> {
    let pad = " ".repeat(indent);
    let Some(roles_at) = (from..to).find(|index| {
        let line = lines[*index].2;
        indentation(line) == Some(indent) && key_rest(line, "roles").is_some()
    }) else {
        let Some(spec) = spec else {
            return Err(bad(format!("there is no roles block to remove {name:?} from")));
        };
        // No block yet: open one after the last thing this level says, so it
        // lands inside the scope rather than after a comment meant for
        // whatever follows it.
        let at = last_content(lines, from, to)
            .map(|index| offset_of(lines, text, index + 1))
            .unwrap_or_else(|| offset_of(lines, text, from));
        let addition = format!("{pad}roles:\n{}", rendered_role(name, spec, indent + 2)?);
        return Ok(insert_at(text, at, &addition));
    };

    let rest = key_rest(lines[roles_at].2, "roles").unwrap_or_default();
    if !rest.trim().is_empty() && !rest.trim_start().starts_with('#') {
        let replacement = if after.is_empty() {
            String::new()
        } else {
            rendered_roles(after, indent)?
        };
        let (start, end, _) = lines[roles_at];
        return Ok(format!("{}{}{}", &text[..start], replacement, &text[end..]));
    }

    let end = block_end(lines, roles_at, to, indent);
    let entry_indent = (roles_at + 1..end)
        .filter(|index| is_content(lines[*index].2))
        .filter_map(|index| indentation(lines[index].2))
        .min()
        .unwrap_or(indent + 2);
    let entry_at = (roles_at + 1..end).find(|index| {
        let line = lines[*index].2;
        indentation(line) == Some(entry_indent) && opens_entry(line, name)
    });

    match (entry_at, spec) {
        (Some(entry), Some(spec)) => {
            let entry_end = block_end(lines, entry, end, entry_indent);
            let last = last_content(lines, entry, entry_end).unwrap_or(entry);
            let start = offset_of(lines, text, entry);
            let stop = offset_of(lines, text, last + 1);
            Ok(format!(
                "{}{}{}",
                &text[..start],
                rendered_role(name, spec, entry_indent)?,
                &text[stop..]
            ))
        }
        (None, Some(spec)) => {
            let last = last_content(lines, roles_at, end).unwrap_or(roles_at);
            let at = offset_of(lines, text, last + 1);
            Ok(insert_at(text, at, &rendered_role(name, spec, entry_indent)?))
        }
        (Some(entry), None) => {
            // The last entry takes the block with it, rather than leaving a
            // `roles:` that parses as nothing at all.
            let (start, last) = if after.is_empty() {
                (roles_at, last_content(lines, roles_at, end).unwrap_or(roles_at))
            } else {
                let entry_end = block_end(lines, entry, end, entry_indent);
                (entry, last_content(lines, entry, entry_end).unwrap_or(entry))
            };
            let from = offset_of(lines, text, start);
            let stop = offset_of(lines, text, last + 1);
            Ok(format!("{}{}", &text[..from], &text[stop..]))
        }
        (None, None) => Err(bad(format!("there is no role named {name:?} here to remove"))),
    }
}

/// The file's text with one role set or removed, in whichever of the two
/// places `file` says the layer is written.
fn splice_role(
    text: &str,
    document: &Value,
    file: RoleFile,
    name: &str,
    spec: Option<&RoleSpec>,
    after: &BTreeMap<String, RoleSpec>,
    path: &Path,
) -> Result<String> {
    let lines = line_table(text);
    if file == RoleFile::Root {
        return splice_roles_block(text, &lines, 0, lines.len(), 0, name, spec, after);
    }

    let scope_index = lines
        .iter()
        .position(|(_, _, line)| indentation(line) == Some(0) && key_rest(line, "scope").is_some())
        .ok_or_else(|| {
            bad(format!(
                "scope config {} has no top-level scope block",
                path.display()
            ))
        })?;
    let scope_rest = key_rest(lines[scope_index].2, "scope").unwrap_or_default();
    let block_scope = scope_rest.trim().is_empty() || scope_rest.trim_start().starts_with('#');
    if !block_scope {
        // A flow-style scope has no line for its roles. Expand just that one
        // `scope:` line to a block, as adding an agent to one already does.
        let mut scope = document
            .as_mapping()
            .and_then(|root| root.get(Value::String("scope".into())))
            .cloned()
            .ok_or_else(|| bad(format!("scope config {} has no scope block", path.display())))?;
        let scope_map = mapping(&mut scope, "scope", path)?;
        let key = Value::String("roles".into());
        if after.is_empty() {
            scope_map.remove(&key);
        } else {
            let encoded = serde_yaml_ng::to_value(after).map_err(|error| {
                FactoryError::Other(anyhow::anyhow!("encoding roles: {error}"))
            })?;
            scope_map.insert(key, encoded);
        }
        let comment = scope_rest
            .find('#')
            .map(|at| format!(" {}", scope_rest[at..].trim()))
            .unwrap_or_default();
        let replacement = rendered_scope(&scope, &comment, path)?;
        let (start, end, _) = lines[scope_index];
        return Ok(format!("{}{}{}", &text[..start], replacement, &text[end..]));
    }

    let scope_end = lines
        .iter()
        .enumerate()
        .skip(scope_index + 1)
        .find(|(_, (_, _, line))| is_content(line) && indentation(line) == Some(0))
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let child_indent = lines[scope_index + 1..scope_end]
        .iter()
        .filter(|(_, _, line)| is_content(line))
        .filter_map(|(_, _, line)| indentation(line))
        .min()
        .unwrap_or(2);
    splice_roles_block(
        text,
        &lines,
        scope_index + 1,
        scope_end,
        child_indent,
        name,
        spec,
        after,
    )
}

impl Engine {
    /// Write one role into a scope's own config and make it hold from the
    /// next request. `replace` must say whether the file already defines it:
    /// creating over a definition, or editing one that is not there, is a
    /// mistake about which scope the page is looking at, not a request to
    /// guess.
    pub(crate) async fn define_role(
        &self,
        scope_name: &str,
        name: &str,
        mut spec: RoleSpec,
        replace: bool,
    ) -> Result<(String, String)> {
        let name = name.trim().to_string();
        if !valid_role_name(&name) {
            return Err(bad(
                "a role name is letters, digits, `-`, `_` and `.`, and cannot be empty",
            ));
        }
        if name == Role::WORKER || name == Role::FOREMAN {
            return Err(bad(format!(
                "{name:?} is a built-in role and cannot be redefined at any level"
            )));
        }
        spec.describe = spec
            .describe
            .map(|describe| describe.trim().to_string())
            .filter(|describe| !describe.is_empty());
        spec.grants = spec
            .grants
            .into_iter()
            .map(|grant| grant.trim().to_string())
            .filter(|grant| !grant.is_empty())
            .collect();
        let given = self.given_roles().await?;
        let scope = self.edit_role_layer(scope_name, &name, Some(spec), &given, |_, current, before, _| {
            match (before.contains_key(&name), replace) {
                (true, false) => Err(bad(format!(
                    "{} already defines {name:?}; edit that definition instead",
                    current.name
                ))),
                (false, true) => Err(bad(format!(
                    "{} defines no role named {name:?} of its own to edit. \
                     An inherited role is overridden by defining it here",
                    current.name
                ))),
                _ => Ok(()),
            }
        })?;
        Ok((scope, name))
    }

    /// Remove one role from a scope's own config. Refused while any agent --
    /// in this scope or below it, declared or given -- still resolves the
    /// name to this definition, and the refusal names every one of them.
    pub(crate) async fn delete_role(&self, scope_name: &str, name: &str) -> Result<(String, String)> {
        let name = name.trim().to_string();
        if name == Role::WORKER || name == Role::FOREMAN {
            return Err(bad(format!("{name:?} is a built-in role and cannot be deleted")));
        }
        let given = self.given_roles().await?;
        let scope = self.edit_role_layer(scope_name, &name, None, &given, |factory, current, before, origin| {
            if !before.contains_key(&name) {
                return Err(bad(format!(
                    "{} defines no role named {name:?} of its own. \
                     An inherited role is removed where it is defined",
                    current.name
                )));
            }
            let dependents = self.dependents_of(factory, origin, &Role::new(name.clone()), &given)?;
            if !dependents.is_empty() {
                return Err(bad(format!(
                    "{name:?} is still held by {}. Give them another role first",
                    dependents.join(", ")
                )));
            }
            Ok(())
        })?;
        Ok((scope, name))
    }

    /// Every role somebody gave a standing agent, by agent id. Read before a
    /// role edit takes the configuration lock, which is a plain mutex and
    /// cannot be held across the store's await.
    async fn given_roles(&self) -> Result<HashMap<String, Role>> {
        Ok(self
            .store
            .agents()
            .await?
            .into_iter()
            .filter_map(|agent| agent.assigned_role.map(|role| (agent.id, role)))
            .collect())
    }

    /// Set or remove one role in the layer `scope_name` writes, after `check`
    /// has had its say about the layer as it stands. The whole instance is
    /// validated with the change applied before a byte is written: no agent,
    /// declared or given, may be left on a role nothing in its chain defines.
    fn edit_role_layer(
        &self,
        scope_name: &str,
        name: &str,
        spec: Option<RoleSpec>,
        given: &HashMap<String, Role>,
        check: impl FnOnce(
            &factory_core::config::Factory,
            &Scope,
            &BTreeMap<String, RoleSpec>,
            &RoleOrigin,
        ) -> Result<()>,
    ) -> Result<String> {
        let _edit = self
            .configuration_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let factory = self.factory_snapshot();
        let current = factory.scope(scope_name)?.clone();
        // The instance root writes its roles at the top of its own file; every
        // other scope writes them under its `scope:` block.
        let origin = crate::roles::layer_written_by(&factory, &current);
        let file = match origin {
            RoleOrigin::Instance => RoleFile::Root,
            _ => RoleFile::Scope,
        };
        let path = match file {
            RoleFile::Root => factory.factory_dir().join(CONFIG_FILE),
            RoleFile::Scope => factory
                .scope_path(&current.name)?
                .join(FACTORY_DIR)
                .join(CONFIG_FILE),
        };

        let text = fs::read_to_string(&path).map_err(|error| {
            FactoryError::Other(anyhow::anyhow!("reading {}: {error}", path.display()))
        })?;
        let document: Value = serde_yaml_ng::from_str(&text)
            .map_err(|error| bad(format!("parsing {}: {error}", path.display())))?;
        let mut from_file = None;
        let before = match file {
            RoleFile::Root => {
                let config: Config = serde_yaml_ng::from_str(&text)
                    .map_err(|error| bad(format!("parsing {}: {error}", path.display())))?;
                config.roles
            }
            RoleFile::Scope => {
                let (_, _, mut scope) = read_document(&path)?;
                if scope.id != current.id || scope.name != current.name {
                    return Err(bad(format!(
                        "scope config {} changed identity since startup; restart Factory before editing it",
                        path.display()
                    )));
                }
                scope.path = current.path.clone();
                let roles = scope.roles.clone();
                from_file = Some(scope);
                roles
            }
        };

        check(&factory, &current, &before, &origin)?;

        let mut after = before.clone();
        match &spec {
            Some(spec) => {
                after.insert(name.to_string(), spec.clone());
            }
            None => {
                after.remove(name);
            }
        }

        // The instance as it would be, checked whole.
        let mut candidate = factory.clone();
        match (&file, &mut from_file) {
            (RoleFile::Root, _) => candidate.config.roles = after.clone(),
            (RoleFile::Scope, Some(scope)) => {
                scope.roles = after.clone();
                if let Some(slot) = candidate.config.scopes.iter_mut().find(|s| s.id == current.id) {
                    *slot = scope.clone();
                }
            }
            (RoleFile::Scope, None) => unreachable!("a scope layer was read from its file"),
        }
        candidate.config.validate()?;
        for (id, role) in given {
            let Some((scope, agent)) = id.rsplit_once('/') else { continue };
            // A row whose scope has gone is swept by the next reconcile. No edit
            // here can change what it may do, and it must not refuse every role
            // write in the instance until then.
            if candidate.scope(scope).is_err() {
                continue;
            }
            if !candidate.roles_for(scope)?.contains(role) {
                return Err(bad(format!(
                    "{agent} in {scope} was given the role {:?}, which nothing in {scope} would define after this change. \
                     Give it another role first",
                    role.as_str()
                )));
            }
        }

        let serialized = splice_role(&text, &document, file, name, spec.as_ref(), &after, &path)?;
        let written: BTreeMap<String, RoleSpec> = match file {
            RoleFile::Root => serde_yaml_ng::from_str::<Config>(&serialized).map(|c| c.roles),
            RoleFile::Scope => serde_yaml_ng::from_str::<ScopeFile>(&serialized).map(|f| f.scope.roles),
        }
        .map_err(|error| {
            FactoryError::Other(anyhow::anyhow!(
                "could not edit the roles in {} without breaking it ({error}); nothing was written",
                path.display()
            ))
        })?;
        if written != after {
            return Err(FactoryError::Other(anyhow::anyhow!(
                "could not edit the roles in {} without changing more than {name:?}; nothing was written",
                path.display()
            )));
        }
        atomic_write(&path, &serialized)?;

        match (file, from_file) {
            (RoleFile::Root, _) => self.replace_instance_roles(after),
            (RoleFile::Scope, Some(scope)) => self.replace_scope(&current.id, scope),
            (RoleFile::Scope, None) => unreachable!("a scope layer was read from its file"),
        }
        Ok(current.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance as InstanceInfo, Sandbox};
    use factory_core::role::Role;
    use factory_plugins::registry::Registry;
    use factory_plugins::SqliteStore;
    use std::path::PathBuf;
    use std::sync::Arc;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str, yaml: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "factory-agent-config-{tag}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(root.join(FACTORY_DIR)).unwrap();
            fs::write(root.join(FACTORY_DIR).join(CONFIG_FILE), yaml).unwrap();
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn agent(name: &str) -> ScopeAgent {
        ScopeAgent {
            name: Some(name.into()),
            harness: "pi".into(),
            lifetime: Lifetime::Temporary,
            role: Role::worker(),
            autostart: Some(false),
            args: vec!["--model".into(), "local model".into()],
            sandbox: Sandbox::None,
            provider: None,
        }
    }

    fn engine(scratch: &Scratch) -> Arc<Engine> {
        engine_with(scratch, Default::default())
    }

    fn engine_with(scratch: &Scratch, infrastructure: factory_core::config::Infrastructure) -> Arc<Engine> {
        let scope: Scope = serde_yaml_ng::from_str("id: scope-id\nname: demo\n").unwrap();
        let factory = Factory {
            root: scratch.0.clone(),
            config: Config {
                version: 1,
                instance: InstanceInfo {
                    id: "i".into(),
                    name: "test".into(),
                },
                daemon: DaemonConfig::default(),
                scope: None,
                scopes: vec![scope],
                roles: Default::default(),
                dashboard: None,
                policies: Default::default(),
                quality: Default::default(),
                infrastructure,
                plugins_dir: None,
            },
        };
        Arc::new(Engine::new(
            factory,
            Registry::with_builtins(),
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("factory"),
            vec![],
        ))
    }

    #[test]
    fn adding_an_agent_preserves_unrelated_data_and_updates_the_live_snapshot() {
        let scratch = Scratch::new(
            "preserve",
            "# file owner note\nversion: 1\nruntime:\n  # keep this provider note\n  provider: local\nscope:\n  id: scope-id\n  name: demo\n  runtime: herdr\n",
        );
        let engine = engine(&scratch);

        let (scope, saved) = engine.configure_agent("demo", agent("reviewer")).unwrap();

        assert_eq!(scope, "demo");
        assert_eq!(saved.args, vec!["--model", "local model"]);
        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        assert!(text.contains("# file owner note"));
        assert!(text.contains("# keep this provider note"));
        let yaml: Value = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(yaml["runtime"]["provider"].as_str(), Some("local"));
        assert_eq!(yaml["scope"]["runtime"].as_str(), Some("herdr"));
        assert_eq!(
            yaml["scope"]["agents"][0]["name"].as_str(),
            Some("reviewer")
        );
        assert_eq!(
            engine.resolve_agent("demo", "reviewer").unwrap().1,
            "pi",
            "the running daemon uses the saved declaration"
        );
    }

    /// `configure_agent` hands the whole `ScopeAgent` to `serde_yaml_ng`
    /// (`append_agent`/`rendered_item`), so a new field needs nothing named
    /// here to be written -- but the default has to stay invisible, per
    /// `Sandbox::is_none`, or every agent added from now on grows a
    /// `sandbox: none` line nobody asked for.
    #[test]
    fn a_default_sandbox_is_never_written_into_the_file() {
        let scratch = Scratch::new(
            "sandbox-default",
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n",
        );
        let engine = engine(&scratch);

        engine.configure_agent("demo", agent("reviewer")).unwrap();

        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        assert!(
            !text.contains("sandbox"),
            "the default sandbox never appears in the file: {text}"
        );
    }

    #[test]
    fn a_declared_sandbox_is_written_and_reloaded() {
        let scratch = Scratch::new(
            "sandbox-declared",
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n",
        );
        let engine = engine(&scratch);
        let mut boxed = agent("boxed");
        boxed.sandbox = Sandbox::Docker;

        engine.configure_agent("demo", boxed).unwrap();

        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        assert!(text.contains("sandbox: docker"), "{text}");
        let reloaded: ScopeFile = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(
            reloaded.scope.declared_agents()[0].sandbox,
            Sandbox::Docker,
            "the file round-trips back into the same value"
        );
    }

    fn one_provider() -> factory_core::config::Infrastructure {
        serde_yaml_ng::from_str(
            "providers:\n  - name: openrouter\n    vendor: openrouter\n    kind: api-key\n    env: OPENROUTER_API_KEY\n",
        )
        .unwrap()
    }

    /// The roster must never write a declaration the next start refuses:
    /// an undeclared provider is turned away before the file is touched.
    #[test]
    fn an_undeclared_provider_is_refused_with_the_file_untouched() {
        let scratch = Scratch::new("provider-unknown", "version: 1\nscope:\n  id: scope-id\n  name: demo\n");
        let engine = engine_with(&scratch, one_provider());
        let path = scratch.0.join(FACTORY_DIR).join(CONFIG_FILE);
        let before = fs::read(&path).unwrap();
        let mut typo = agent("lab");
        typo.provider = Some("openruter".into());

        let e = engine.configure_agent("demo", typo).unwrap_err().to_string();
        assert!(e.contains("openruter") && e.contains("openrouter"), "{e}");
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn a_declared_provider_is_written_and_reloaded() {
        let scratch = Scratch::new("provider-declared", "version: 1\nscope:\n  id: scope-id\n  name: demo\n");
        let engine = engine_with(&scratch, one_provider());
        let mut lab = agent("lab");
        lab.provider = Some(" openrouter ".into());

        engine.configure_agent("demo", lab).unwrap();

        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        assert!(text.contains("provider: openrouter"), "{text}");
        let reloaded: ScopeFile = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(reloaded.scope.declared_agents()[0].provider.as_deref(), Some("openrouter"));
    }

    /// The roster edits the root scope through this same reader, and the
    /// root's file is where `infrastructure:` lives -- a provider block must
    /// never make that file uneditable.
    #[test]
    fn the_root_file_with_its_providers_stays_editable() {
        let scratch = Scratch::new(
            "provider-root",
            "version: 1\ninstance: { id: i, name: test }\n\
             infrastructure:\n  providers:\n    - name: openrouter\n      vendor: openrouter\n      kind: api-key\n\
             scope:\n  id: scope-id\n  name: demo\n",
        );
        let engine = engine_with(&scratch, one_provider());
        let mut lab = agent("lab");
        lab.provider = Some("openrouter".into());

        engine.configure_agent("demo", lab).unwrap();

        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        assert!(text.contains("infrastructure:\n  providers:"), "left where it was: {text}");
        assert!(text.contains("provider: openrouter"), "{text}");
    }

    #[test]
    fn a_rejected_agent_leaves_the_file_byte_for_byte_unchanged() {
        let yaml =
            "version: 1\nruntime: { provider: local }\nscope: { id: scope-id, name: demo }\n";
        let scratch = Scratch::new("atomic", yaml);
        let engine = engine(&scratch);
        let path = scratch.0.join(FACTORY_DIR).join(CONFIG_FILE);
        let before = fs::read(&path).unwrap();
        let mut invalid = agent("bad");
        invalid.harness = "missing-adapter".into();

        assert!(engine.configure_agent("demo", invalid).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn duplicate_effective_names_are_rejected() {
        let scratch = Scratch::new(
            "duplicate",
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n  agents:\n    - harness: pi\n",
        );
        let engine = engine(&scratch);
        let duplicate = ScopeAgent {
            name: None,
            ..agent("ignored")
        };

        let error = engine
            .configure_agent("demo", duplicate)
            .unwrap_err()
            .to_string();
        assert!(error.contains("already declares"), "{error}");
    }

    #[test]
    fn an_existing_agent_list_is_extended_without_moving_other_scope_fields() {
        let scratch = Scratch::new(
            "existing",
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n  agents:\n    - name: first\n      harness: codex\n  # the store stays below the roster\n  task_store: sqlite\n",
        );
        let engine = engine(&scratch);

        engine.configure_agent("demo", agent("second")).unwrap();

        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        assert!(text.contains("# the store stays below the roster"));
        assert!(text.find("name: first").unwrap() < text.find("name: second").unwrap());
        assert!(text.find("name: second").unwrap() < text.find("task_store: sqlite").unwrap());
    }

    #[test]
    fn deleting_an_agent_removes_only_that_list_item_and_updates_the_snapshot() {
        let scratch = Scratch::new(
            "delete-list",
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n  agents:\n    - name: first\n      harness: codex\n    - name: second\n      harness: pi\n  # keep this scope note\n  task_store: sqlite\n",
        );
        let engine = engine(&scratch);

        let (scope, removed) = engine.delete_agent_declaration("demo", "first").unwrap();

        assert_eq!(scope, "demo");
        assert_eq!(removed.name(), "first");
        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        assert!(!text.contains("name: first"));
        assert!(text.contains("name: second"));
        assert!(text.contains("# keep this scope note"));
        assert!(text.contains("task_store: sqlite"));
        assert!(engine.resolve_agent("demo", "first").is_err());
        assert_eq!(engine.resolve_agent("demo", "second").unwrap().1, "pi");
    }

    #[test]
    fn deleting_the_only_list_agent_leaves_valid_yaml() {
        let scratch = Scratch::new(
            "delete-only",
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n  agents:\n    - name: only\n      harness: pi\n  runtime: herdr\n",
        );
        let engine = engine(&scratch);

        engine.delete_agent_declaration("demo", "only").unwrap();

        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        let parsed: ScopeFile = serde_yaml_ng::from_str(&text).unwrap();
        assert!(parsed.scope.agents.is_empty());
        assert!(text.contains("runtime: herdr"));
    }

    #[test]
    fn deleting_a_singular_declaration_keeps_the_rest_of_the_scope() {
        let scratch = Scratch::new(
            "delete-singular",
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n  agent:\n    name: lead\n    harness: pi\n    lifetime: permanent\n  runtime: herdr\n",
        );
        let engine = engine(&scratch);

        engine.delete_agent_declaration("demo", "lead").unwrap();

        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        let parsed: ScopeFile = serde_yaml_ng::from_str(&text).unwrap();
        assert!(parsed.scope.agent.is_none());
        assert!(text.contains("runtime: herdr"));
    }

    #[test]
    fn deletion_handles_inline_agent_lists_without_rewriting_siblings() {
        let scratch = Scratch::new(
            "delete-inline",
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n  agents: [{ name: first, harness: codex }, { name: second, harness: pi }]\nruntime: { provider: local }\n",
        );
        let engine = engine(&scratch);

        engine.delete_agent_declaration("demo", "first").unwrap();

        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        let parsed: ScopeFile = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(parsed.scope.agents.len(), 1);
        assert_eq!(parsed.scope.agents[0].name(), "second");
        assert!(text.contains("runtime: { provider: local }"));
    }

    #[test]
    fn deletion_handles_a_flow_style_scope() {
        let scratch = Scratch::new(
            "delete-flow",
            "version: 1\nscope: { id: scope-id, name: demo, agents: [{ name: reviewer, harness: pi }] }\nruntime: { provider: local }\n",
        );
        let engine = engine(&scratch);

        engine.delete_agent_declaration("demo", "reviewer").unwrap();

        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        let parsed: ScopeFile = serde_yaml_ng::from_str(&text).unwrap();
        assert!(parsed.scope.agents.is_empty());
        assert!(text.contains("runtime: { provider: local }"));
    }

    #[test]
    fn a_flow_style_scope_expands_without_rewriting_sibling_blocks() {
        let scratch = Scratch::new(
            "flow",
            "version: 1\n# scope identity\nscope: { id: scope-id, name: demo }\n# runtime belongs to another manager\nruntime: { provider: local }\n",
        );
        let engine = engine(&scratch);

        engine.configure_agent("demo", agent("reviewer")).unwrap();

        let text = fs::read_to_string(scratch.0.join(FACTORY_DIR).join(CONFIG_FILE)).unwrap();
        assert!(text.contains("# scope identity"));
        assert!(text.contains("# runtime belongs to another manager"));
        assert!(text.contains("runtime: { provider: local }"));
        let yaml: Value = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(
            yaml["scope"]["agents"][0]["name"].as_str(),
            Some("reviewer")
        );
    }

    #[tokio::test]
    async fn the_request_returns_ok_and_announces_the_new_declaration() {
        use factory_core::event::Event;
        use factory_core::protocol::{Payload, Request, Response};

        let scratch = Scratch::new(
            "request",
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n",
        );
        let engine = engine(&scratch);
        let mut events = engine.bus.subscribe();

        let response = engine
            .handle_request(Request::AgentConfigure {
                scope: "demo".into(),
                agent: agent("reviewer"),
            })
            .await;

        assert!(matches!(response, Response::Ok { data: Payload::Ok }));
        assert!(matches!(
            events.recv().await.unwrap(),
            Event::AgentConfigured { scope, name }
                if scope == "demo" && name == "reviewer"
        ));
    }

    #[tokio::test]
    async fn the_delete_request_removes_and_announces_the_declaration() {
        use factory_core::event::Event;
        use factory_core::protocol::{Payload, Request, Response};

        let scratch = Scratch::new(
            "delete-request",
            "version: 1\nscope:\n  id: scope-id\n  name: demo\n  agents:\n    - name: reviewer\n      harness: pi\n",
        );
        let engine = engine(&scratch);
        let mut events = engine.bus.subscribe();

        let response = engine
            .handle_request(Request::AgentDelete {
                scope: "demo".into(),
                name: "reviewer".into(),
            })
            .await;

        assert!(matches!(
            response,
            Response::Ok {
                data: Payload::Deleted { deleted: true }
            }
        ));
        assert!(matches!(
            events.recv().await.unwrap(),
            Event::AgentDeleted { scope, name }
                if scope == "demo" && name == "reviewer"
        ));
    }

    // -- roles ----------------------------------------------------------------

    /// A real instance on disk -- the root's config and each nested scope's,
    /// at their paths -- loaded the way the daemon loads one, through
    /// discovery, so the scope list and its paths are the real ones.
    struct Instance(PathBuf);

    impl Instance {
        fn new(tag: &str, root: &str, scopes: &[(&str, &str)]) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "factory-roles-{tag}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(dir.join(FACTORY_DIR)).unwrap();
            fs::write(dir.join(FACTORY_DIR).join(CONFIG_FILE), root).unwrap();
            for (rel, yaml) in scopes {
                let at = dir.join(rel).join(FACTORY_DIR);
                fs::create_dir_all(&at).unwrap();
                fs::write(at.join(CONFIG_FILE), yaml).unwrap();
            }
            Self(dir)
        }

        fn engine(&self) -> Arc<Engine> {
            let mut factory = Factory::load(&self.0).unwrap();
            crate::discovery::apply(&mut factory).unwrap();
            factory.config.validate().unwrap();
            Arc::new(Engine::new(
                factory,
                Registry::with_builtins(),
                Arc::new(SqliteStore::in_memory().unwrap()),
                PathBuf::from("factory"),
                vec![],
            ))
        }

        fn file(&self, rel: &str) -> PathBuf {
            self.0.join(rel).join(FACTORY_DIR).join(CONFIG_FILE)
        }

        fn text(&self, rel: &str) -> String {
            fs::read_to_string(self.file(rel)).unwrap()
        }

        fn yaml(&self, rel: &str) -> Value {
            serde_yaml_ng::from_str(&self.text(rel)).unwrap()
        }
    }

    impl Drop for Instance {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const ROOT: &str = "version: 1\ninstance: { id: i, name: test }\n";

    fn spec(describe: &str, grants: &[&str], reach: &str) -> RoleSpec {
        serde_yaml_ng::from_str(&format!(
            "describe: {describe}\ngrants: [{}]\nreach: {reach}\n",
            grants.join(", ")
        ))
        .unwrap()
    }

    fn grants(yaml: &Value, rel_role: &str) -> Vec<String> {
        yaml["scope"]["roles"][rel_role]["grants"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|g| g.as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn defining_a_role_writes_scope_roles_and_leaves_every_other_line_alone() {
        let instance = Instance::new(
            "define",
            ROOT,
            &[(
                "projects",
                "# owner note\nversion: 1\nscope:\n  id: p\n  name: projects\n  # the runtime stays put\n  runtime: herdr\n# runtime belongs to another manager\nruntime:\n  provider: local\n",
            )],
        );
        let engine = instance.engine();

        engine
            .define_role("projects", "reviewer", spec("works its own tasks", &["task.edit", "task.report"], "own"), false)
            .await
            .unwrap();

        let text = instance.text("projects");
        for kept in ["# owner note", "# the runtime stays put", "runtime: herdr", "# runtime belongs to another manager"] {
            assert!(text.contains(kept), "{kept:?} is gone from:\n{text}");
        }
        assert!(
            text.find("roles:").unwrap() < text.find("# runtime belongs to another manager").unwrap(),
            "the block lands inside the scope, not after a comment meant for what follows:\n{text}"
        );
        let yaml = instance.yaml("projects");
        assert_eq!(grants(&yaml, "reviewer"), ["task.edit", "task.report"]);
        assert_eq!(yaml["scope"]["roles"]["reviewer"]["reach"].as_str(), Some("own"));
        assert_eq!(yaml["runtime"]["provider"].as_str(), Some("local"));
        assert!(
            engine.roles_for("projects").contains(&Role::new("reviewer")),
            "the running daemon has it from the next request"
        );
    }

    #[tokio::test]
    async fn editing_replaces_the_whole_definition_and_leaves_its_neighbours() {
        let instance = Instance::new(
            "edit",
            ROOT,
            &[(
                "projects",
                "version: 1\nscope:\n  id: p\n  name: projects\n  roles:\n    reviewer:\n      describe: works its own tasks\n      grants: [task.edit, task.report]\n      reach: own\n    # the lead runs the board\n    lead:\n      grants: [task.run]\n      reach: scope\n  task_store: sqlite\n",
            )],
        );
        let engine = instance.engine();

        let exists = engine
            .define_role("projects", "reviewer", spec("x", &["task.create"], "own"), false)
            .await
            .unwrap_err()
            .to_string();
        assert!(exists.contains("already defines"), "{exists}");
        let missing = engine
            .define_role("projects", "ghost", spec("x", &["task.create"], "own"), true)
            .await
            .unwrap_err()
            .to_string();
        assert!(missing.contains("defines no role"), "{missing}");

        engine
            .define_role("projects", "reviewer", spec("reviews, and opens follow-ups", &["task.create"], "scope"), true)
            .await
            .unwrap();

        let text = instance.text("projects");
        assert!(text.contains("# the lead runs the board"), "{text}");
        assert!(text.contains("task_store: sqlite"), "{text}");
        let yaml = instance.yaml("projects");
        assert_eq!(grants(&yaml, "reviewer"), ["task.create"], "replaced, never merged");
        assert_eq!(yaml["scope"]["roles"]["reviewer"]["reach"].as_str(), Some("scope"));
        assert_eq!(grants(&yaml, "lead"), ["task.run"]);
    }

    #[tokio::test]
    async fn deleting_removes_one_entry_and_the_last_one_takes_the_block() {
        let instance = Instance::new(
            "delete",
            ROOT,
            &[(
                "projects",
                "version: 1\nscope:\n  id: p\n  name: projects\n  roles:\n    reviewer:\n      grants: [task.report]\n    lead:\n      grants: [task.run]\n  runtime: herdr\n",
            )],
        );
        let engine = instance.engine();

        engine.delete_role("projects", "reviewer").await.unwrap();
        let yaml = instance.yaml("projects");
        assert!(yaml["scope"]["roles"].get("reviewer").is_none());
        assert_eq!(grants(&yaml, "lead"), ["task.run"]);

        engine.delete_role("projects", "lead").await.unwrap();
        let text = instance.text("projects");
        assert!(!text.contains("roles"), "no empty block is left behind:\n{text}");
        assert!(text.contains("runtime: herdr"), "{text}");
        assert!(!engine.roles_for("projects").contains(&Role::new("lead")));
    }

    #[tokio::test]
    async fn a_role_still_in_use_is_not_deleted_and_every_holder_is_named() {
        let parent = "version: 1\nscope:\n  id: p\n  name: projects\n  roles:\n    reviewer:\n      grants: [task.report]\n";
        let instance = Instance::new(
            "in-use",
            ROOT,
            &[
                ("projects", parent),
                (
                    "projects/demo",
                    "version: 1\nscope:\n  id: d\n  name: demo\n  agents:\n    - name: critic\n      harness: pi\n      role: reviewer\n",
                ),
            ],
        );
        let engine = instance.engine();
        let watcher = factory_core::agent::AgentSession::new(
            "demo", "watcher", "pi", "herdr", Lifetime::Permanent, Role::worker(),
        );
        engine.store.put_agent(&watcher).await.unwrap();
        engine.set_agent_role("demo/watcher", Some(Role::new("reviewer"))).await.unwrap();

        let error = engine.delete_role("projects", "reviewer").await.unwrap_err().to_string();
        assert!(error.contains("critic in demo"), "{error}");
        assert!(error.contains("watcher"), "a given role counts too: {error}");
        assert_eq!(instance.text("projects"), parent, "nothing was written");
    }

    #[tokio::test]
    async fn a_parent_definition_nobody_resolves_to_can_go_while_an_override_below_is_held() {
        let instance = Instance::new(
            "override",
            ROOT,
            &[
                ("projects", "version: 1\nscope:\n  id: p\n  name: projects\n  roles:\n    reviewer:\n      grants: [task.report]\n"),
                (
                    "projects/demo",
                    "version: 1\nscope:\n  id: d\n  name: demo\n  roles:\n    reviewer:\n      grants: [task.create]\n  agents:\n    - name: critic\n      harness: pi\n      role: reviewer\n",
                ),
            ],
        );
        let engine = instance.engine();

        let held = engine.delete_role("demo", "reviewer").await.unwrap_err().to_string();
        assert!(held.contains("critic in demo"), "critic resolves to demo's own: {held}");

        engine.delete_role("projects", "reviewer").await.unwrap();
        let roles = engine.roles_for("demo");
        let entry = roles.entry(&Role::new("reviewer")).unwrap();
        assert_eq!(entry.origin, RoleOrigin::Scope { scope: "demo".into() });
        assert_eq!(entry.overrides, None, "nothing above it to replace any more");
    }

    #[tokio::test]
    async fn an_agent_row_left_behind_by_a_gone_scope_does_not_block_role_writes() {
        let instance = Instance::new("stale", ROOT, &[("projects", "version: 1\nscope:\n  id: p\n  name: projects\n")]);
        let engine = instance.engine();
        let mut stale = factory_core::agent::AgentSession::new(
            "vanished", "ghost", "pi", "herdr", Lifetime::Permanent, Role::worker(),
        );
        stale.assigned_role = Some(Role::new("reviewer"));
        engine.store.put_agent(&stale).await.unwrap();

        engine
            .define_role("projects", "lead", spec("runs the board", &["task.run"], "scope"), false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_two_that_ship_are_refused_at_every_level() {
        let instance = Instance::new("presets", ROOT, &[("projects", "version: 1\nscope:\n  id: p\n  name: projects\n")]);
        let engine = instance.engine();
        for name in ["worker", "foreman"] {
            let defined = engine
                .define_role("projects", name, spec("wider", &["task.create"], "scope"), false)
                .await
                .unwrap_err()
                .to_string();
            assert!(defined.contains("built-in"), "{defined}");
            let deleted = engine.delete_role("projects", name).await.unwrap_err().to_string();
            assert!(deleted.contains("built-in"), "{deleted}");
        }
    }

    #[tokio::test]
    async fn a_bad_role_is_refused_with_the_file_untouched() {
        let yaml = "version: 1\nscope:\n  id: p\n  name: projects\n";
        let instance = Instance::new("bad", ROOT, &[("projects", yaml)]);
        let engine = instance.engine();

        let error = engine
            .define_role("projects", "approver", spec("x", &["task.approve"], "own"), false)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("task.approve"), "{error}");
        let error = engine
            .define_role("projects", "has space", spec("x", &["task.report"], "own"), false)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("role name"), "{error}");
        assert_eq!(instance.text("projects"), yaml);
    }

    #[tokio::test]
    async fn the_instance_root_writes_its_top_level_roles() {
        let root = "version: 1\ninstance: { id: i, name: test }\nscope:\n  id: root\n  name: company\n# daemon settings follow\ndaemon:\n  tick_seconds: 5\n";
        let instance = Instance::new("root", root, &[("projects", "version: 1\nscope:\n  id: p\n  name: projects\n")]);
        let engine = instance.engine();

        engine
            .define_role("company", "runner", spec("starts what is on the board", &["task.run", "task.cancel"], "scope"), false)
            .await
            .unwrap();

        let text = instance.text("");
        assert!(text.contains("# daemon settings follow"), "{text}");
        let yaml = instance.yaml("");
        assert!(yaml["roles"]["runner"].is_mapping(), "{text}");
        assert!(yaml["scope"].get("roles").is_none(), "never scope.roles on the root:\n{text}");
        assert_eq!(yaml["daemon"]["tick_seconds"].as_u64(), Some(5));
        let everywhere = engine.roles_for("projects");
        assert_eq!(everywhere.entry(&Role::new("runner")).unwrap().origin, RoleOrigin::Instance);
    }

    #[tokio::test]
    async fn a_flow_style_scope_is_expanded_to_hold_its_roles() {
        let instance = Instance::new(
            "flow",
            ROOT,
            &[("projects", "version: 1\n# identity\nscope: { id: p, name: projects }\nruntime: { provider: local }\n")],
        );
        let engine = instance.engine();

        engine
            .define_role("projects", "reviewer", spec("works its own tasks", &["task.report"], "own"), false)
            .await
            .unwrap();

        let text = instance.text("projects");
        assert!(text.contains("# identity"), "{text}");
        assert!(text.contains("runtime: { provider: local }"), "{text}");
        assert_eq!(grants(&instance.yaml("projects"), "reviewer"), ["task.report"]);
    }

    #[tokio::test]
    async fn the_role_requests_answer_and_announce_the_change() {
        use factory_core::event::Event;
        use factory_core::protocol::{Payload, Request, Response};

        let instance = Instance::new("request", ROOT, &[("projects", "version: 1\nscope:\n  id: p\n  name: projects\n")]);
        let engine = instance.engine();
        let mut events = engine.bus.subscribe();

        let response = engine
            .handle_request(Request::RoleDefine {
                scope: "projects".into(),
                name: "reviewer".into(),
                role: spec("works its own tasks", &["task.report"], "own"),
                replace: false,
            })
            .await;
        assert!(matches!(response, Response::Ok { data: Payload::Ok }), "{response:?}");
        assert!(matches!(
            events.recv().await.unwrap(),
            Event::RolesChanged { scope, name } if scope == "projects" && name == "reviewer"
        ));

        let response = engine
            .handle_request(Request::RoleDelete { scope: "projects".into(), name: "reviewer".into() })
            .await;
        assert!(matches!(response, Response::Ok { data: Payload::Deleted { deleted: true } }), "{response:?}");
    }
}
