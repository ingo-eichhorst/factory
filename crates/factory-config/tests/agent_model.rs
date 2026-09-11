//! `model:` on an agent (ADR 0024): optional, opaque, and passed through.
//!
//! What is tested here is the *boundary*, not a model catalogue. Factory
//! stores the name a scope wrote and hands it to the harness; whether the
//! harness knows it is the harness's answer to give, so there is deliberately
//! no test asserting that some particular name is valid.

use factory_config::Harness;

const BASE: &str = r#"
version: 1
instance:
  id: f72da97f-140d-4f88-839b-685424441ed7
  name: model-test
scopes:
  - id: 8444ec60-970a-483d-9fcd-ef10938a1581
    name: demo
    path: projects/demo
    agent:
      name: builder
      harness: pi
"#;

fn parse(extra: &str) -> Result<factory_config::InstanceConfig, factory_config::ConfigError> {
    factory_config::parse(&format!("{BASE}{extra}"), "config.yaml")
}

#[test]
fn an_agent_without_a_model_names_none() {
    let config = parse("").expect("the base fixture is valid");
    let agent = &config.scopes[0].agents[0];

    assert_eq!(agent.harness, Harness::Pi);
    assert_eq!(
        agent.model, None,
        "no `model:` means the harness's own configuration decides, which is \
         what every scope did before this key existed"
    );
}

#[test]
fn a_model_is_stored_exactly_as_written() {
    let config = parse("      model: business-factory-qwen3.8/qwen3.8-27b\n")
        .expect("naming a model is valid");

    assert_eq!(
        config.scopes[0].agents[0].model.as_deref(),
        Some("business-factory-qwen3.8/qwen3.8-27b"),
        "the provider/id form Pi accepts must survive unparsed — this crate \
         never splits, normalises or resolves the name"
    );
}

#[test]
fn a_model_name_this_crate_has_never_heard_of_is_still_accepted() {
    // The point of the field: a harness knows models Factory does not, and a
    // catalogue here would reject names the harness would have taken.
    let config = parse("      model: some-provider/a-model-shipped-tomorrow\n")
        .expect("an unknown name is the harness's to refuse, not this crate's");

    assert_eq!(
        config.scopes[0].agents[0].model.as_deref(),
        Some("some-provider/a-model-shipped-tomorrow")
    );
}

#[test]
fn an_empty_model_is_refused_with_the_key_named() {
    let error = parse("      model: \"\"\n").expect_err("an empty model must not load");

    assert!(
        error.to_string().contains("model"),
        "the message must name the key that is wrong: {error}"
    );
}

#[test]
fn a_blank_model_is_refused_the_same_way() {
    let error = parse("      model: \"   \"\n").expect_err("whitespace is not a model name");

    assert!(error.to_string().contains("model"), "{error}");
}

#[test]
fn a_misspelled_model_key_is_reported_against_the_agent_field_list() {
    let error = parse("      modell: whatever\n").expect_err("`modell` is not a key");
    let text = error.to_string();

    assert!(
        text.contains("modell"),
        "the message must quote what was written: {text}"
    );
    assert!(
        text.contains("model"),
        "and offer the key that was meant: {text}"
    );
}
