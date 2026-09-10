//! Turning a domain crate's typed error into ADR 0003 §5's wire shape.
//!
//! Every `From`-style mapping here is this crate's own translation, not a new
//! rule: the *decision* about what went wrong (a task is terminal, a scope is
//! not registered, a session already holds a lease) is made once, by the
//! domain crate that returned the error. This module only ever chooses a
//! stable `code`, a safe `message`, and whether the client may retry — it
//! never re-derives what the error means.

use serde_json::Value;

use crate::envelope::ErrorBody;

/// Build an [`ErrorBody`] directly, for a failure this crate itself detects
/// (a malformed payload, a field this crate cannot resolve) rather than one a
/// domain crate returned.
#[must_use]
pub fn err(code: &str, message: impl Into<String>) -> ErrorBody {
    ErrorBody {
        code: code.to_string(),
        message: message.into(),
        retryable: false,
        details: serde_json::json!({}),
    }
}

/// The same, with a `details` object — used when a caller benefits from
/// structured corrective data (ADR 0003 §5) rather than only the message.
#[must_use]
pub fn err_with_details(code: &str, message: impl Into<String>, details: Value) -> ErrorBody {
    ErrorBody {
        code: code.to_string(),
        message: message.into(),
        retryable: false,
        details,
    }
}

/// `validation.missing_field`, naming the field — never a silent default for
/// a field this crate's own payload contract requires (see `lib.rs`'s payload
/// table).
#[must_use]
pub fn missing_field(field: &str) -> ErrorBody {
    err_with_details(
        "validation.missing_field",
        format!("payload is missing required field `{field}`"),
        serde_json::json!({ "field": field }),
    )
}

/// `validation.malformed_payload` — the payload parsed as JSON but did not
/// match the operation's schema (wrong type, unknown field, malformed UUID).
#[must_use]
pub fn malformed_payload(detail: impl std::fmt::Display) -> ErrorBody {
    err(
        "validation.malformed_payload",
        format!("payload did not match this operation's schema: {detail}"),
    )
}

/// `validation.unknown_operation` — a `command`/`query` name outside decision
/// 9's fixed vocabulary.
#[must_use]
pub fn unknown_operation(kind: &str, name: &str) -> ErrorBody {
    err(
        "validation.unknown_operation",
        format!(
            "unknown {kind} `{name}`; see `lib.rs`'s decision 9 table for the fixed vocabulary"
        ),
    )
}

/// Deserialize `payload` into `T`, or a [`malformed_payload`] error naming
/// what did not match. Every operation's payload struct uses
/// `#[serde(deny_unknown_fields)]` so a field name mismatch with a client is
/// diagnosable here rather than silently ignored.
pub fn parse_payload<T: serde::de::DeserializeOwned>(payload: &Value) -> Result<T, ErrorBody> {
    serde_json::from_value(payload.clone()).map_err(malformed_payload)
}

// --- Domain error mappings --------------------------------------------

pub fn session_error(e: factory_session::SessionError) -> ErrorBody {
    use factory_session::SessionError as E;
    match e {
        E::NotFound(id) => err("not_found.session", format!("no session with id {id}")),
        E::TaskNotFound(id) => err("not_found.task", format!("no task with id {id}")),
        E::WorkspaceLeased { .. } => err("conflict.workspace_leased", e.to_string()),
        E::MaxSessionsReached { .. } => err("conflict.max_sessions_reached", e.to_string()),
        E::TaskNotTerminal { .. } => err("conflict.task_not_terminal", e.to_string()),
        E::InvalidTransition { .. } => err("conflict.invalid_session_transition", e.to_string()),
        E::Path(_) | E::Store(_) => err("internal.session_error", e.to_string()),
    }
}

pub fn task_error(e: factory_task::TaskError) -> ErrorBody {
    use factory_task::TaskError as E;
    match e {
        E::NotFound(id) => err("not_found.task", format!("no task with id {id}")),
        E::AlreadyTerminal { .. } => err("conflict.task_already_terminal", e.to_string()),
        E::NotBlocked { .. } => err("conflict.task_not_blocked", e.to_string()),
        E::InvalidTransition { .. } => err("conflict.invalid_task_transition", e.to_string()),
        E::ResultSummaryTooLarge { .. } | E::ResultArtifactPathsTooLarge { .. } => {
            err("validation.result_too_large", e.to_string())
        }
        // A schedule that vanished between being read as due and being
        // stamped. The foreign key on the run just inserted rules this out
        // in practice, so it gets its own code rather than reading as a
        // database fault to an operator who would find nothing wrong with
        // the database.
        E::ScheduleVanished(_) => err("conflict.schedule_vanished", e.to_string()),
        E::Store(_) => err("internal.store_error", e.to_string()),
    }
}

pub fn assign_error(e: factory_task::assign::AssignError) -> ErrorBody {
    use factory_task::assign::AssignError as E;
    match e {
        E::TaskNotFound(id) => err("not_found.task", format!("no task with id {id}")),
        E::NotQueued { .. } => err("conflict.task_not_queued", e.to_string()),
        E::Session(inner) => session_error(inner),
        E::Path(_) | E::Store(_) => err("internal.store_error", e.to_string()),
    }
}

pub fn deliver_error(e: factory_task::deliver::DeliverError) -> ErrorBody {
    use factory_task::deliver::DeliverError as E;
    match e {
        E::NotFound(id) => err("not_found.task", format!("no task with id {id}")),
        E::NotQueued { .. } => err("conflict.task_not_queued", e.to_string()),
        E::NotAssigned(_) => err("conflict.task_not_assigned", e.to_string()),
        E::AlreadyAttempted(_) => err("conflict.delivery_already_attempted", e.to_string()),
        E::WriteFailed { .. } => err("unavailable.delivery_failed", e.to_string()),
        E::NotDelivered(_) => err("conflict.task_not_delivered", e.to_string()),
        E::InvalidTransition { .. } => err("conflict.invalid_task_transition", e.to_string()),
        E::Store(_) => err("internal.store_error", e.to_string()),
    }
}

pub fn complete_error(e: factory_task::complete::CompleteError) -> ErrorBody {
    use factory_task::complete::CompleteError as E;
    match e {
        E::Task(inner) => task_error(inner),
        E::NoResult(_) => err("validation.no_result", e.to_string()),
        E::NotRunning { .. } => err("conflict.task_not_running", e.to_string()),
        E::CancellationNotRequested(_) => err("conflict.cancellation_not_requested", e.to_string()),
    }
}

pub fn delegation_error(e: factory_delegation::rule::DelegationError) -> ErrorBody {
    use factory_delegation::rule::DelegationError as E;
    match e {
        E::Registry(inner) => registry_error(inner),
        E::Session(inner) => session_error(inner),
        E::Task(inner) => task_error(inner),
        E::NotEligible { .. } => err("authorization.scope_not_eligible", e.to_string()),
        E::AlreadyInChain { .. } => err("conflict.already_in_delegation_chain", e.to_string()),
    }
}

pub fn registry_error(e: factory_registry::RegistryError) -> ErrorBody {
    use factory_registry::RegistryError as E;
    match e {
        E::UnknownScope { id } => err("not_found.scope", format!("scope `{id}` is not registered")),
        E::EscapesInstance { .. } => err("validation.scope_escapes_instance", e.to_string()),
        E::CyclicParentage { .. } => err("internal.cyclic_parentage", e.to_string()),
        E::Path(_) | E::Store(_) => err("internal.registry_error", e.to_string()),
    }
}

pub fn context_error(e: factory_context::ContextError) -> ErrorBody {
    use factory_context::ContextError as E;
    match e {
        E::UnreadableSource { .. } => err("unavailable.context_source_unreadable", e.to_string()),
        E::NoScopes => err("internal.no_scopes", e.to_string()),
    }
}

pub fn adapter_error(e: factory_adapter::AdapterError) -> ErrorBody {
    use factory_adapter::AdapterError as E;
    match e {
        E::RuntimeUnavailable { .. } => err("unavailable.harness_runtime", e.to_string()),
        E::UnreadableOutput { .. } => err("unavailable.harness_output", e.to_string()),
        E::WrongHarness { .. } => err("conflict.wrong_harness", e.to_string()),
        E::SubmissionUnconfirmed { .. } => err("unavailable.submission_unconfirmed", e.to_string()),
        E::SessionBusy { .. } => err("conflict.session_busy", e.to_string()),
    }
}

pub fn config_error(e: factory_config::ConfigError) -> ErrorBody {
    err("internal.config_error", e.to_string())
}

pub fn recovery_error(e: factory_recovery::restore::RecoveryError) -> ErrorBody {
    err("internal.recovery_error", e.to_string())
}
