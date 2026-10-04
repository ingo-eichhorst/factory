use factory_kernel::{
    parse_span, CronSchedule, FactoryError, LaunchKind, LaunchSpec, Schedule, Span,
};
use serde_json::json;

#[test]
fn schedule_preserves_legacy_and_zoned_wire_shapes() {
    for wire in [
        json!({"cron": "0 7 * * 1"}),
        json!({"cron": {"expr": "0 9 * * 1", "timezone": "Europe/Berlin"}}),
        json!({"every": {"seconds": 300}}),
    ] {
        let schedule: Schedule = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(schedule).unwrap(), wire);
    }
    let cron: CronSchedule =
        serde_json::from_value(json!({"expr": "* * * * *", "timezone": " "})).unwrap();
    assert_eq!(cron.timezone, None);
    assert_eq!(serde_json::to_value(cron).unwrap(), json!("* * * * *"));
}

#[test]
fn spans_validate_and_preserve_the_authored_units() {
    for (text, seconds) in [("30s", 30), ("5m", 300), ("2h", 7200), ("28d", 2419200)] {
        let span: Span = serde_json::from_value(json!(text)).unwrap();
        assert_eq!(span.seconds(), seconds);
        assert_eq!(parse_span(text), Ok(seconds));
        assert_eq!(serde_json::to_value(span).unwrap(), json!(text));
    }
    for invalid in ["0s", "1", "2w", "-1m", "18446744073709551615d", "", "🙂"] {
        assert!(parse_span(invalid).is_err(), "{invalid}");
        assert!(serde_json::from_value::<Span>(json!(invalid)).is_err());
    }
}

#[test]
fn launch_shapes_are_adapter_independent_and_backward_compatible() {
    let old: LaunchSpec = serde_json::from_value(json!({"kind": {"named": "shell"}})).unwrap();
    assert_eq!(old.kind, LaunchKind::Named("shell".into()));
    assert!(old.args.is_empty() && old.env.is_empty());
    let wire = json!({
        "kind": {"command": ["sh", "-c"]},
        "args": ["true"], "env": {"FACTORY_RUN_ID": "r1"}
    });
    let launch: LaunchSpec = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(launch).unwrap(), wire);
}

#[test]
fn shared_errors_keep_stable_adapter_and_authorization_codes() {
    assert_eq!(
        FactoryError::adapter("shell", "exit 1").code(),
        "adapter_failed"
    );
    assert_eq!(FactoryError::Denied("no grant".into()).code(), "denied");
    assert_eq!(
        FactoryError::BadRequest("invalid".into()).to_string(),
        "invalid request: invalid"
    );
    assert_eq!(
        FactoryError::DispatchSuperseded("r1".into()).code(),
        "dispatch_superseded"
    );
}
