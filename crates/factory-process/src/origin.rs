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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
