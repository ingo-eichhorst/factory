//! An origin is producer-owned metadata. Process only keeps and forwards it.
use serde::{Deserialize, Serialize};

/// Opaque to process: it can distinguish an origin from no origin, but has
/// no benchmark run/case/agent/attempt API. Serde preserves legacy object
/// references as well as future string ids, without changing stored/plugin
/// JSON. Interpretation belongs to the producing service, not L4 or L0.
///
/// ```compile_fail
/// use factory_process::origin::OriginRef;
/// let reference = OriginRef::opaque("origin-id");
/// let _ = reference.bench_run_id;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OriginRef(serde_json::Value);

impl OriginRef {
    pub fn opaque(id: impl Into<String>) -> Self {
        Self(serde_json::Value::String(id.into()))
    }

    /// The workspace placement a producer asked for, in L4's own words: `base` (the commit the task's worktree
    /// branches from) and `reset` (a command that runs in the fresh worktree before the agent). Any producer's
    /// reference may carry either; an origin that is not an object, or names neither, has no placement. A hint that
    /// is present but not a string is an error, never silently skipped: a reset that did not run must not look like
    /// one that was not asked for.
    pub fn placement(&self) -> Result<Placement, String> {
        let Some(object) = self.0.as_object() else {
            return Ok(Placement::default());
        };
        let read = |key: &str| match object.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
            Some(other) => Err(format!("the origin's {key:?} must be a string, got {other}")),
        };
        Ok(Placement { base: read("base")?, reset: read("reset")? })
    }
}

/// See [`OriginRef::placement`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    pub base: Option<String>,
    pub reset: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn placement_is_read_from_any_origin_and_an_ill_typed_hint_is_an_error() {
        let with = |wire: serde_json::Value| serde_json::from_value::<OriginRef>(wire).unwrap().placement();
        assert_eq!(with(json!("opaque-id")), Ok(Placement::default()));
        assert_eq!(with(json!({"bench_run_id": "b"})), Ok(Placement::default()), "a legacy origin has none");
        assert_eq!(
            with(json!({"base": "abc", "reset": "make clean"})),
            Ok(Placement { base: Some("abc".into()), reset: Some("make clean".into()) })
        );
        assert_eq!(with(json!({"reset": null})), Ok(Placement::default()));
        assert!(with(json!({"base": 5})).unwrap_err().contains("base"));
    }

    #[test]
    fn legacy_objects_and_future_ids_are_forwarded_without_interpretation() {
        for wire in [
            json!("opaque-id"),
            json!({"bench_run_id":"b", "case_id":"c", "agent":"shell", "attempt":2}),
            json!({"another-producer":{"id":"value"}}),
        ] {
            let reference: OriginRef = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(reference.clone()).unwrap(), wire);
            assert_eq!(serde_json::to_value(reference).unwrap(), wire);
        }
        assert_eq!(
            serde_json::to_value(OriginRef::opaque("id")).unwrap(),
            json!("id")
        );
    }
}
