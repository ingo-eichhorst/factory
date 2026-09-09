//! The wire envelopes: ADR 0003 §§2–5's field tables, minus `idempotency_key`
//! — see the crate docs for why that field is absent.
//!
//! Two mechanisms do the work here:
//!
//! - [`Request`] and [`Response`] are `#[serde(tag = "api")]` enums, so a
//!   well-formed envelope serializes and deserializes in one step, in both
//!   directions, with no hand-written matching of the `api` string.
//! - [`parse_request`] and [`parse_response`] exist *on top of* that, because
//!   a malformed or unrecognized envelope needs a stable `validation.*` code
//!   and — per ADR 0003 §5 — the originating `request_id` echoed back when it
//!   can be recovered, and serde's own error on an unknown tag gives neither.
//!   They peek at the raw JSON first for exactly that reason, then hand the
//!   real parse to the derived `Deserialize` impl.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Exact `api` value for a command request (ADR 0003 §2).
pub const COMMAND_API: &str = "factory.command/v1";
/// Exact `api` value for a query request (ADR 0003 §3).
pub const QUERY_API: &str = "factory.query/v1";
/// Exact `api` value for a successful response (ADR 0003 §4).
pub const RESPONSE_API: &str = "factory.response/v1";
/// Exact `api` value for an error response (ADR 0003 §5).
pub const ERROR_API: &str = "factory.error/v1";

/// ADR 0003 §2's command envelope, without `idempotency_key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRequest {
    #[serde(with = "uuid_str")]
    pub request_id: Uuid,
    #[serde(with = "uuid_str")]
    pub scope_id: Uuid,
    pub command: String,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}

/// ADR 0003 §3's query envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryRequest {
    #[serde(with = "uuid_str")]
    pub request_id: Uuid,
    #[serde(with = "uuid_str")]
    pub scope_id: Uuid,
    pub query: String,
    pub payload: Value,
}

/// A request, still tagged by which envelope it arrived as.
///
/// Serializing this produces exactly ADR 0003's `{"api": ..., ...}` shape;
/// deserializing it accepts exactly that shape. Use [`parse_request`] instead
/// of this type's `Deserialize` impl directly when the input is untrusted —
/// see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "api")]
pub enum Request {
    #[serde(rename = "factory.command/v1")]
    Command(CommandRequest),
    #[serde(rename = "factory.query/v1")]
    Query(QueryRequest),
}

impl Request {
    /// The `request_id` of either variant.
    #[must_use]
    pub fn request_id(&self) -> Uuid {
        match self {
            Request::Command(request) => request.request_id,
            Request::Query(request) => request.request_id,
        }
    }
}

/// ADR 0003 §4's success envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuccessResponse {
    #[serde(with = "uuid_str")]
    pub request_id: Uuid,
    pub event_cursor: u64,
    pub result: Value,
}

/// ADR 0003 §5's error body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: Value,
}

/// ADR 0003 §5's error envelope. `request_id` is nullable: absent or explicit
/// `null` both decode to `None`, which is what "could not be decoded" means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(default, with = "opt_uuid_str")]
    pub request_id: Option<Uuid>,
    pub error: ErrorBody,
}

/// A response, still tagged by which envelope it arrived as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "api")]
pub enum Response {
    #[serde(rename = "factory.response/v1")]
    Success(SuccessResponse),
    #[serde(rename = "factory.error/v1")]
    Error(ErrorEnvelope),
}

/// Why a line on the wire could not be turned into a [`Request`].
///
/// `request_id` is populated whenever the raw JSON has a well-formed
/// `request_id` field, even when everything else about the envelope is
/// invalid — that's the whole reason `parse_request` peeks at the value
/// before handing it to `Request`'s derived `Deserialize`, which would
/// otherwise report a schema error with nothing usable to echo back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseFailure {
    pub request_id: Option<Uuid>,
    pub code: String,
    pub message: String,
}

/// Parse one line of input as a [`Request`], with a stable `validation.*`
/// code on failure. See the module docs for why this exists alongside
/// `Request`'s own `Deserialize` impl.
pub fn parse_request(line: &str) -> Result<Request, ParseFailure> {
    let value: Value = serde_json::from_str(line).map_err(|source| ParseFailure {
        request_id: None,
        code: "validation.malformed_json".to_string(),
        message: format!("could not parse request as JSON: {source}"),
    })?;
    let request_id = extract_request_id(&value);

    match value.get("api").and_then(Value::as_str) {
        Some(COMMAND_API | QUERY_API) => {
            serde_json::from_value::<Request>(value).map_err(|source| ParseFailure {
                request_id,
                code: "validation.malformed_request".to_string(),
                message: format!("request did not match its envelope schema: {source}"),
            })
        }
        Some(other) => Err(ParseFailure {
            request_id,
            code: "validation.unknown_api".to_string(),
            message: format!(
                "unknown \"api\" {other:?}; expected {COMMAND_API:?} or {QUERY_API:?}"
            ),
        }),
        None => Err(ParseFailure {
            request_id,
            code: "validation.missing_api".to_string(),
            message: "request is missing the required \"api\" field".to_string(),
        }),
    }
}

/// Parse one line of input as a [`Response`], returning a human-readable
/// reason on failure. Used by the client, which has no wire-level error-code
/// family of its own to report a malformed response through — see
/// [`crate::client::ClientError::Protocol`].
pub fn parse_response(line: &str) -> Result<Response, String> {
    let value: Value = serde_json::from_str(line)
        .map_err(|source| format!("could not parse response as JSON: {source}"))?;

    match value.get("api").and_then(Value::as_str) {
        Some(RESPONSE_API | ERROR_API) => serde_json::from_value::<Response>(value)
            .map_err(|source| format!("response did not match its envelope schema: {source}")),
        Some(other) => Err(format!(
            "unexpected response \"api\" {other:?}; expected {RESPONSE_API:?} or {ERROR_API:?}"
        )),
        None => Err("response is missing the required \"api\" field".to_string()),
    }
}

fn extract_request_id(value: &Value) -> Option<Uuid> {
    value
        .get("request_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// `Uuid <-> String` for a required field, since the `uuid` crate's `serde`
/// feature is not enabled in this workspace (checked in `Cargo.lock`: `uuid`
/// carries no `serde` dependency here). ADR 0003 §1 requires UUIDv7 stable
/// IDs; the wire representation for all of them is a plain JSON string.
mod uuid_str {
    use serde::{Deserialize, Deserializer, Serializer};
    use uuid::Uuid;

    pub fn serialize<S: Serializer>(id: &Uuid, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(id)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Uuid, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Uuid::parse_str(&raw).map_err(serde::de::Error::custom)
    }
}

/// `Option<Uuid> <-> String | null` for the one nullable UUID field on the
/// wire ([`ErrorEnvelope::request_id`]). Paired with `#[serde(default, ...)]`
/// so a key that is simply absent — not just explicit `null` — still decodes
/// to `None`; `with` overrides serde's built-in "missing `Option` field is
/// `None`" behaviour, so the `default` has to say it again.
mod opt_uuid_str {
    use serde::{Deserialize, Deserializer, Serializer};
    use uuid::Uuid;

    pub fn serialize<S: Serializer>(id: &Option<Uuid>, serializer: S) -> Result<S::Ok, S::Error> {
        match id {
            Some(id) => serializer.collect_str(id),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Uuid>, D::Error> {
        let raw: Option<String> = Option::deserialize(deserializer)?;
        raw.map(|s| Uuid::parse_str(&s).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(seed: u8) -> Uuid {
        Uuid::from_bytes([seed; 16])
    }

    #[test]
    fn command_request_round_trips_without_idempotency_key() {
        let request = Request::Command(CommandRequest {
            request_id: uuid(1),
            scope_id: uuid(2),
            command: "task.create".to_string(),
            payload: serde_json::json!({"title": "Generate weekly report"}),
            expected_revision: Some(7),
        });

        let line = serde_json::to_string(&request).unwrap();
        assert!(
            !line.contains("idempotency_key"),
            "the wire envelope must never carry idempotency_key: {line}"
        );
        assert!(line.contains("\"api\":\"factory.command/v1\""));

        let parsed = parse_request(&line).unwrap();
        assert_eq!(parsed, request);
    }

    #[test]
    fn query_request_round_trips() {
        let request = Request::Query(QueryRequest {
            request_id: uuid(3),
            scope_id: uuid(4),
            query: "task.get".to_string(),
            payload: serde_json::json!({"task_id": uuid(5).to_string()}),
        });

        let line = serde_json::to_string(&request).unwrap();
        let parsed = parse_request(&line).unwrap();
        assert_eq!(parsed, request);
    }

    #[test]
    fn an_idempotency_key_sent_by_a_client_is_silently_ignored_not_rejected() {
        // The decision is to leave the field out of the type, not to reject
        // a client that sends it anyway (crate docs decision 7).
        let line = serde_json::json!({
            "api": "factory.command/v1",
            "request_id": uuid(1).to_string(),
            "idempotency_key": uuid(9).to_string(),
            "scope_id": uuid(2).to_string(),
            "command": "task.create",
            "payload": {}
        })
        .to_string();

        let parsed = parse_request(&line).unwrap();
        assert_eq!(parsed.request_id(), uuid(1));
    }

    #[test]
    fn missing_api_field_is_a_stable_validation_code_with_request_id_recovered() {
        let line = serde_json::json!({
            "request_id": uuid(1).to_string(),
            "scope_id": uuid(2).to_string(),
            "command": "task.create",
            "payload": {}
        })
        .to_string();

        let failure = parse_request(&line).unwrap_err();
        assert_eq!(failure.code, "validation.missing_api");
        assert_eq!(failure.request_id, Some(uuid(1)));
    }

    #[test]
    fn unknown_api_field_is_a_stable_validation_code() {
        let line = serde_json::json!({
            "api": "factory.response/v1",
            "request_id": uuid(1).to_string(),
        })
        .to_string();

        let failure = parse_request(&line).unwrap_err();
        assert_eq!(failure.code, "validation.unknown_api");
        assert_eq!(failure.request_id, Some(uuid(1)));
    }

    #[test]
    fn malformed_json_is_a_stable_validation_code_with_no_request_id() {
        let failure = parse_request("not json at all").unwrap_err();
        assert_eq!(failure.code, "validation.malformed_json");
        assert_eq!(failure.request_id, None);
    }

    #[test]
    fn command_envelope_missing_a_required_field_is_malformed_request() {
        let line = serde_json::json!({
            "api": "factory.command/v1",
            "request_id": uuid(1).to_string(),
            "scope_id": uuid(2).to_string(),
            "payload": {}
            // "command" is missing
        })
        .to_string();

        let failure = parse_request(&line).unwrap_err();
        assert_eq!(failure.code, "validation.malformed_request");
        assert_eq!(failure.request_id, Some(uuid(1)));
    }

    #[test]
    fn success_response_round_trips() {
        let response = Response::Success(SuccessResponse {
            request_id: uuid(1),
            event_cursor: 1842,
            result: serde_json::json!({"task_id": uuid(2).to_string(), "status": "running"}),
        });

        let line = serde_json::to_string(&response).unwrap();
        let parsed = parse_response(&line).unwrap();
        assert_eq!(parsed, response);
    }

    #[test]
    fn error_response_round_trips_with_null_request_id() {
        let response = Response::Error(ErrorEnvelope {
            request_id: None,
            error: ErrorBody {
                code: "validation.malformed_json".to_string(),
                message: "could not parse request as JSON".to_string(),
                retryable: false,
                details: serde_json::json!({}),
            },
        });

        let line = serde_json::to_string(&response).unwrap();
        assert!(line.contains("\"request_id\":null"));
        let parsed = parse_response(&line).unwrap();
        assert_eq!(parsed, response);
    }

    #[test]
    fn error_response_with_absent_request_id_key_still_parses() {
        // Not just explicit null: a hand-written envelope that omits the key
        // entirely must also decode to None (see opt_uuid_str's docs).
        let line = serde_json::json!({
            "api": "factory.error/v1",
            "error": {
                "code": "internal.unexpected",
                "message": "boom",
                "retryable": false,
                "details": {}
            }
        })
        .to_string();

        let parsed = parse_response(&line).unwrap();
        assert_eq!(
            parsed,
            Response::Error(ErrorEnvelope {
                request_id: None,
                error: ErrorBody {
                    code: "internal.unexpected".to_string(),
                    message: "boom".to_string(),
                    retryable: false,
                    details: serde_json::json!({}),
                },
            })
        );
    }

    #[test]
    fn malformed_response_is_reported_not_panicked_on() {
        assert!(parse_response("not json").is_err());
        assert!(parse_response("{}").is_err());
        assert!(parse_response(r#"{"api":"factory.command/v1"}"#).is_err());
    }
}
