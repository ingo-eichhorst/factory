//! The HTTP transport's framing and its three guards (ADR 0023).
//!
//! These tests speak raw HTTP over a real loopback socket rather than calling
//! the parser directly, because the thing under test is exactly what a
//! browser puts on the wire: a request line, some headers, and a body whose
//! length the headers declare.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;

use factory_daemon::FactoryHandler;
use factory_daemon::envelope::{CommandRequest, QueryRequest};
use factory_daemon::server::{Handler, HandlerOutcome, HandlerSuccess};

mod common;

/// A handler that answers every query and refuses every command, so a test
/// can tell the two paths apart without a store behind them.
struct StubHandler;

impl Handler for StubHandler {
    fn handle_command(&self, _request: CommandRequest) -> HandlerOutcome {
        Ok(HandlerSuccess {
            event_cursor: 7,
            result: serde_json::json!({ "stub": "command" }),
        })
    }

    fn handle_query(&self, request: QueryRequest) -> HandlerOutcome {
        Ok(HandlerSuccess {
            event_cursor: 3,
            result: serde_json::json!({ "stub": request.query }),
        })
    }
}

/// Bind an ephemeral loopback port and serve the stub on it. The serving
/// thread is deliberately not joined: it accepts forever, and the test
/// process ending is what stops it.
fn serve() -> SocketAddr {
    let ui = factory_daemon::Ui::bind("127.0.0.1:0".parse().expect("a literal address parses"))
        .expect("binding an ephemeral loopback port");
    let addr = ui.addr();
    let _join = ui.spawn(Arc::new(StubHandler));
    addr
}

/// Send one raw request and read the whole response back as text.
fn request(addr: SocketAddr, raw: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("connecting to the test listener");
    stream
        .write_all(raw.as_bytes())
        .expect("writing the request");
    stream.flush().expect("flushing the request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("reading the response");
    response
}

fn status_line(response: &str) -> &str {
    response.lines().next().unwrap_or("")
}

fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// The same as [`body`], named so the end-to-end test below reads clearly
/// next to its own `body` local.
fn body_of(response: &str) -> &str {
    body(response)
}

fn envelope_line() -> String {
    serde_json::json!({
        "api": "factory.query/v1",
        "request_id": "11111111-1111-4111-8111-111111111111",
        "scope_id": "22222222-2222-4222-8222-222222222222",
        "query": "scope.list",
        "payload": {}
    })
    .to_string()
}

fn post_api(addr: SocketAddr, headers: &str, body: &str) -> String {
    request(
        addr,
        &format!(
            "POST /api HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            addr.port(),
            body.len()
        ),
    )
}

#[test]
fn the_root_path_serves_the_page() {
    let addr = serve();
    let response = request(
        addr,
        &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n", addr.port()),
    );

    assert!(
        status_line(&response).starts_with("HTTP/1.1 200"),
        "expected 200, got {:?}",
        status_line(&response)
    );
    assert!(
        response.contains("Content-Type: text/html; charset=utf-8"),
        "the page must be served as HTML, got {response:?}"
    );
    assert!(
        body(&response).contains("Factory Leitstand"),
        "the served page must be the UI, not a placeholder"
    );
}

#[test]
fn the_api_path_carries_the_same_envelope_the_socket_takes() {
    let addr = serve();
    let response = post_api(addr, "Content-Type: application/json\r\n", &envelope_line());

    assert!(
        status_line(&response).starts_with("HTTP/1.1 200"),
        "expected 200, got {:?}",
        status_line(&response)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(body(&response)).expect("the body is the response envelope");
    assert_eq!(parsed["api"], "factory.response/v1");
    assert_eq!(parsed["result"]["stub"], "scope.list");
}

#[test]
fn a_request_that_does_not_parse_comes_back_as_an_error_envelope_not_a_dropped_connection() {
    let addr = serve();
    let response = post_api(addr, "Content-Type: application/json\r\n", "{not json");

    assert!(
        status_line(&response).starts_with("HTTP/1.1 200"),
        "a malformed envelope is an application-level error, not a transport failure"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(body(&response)).expect("the body is still an envelope");
    assert_eq!(parsed["error"]["code"], "validation.malformed_json");
}

#[test]
fn a_host_header_naming_anything_but_the_bound_loopback_is_refused() {
    let addr = serve();
    // DNS rebinding: the attacker's own name, resolved to 127.0.0.1, reaches
    // this port with the attacker's origin. The browser puts their name here.
    let response = request(
        addr,
        &format!(
            "POST /api HTTP/1.1\r\nHost: rebind.example.com:{}\r\nContent-Type: \
             application/json\r\nContent-Length: 2\r\n\r\n{{}}",
            addr.port()
        ),
    );

    assert!(
        status_line(&response).starts_with("HTTP/1.1 421"),
        "expected 421, got {:?}",
        status_line(&response)
    );
}

#[test]
fn a_host_header_naming_the_wrong_port_is_refused() {
    let addr = serve();
    let response = request(
        addr,
        "POST /api HTTP/1.1\r\nHost: 127.0.0.1:1\r\nContent-Type: application/json\r\n\
         Content-Length: 2\r\n\r\n{}",
    );

    assert!(
        status_line(&response).starts_with("HTTP/1.1 421"),
        "expected 421, got {:?}",
        status_line(&response)
    );
}

#[test]
fn a_foreign_origin_is_refused() {
    let addr = serve();
    let response = post_api(
        addr,
        "Content-Type: application/json\r\nOrigin: https://hostile.example\r\n",
        &envelope_line(),
    );

    assert!(
        status_line(&response).starts_with("HTTP/1.1 403"),
        "expected 403, got {:?}",
        status_line(&response)
    );
}

#[test]
fn the_pages_own_origin_is_accepted() {
    let addr = serve();
    let response = post_api(
        addr,
        &format!(
            "Content-Type: application/json\r\nOrigin: http://127.0.0.1:{}\r\n",
            addr.port()
        ),
        &envelope_line(),
    );

    assert!(
        status_line(&response).starts_with("HTTP/1.1 200"),
        "expected 200, got {:?}",
        status_line(&response)
    );
}

#[test]
fn a_body_that_is_not_json_is_refused_before_it_reaches_the_handler() {
    let addr = serve();
    // `text/plain` is one of the three content types a cross-origin `fetch`
    // may send without a preflight. Requiring `application/json` is what
    // makes the preflight — and therefore our 405 — unavoidable for a
    // hostile page.
    let response = post_api(addr, "Content-Type: text/plain\r\n", &envelope_line());

    assert!(
        status_line(&response).starts_with("HTTP/1.1 415"),
        "expected 415, got {:?}",
        status_line(&response)
    );
}

#[test]
fn a_preflight_is_not_answered() {
    let addr = serve();
    let response = request(
        addr,
        &format!(
            "OPTIONS /api HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: http://127.0.0.1:{}\r\n\r\n",
            addr.port(),
            addr.port()
        ),
    );

    assert!(
        status_line(&response).starts_with("HTTP/1.1 405"),
        "a CORS preflight must not be granted, got {:?}",
        status_line(&response)
    );
    assert!(
        !response
            .to_ascii_lowercase()
            .contains("access-control-allow"),
        "no CORS header may ever be emitted"
    );
}

#[test]
fn an_unknown_path_is_not_found() {
    let addr = serve();
    let response = request(
        addr,
        &format!(
            "GET /../../etc/passwd HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            addr.port()
        ),
    );

    assert!(
        status_line(&response).starts_with("HTTP/1.1 404"),
        "expected 404, got {:?}",
        status_line(&response)
    );
}

#[test]
fn a_routable_address_is_refused_at_bind() {
    let error = factory_daemon::Ui::bind("0.0.0.0:0".parse().expect("a literal address parses"))
        .expect_err("binding a routable address must fail");

    assert!(
        matches!(error, factory_daemon::UiError::NotLoopback(_)),
        "expected NotLoopback, got {error:?}"
    );
}

/// Serve a real [`FactoryHandler`] over a fixture instance, and run exactly
/// the sequence of queries the served page runs on every poll.
///
/// This is the test that protects the page rather than the transport: it
/// fails the moment a query stops answering, renames a field the page reads,
/// or starts demanding a payload the page does not send. The order below is
/// the order in `ui/index.html`'s `load()`, and the assertions name the
/// fields that page actually indexes into.
#[test]
fn the_served_pages_own_query_sequence_answers_over_http() {
    let fixture = common::build(&[
        common::ScopeSpec::new("alpha", "alpha"),
        common::ScopeSpec::new("beta", "beta"),
    ]);
    let store =
        factory_store::Store::open(fixture.instance_root()).expect("open the fixture store");
    let handler = FactoryHandler::new(store, common::FakeAdapter::new(), fixture.instance_root());

    let ui = factory_daemon::Ui::bind("127.0.0.1:0".parse().expect("a literal address parses"))
        .expect("binding an ephemeral loopback port");
    let addr = ui.addr();
    let _join = ui.spawn(Arc::new(handler));

    let ask = |name: &str, payload: serde_json::Value, scope: &str| -> serde_json::Value {
        let body = serde_json::json!({
            "api": "factory.query/v1",
            // A fixed id: nothing here correlates responses, and `uuid`'s v4
            // feature is deliberately not enabled in this workspace.
            "request_id": "33333333-3333-4333-8333-333333333333",
            "scope_id": scope,
            "query": name,
            "payload": payload,
        })
        .to_string();
        let response = post_api(addr, "Content-Type: application/json\r\n", &body);
        let parsed: serde_json::Value =
            serde_json::from_str(body_of(&response)).expect("a response envelope");
        assert_eq!(
            parsed["api"], "factory.response/v1",
            "{name} did not succeed: {parsed}"
        );
        parsed["result"].clone()
    };

    let nil = uuid::Uuid::nil().to_string();

    // 1. daemon.status — the wire bar and the whole of L1.
    let status = ask("daemon.status", serde_json::json!({}), &nil);
    for field in [
        "schema_version",
        "database_path",
        "socket_path",
        "lock_path",
    ] {
        assert!(
            !status[field].is_null(),
            "daemon.status must carry `{field}`: {status}"
        );
    }

    // 2. scope.list — the L3 tree, nested by `parent_id`.
    let scopes = ask("scope.list", serde_json::json!({}), &nil);
    let scopes = scopes["scopes"].as_array().expect("scopes is an array");
    assert_eq!(scopes.len(), 3, "one root plus two children: {scopes:?}");
    let root = scopes
        .iter()
        .find(|s| s["parent_id"].is_null())
        .expect("exactly the root scope has no parent");
    for field in ["id", "name", "declared_path"] {
        assert!(!root[field].is_null(), "scope.list must carry `{field}`");
    }
    let root_id = root["id"].as_str().expect("an id string").to_string();

    // 3. agent.list — asked as the root scope, the way the page asks it.
    let agents = ask(
        "agent.list",
        serde_json::json!({ "scope_id": root_id }),
        &root_id,
    );
    let rows = agents["scopes"].as_array().expect("scopes is an array");
    assert_eq!(rows.len(), 3, "one row per registered scope: {rows:?}");
    for row in rows {
        for field in ["scope_id", "scope_name", "targetable", "agents"] {
            assert!(
                !row[field].is_null(),
                "agent.list row must carry `{field}`: {row}"
            );
        }
    }

    // 4. agent.status, once per scope — the reason the page counts its calls.
    for row in rows {
        let scope_id = row["scope_id"].as_str().expect("a scope id");
        let sessions = ask("agent.status", serde_json::json!({}), scope_id);
        assert!(
            sessions["sessions"].is_array(),
            "agent.status must carry `sessions` even when empty: {sessions}"
        );
    }

    // 5. task.list and 6. schedule.list — L4 and the rest of L1.
    let tasks = ask("task.list", serde_json::json!({}), &nil);
    assert!(tasks["tasks"].is_array(), "task.list must carry `tasks`");
    let schedules = ask("schedule.list", serde_json::json!({}), &nil);
    assert!(
        schedules["schedules"].is_array(),
        "schedule.list must carry `schedules`"
    );
}
