//! Canonical compatibility path for the L5 registry vocabulary.
pub use factory_assurance::metrics::*;

#[cfg(test)]
mod integration_tests {
    #[test]
    fn an_older_metrics_request_without_scope_or_window_still_deserializes() {
        let request: crate::protocol::Request = serde_json::from_value(serde_json::json!({
            "op": "metrics",
            "params": { "ids": ["throughput_week"] }
        }))
        .unwrap();
        assert!(matches!(
            request,
            crate::protocol::Request::Metrics {
                scope: None,
                window: None,
                ..
            }
        ));
    }
}
