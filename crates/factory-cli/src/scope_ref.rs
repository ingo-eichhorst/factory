//! Resolving what an operator typed after `--scope`.
//!
//! Design §7 addresses scopes by **name** throughout — `factory task send
//! irrlicht "…"`, `factory agent start irrlicht`, `factory agent status
//! irrlicht`. Nothing in those examples is a UUID, and rightly: a person
//! types a name, and a scope's name is what appears in `config.yaml` and in
//! `factory scope list`.
//!
//! Ids are still accepted, because they are what an *agent* has. A delegating
//! session is given ids, task payloads carry ids, and a script that read one
//! out of `scope list` should not have to translate it back into a name.
//!
//! So this accepts either, and the rule for telling them apart is the only
//! one that cannot be ambiguous: **if it parses as a UUID it is an id,
//! otherwise it is a name.** A scope cannot be named something that parses as
//! a UUID without deliberately choosing to, and if one were, the id reading
//! is the safe one — it addresses exactly one scope or none.
//!
//! Resolution is a `scope.list` query, so the registry stays the one place
//! that knows which scopes exist. This never reads `config.yaml` itself.

use std::path::Path;

use uuid::Uuid;

use crate::rpc::{self, RpcOutcome};

/// What the operator typed: an id, or a name still to be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeRef {
    Id(Uuid),
    Name(String),
}

impl std::str::FromStr for ScopeRef {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match Uuid::parse_str(s) {
            Ok(id) => Self::Id(id),
            Err(_) => Self::Name(s.to_string()),
        })
    }
}

impl std::fmt::Display for ScopeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(id) => write!(f, "{id}"),
            Self::Name(name) => write!(f, "{name}"),
        }
    }
}

/// Turn a [`ScopeRef`] into a scope id, asking the daemon when it is a name.
///
/// An unknown name lists what does exist. An operator who mistypes a scope
/// name is one keystroke from the right answer, and should not have to run a
/// second command to see it.
pub fn resolve(root: &Path, reference: &ScopeRef) -> Result<Uuid, String> {
    let name = match reference {
        ScopeRef::Id(id) => return Ok(*id),
        ScopeRef::Name(name) => name,
    };

    let result = match rpc::query(root, Uuid::nil(), "scope.list", serde_json::json!({})) {
        RpcOutcome::Ok(result) => result,
        RpcOutcome::NotRunning(socket) => {
            return Err(format!(
                "a scope name is resolved against the registry, which only the daemon can \
                 read, and nothing is listening at {}\n  help: run `factory start`, or pass \
                 the scope's id instead of its name",
                socket.display()
            ));
        }
        RpcOutcome::Error(message) => return Err(message),
    };

    let scopes = result
        .get("scopes")
        .and_then(|scopes| scopes.as_array())
        .ok_or_else(|| "the daemon's scope list was not a list".to_string())?;

    let mut known: Vec<&str> = Vec::new();
    for scope in scopes {
        let Some(scope_name) = scope.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        if scope_name == name {
            return scope
                .get("id")
                .and_then(|id| id.as_str())
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or_else(|| format!("scope `{name}` has no usable id"));
        }
        known.push(scope_name);
    }

    known.sort_unstable();
    Err(format!(
        "no registered scope is named `{name}`\n  known scopes: {}\n  help: register it in \
         `.factory/config.yaml`, then run `factory scope reconcile --apply`",
        if known.is_empty() {
            "(none)".to_string()
        } else {
            known.join(", ")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::ScopeRef;

    #[test]
    fn a_uuid_is_read_as_an_id_and_anything_else_as_a_name() {
        assert_eq!(
            "66666666-6666-4666-8666-666666666666"
                .parse::<ScopeRef>()
                .unwrap(),
            ScopeRef::Id(uuid::Uuid::parse_str("66666666-6666-4666-8666-666666666666").unwrap())
        );
        assert_eq!(
            "irrlicht".parse::<ScopeRef>().unwrap(),
            ScopeRef::Name("irrlicht".to_string())
        );
        // Not a UUID despite looking like one — read as a name, which
        // resolves against the registry and simply will not be found.
        assert_eq!(
            "66666666-6666-4666-8666".parse::<ScopeRef>().unwrap(),
            ScopeRef::Name("66666666-6666-4666-8666".to_string())
        );
    }
}
