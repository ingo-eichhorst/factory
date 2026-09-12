//! Mutations of the local, scope-owned Factory configuration.
//!
//! Discovery reads these files at startup. Roster edits use the same files as
//! their source of truth, replace one atomically, and then update the Engine's
//! configuration snapshot so the declaration is usable without a restart.

use factory_core::agent::Lifetime;
use factory_core::config::{Scope, ScopeAgent, CONFIG_FILE, FACTORY_DIR};
use factory_core::error::{FactoryError, Result};
use serde::Deserialize;
use serde_yaml_ng::{Mapping, Value};
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
        let yaml = serde_yaml_ng::to_string(scope).map_err(|error| {
            FactoryError::Other(anyhow::anyhow!(
                "encoding scope config {}: {error}",
                path.display()
            ))
        })?;
        let comment = scope_rest
            .find('#')
            .map(|at| format!(" {}", scope_rest[at..].trim()))
            .unwrap_or_default();
        let mut replacement = format!("scope:{comment}\n");
        for line in yaml.lines() {
            replacement.push_str("  ");
            replacement.push_str(line);
            replacement.push('\n');
        }
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
        if !self.roles.contains(&agent.role) {
            return Err(bad(format!(
                "no role named {:?}. This instance has: {}",
                agent.role.as_str(),
                self.roles.names().join(", ")
            )));
        }
        if agent.harness == "shell" && !agent.args.is_empty() {
            return Err(bad(
                "the shell agent runs task instructions directly and cannot use CLI arguments",
            ));
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance};
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
        }
    }

    fn engine(scratch: &Scratch) -> Arc<Engine> {
        let scope: Scope = serde_yaml_ng::from_str("id: scope-id\nname: demo\n").unwrap();
        let factory = Factory {
            root: scratch.0.clone(),
            config: Config {
                version: 1,
                instance: Instance {
                    id: "i".into(),
                    name: "test".into(),
                },
                daemon: DaemonConfig::default(),
                scope: None,
                scopes: vec![scope],
                roles: Default::default(),
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
}
