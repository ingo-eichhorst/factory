//! The `http` interface: REST over the same request envelope, a WebSocket for
//! the event stream, and the web UI on `/`.
//!
//! Bound to loopback unless the config says otherwise. Nothing here decides
//! anything -- every route builds a `Request` and hands it to the engine.

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response as AxumResponse};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use factory_core::adapter::interface::{Interface, InterfaceContext};
use factory_core::config::ScopeAgent;
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::{Envelope, Payload, ProductionBin, Request, Response};
use factory_core::role::RoleSpec;
use factory_core::task::{NewTask, TaskFilter, TaskPatch, TaskReport};
use factory_core::workflow::WorkflowDraft;
use futures_util::{sink::SinkExt, stream::StreamExt};
use std::sync::Arc;

use crate::engine::Engine;

pub const DEFAULT_BIND: &str = "127.0.0.1:8787";

pub struct HttpInterface;

#[async_trait]
impl Interface<Engine> for HttpInterface {
    fn name(&self) -> &str {
        "http"
    }

    fn description(&self) -> String {
        "REST, a WebSocket event stream, and the web UI".into()
    }

    async fn serve(
        self: Arc<Self>,
        engine: Arc<Engine>,
        ctx: InterfaceContext,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        let bind = ctx
            .config
            .string("bind")
            .unwrap_or_else(|| DEFAULT_BIND.to_string());

        let app = router(engine);

        let listener = tokio::net::TcpListener::bind(&bind)
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("binding {bind}: {e}")))?;
        let addr = listener
            .local_addr()
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("reading bound address: {e}")))?;

        if addr.ip().is_loopback() {
            tracing::info!(url = %format!("http://{addr}"), "http interface listening");
        } else {
            // Bound past loopback, and this interface has no authentication:
            // anyone who can reach it can start a task, and a task runs
            // commands as whoever runs the daemon. Say so every time, with the
            // address a person would actually type.
            tracing::warn!(
                "http interface is reachable from the network and has no \
                 authentication -- anyone who can reach it can start tasks and \
                 type directly into running agents' terminals, as you"
            );
            for url in reachable_urls(addr.port()) {
                tracing::info!(url = %url, "http interface listening");
            }
        }

        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                while shutdown.changed().await.is_ok() {
                    if *shutdown.borrow() {
                        return;
                    }
                }
            })
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("http server: {e}")))?;

        Ok(())
    }
}

/// The addresses a person on the same network would type. Asking a UDP socket
/// where it would send from is how to learn the primary address without
/// enumerating interfaces or taking a dependency; nothing is sent.
fn reachable_urls(port: u16) -> Vec<String> {
    let mut urls = vec![format!("http://localhost:{port}")];
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(local) = sock.local_addr() {
                if !local.ip().is_loopback() && !local.ip().is_unspecified() {
                    urls.push(format!("http://{}:{port}", local.ip()));
                }
            }
        }
    }
    urls
}

fn router(engine: Arc<Engine>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ui/{*path}", get(asset))
        .route("/ws", get(ws_upgrade))
        .route("/ws/term", get(term_upgrade))
        .route("/api/status", get(status))
        .route("/api/adapters", get(adapters))
        .route("/api/agent-runtime", get(runtime_connections))
        .route("/api/agents", get(agents))
        .route("/api/occupancy", get(occupancy))
        .route("/api/production", get(production))
        .route("/api/site", get(site_footprint))
        .route("/api/environment", get(environment))
        .route("/api/dependencies", get(dependencies))
        .route("/api/infrastructure", get(infrastructure))
        .route("/api/backup", get(backup))
        .route("/api/backup/run", post(backup_run))
        .route("/api/backup/verify", post(backup_verify))
        .route("/api/knowledge", get(knowledge))
        .route("/api/knowledge/search", get(knowledge_search))
        // Its own body limit, scoped to this one route with a nested router:
        // the browser's "Add documents" upload sends raw bytes, up to 50 MiB,
        // and every other route here still gets axum's ordinary default.
        .merge(
            Router::new()
                .route("/api/knowledge/files", put(knowledge_write_file))
                .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
                .with_state(engine.clone()),
        )
        // The L6 Policy tab: ADR 0004's catalogue of controls checked
        // against evidence Factory already has, re-read on every call like
        // knowledge and datasets above.
        .route("/api/policy", get(policy))
        .route("/api/policy/controls/{framework}/{id}", get(policy_control))
        .route("/api/policy/attestations", post(create_attestation))
        .route(
            "/api/policy/attestations/{id}/withdraw",
            post(withdraw_attestation),
        )
        .route("/api/policy/remediate", post(policy_remediate))
        .route("/api/policy/export", get(policy_export))
        .route("/api/metrics", get(metrics))
        .route("/api/costs", get(costs))
        .route("/api/goals", get(goals))
        .route("/api/goals/checkins", post(create_goals_checkin))
        .route("/api/scenarios", get(scenarios))
        .route("/api/scenarios/promote", post(scenario_promote))
        .route("/api/scenarios/whatif", post(scenario_whatif))
        // The L6 Quality attributes tab (`#107`).
        .route("/api/quality", get(quality))
        .route("/api/quality/remediate", post(quality_remediate))
        .route("/api/operations", get(operations))
        .route("/api/intake", get(intake_board).post(intake_add))
        .route("/api/intake/{id}/triage", post(intake_triage))
        .route("/api/intake/{id}/assess", post(intake_assess))
        .route("/api/intake/{id}/decide", post(intake_decide))
        .route("/api/intake/{id}/info", post(intake_info))
        .route("/api/benchmarks", get(benchmarks))
        .route("/api/datasets", get(list_datasets).post(create_dataset))
        .route("/api/datasets/{name}", get(get_dataset).delete(delete_dataset))
        .route("/api/datasets/{name}/cases", post(add_dataset_cases))
        .route("/api/datasets/{name}/import", post(import_dataset))
        .route("/api/datasets/{name}/from-tasks", post(dataset_from_tasks))
        .route("/api/datasets/{name}/cases/{id}", delete(delete_dataset_case))
        .route("/api/bench/runs", get(list_bench_runs).post(start_bench_run))
        .route("/api/bench/runs/{id}", get(get_bench_run))
        .route("/api/bench/runs/{id}/cancel", post(cancel_bench_run))
        .route("/api/bench/runs/{id}/clean", post(clean_bench_run))
        // A role is written into one scope's config, so like a declaration it
        // is addressed by scope and name in the body rather than the path.
        .route(
            "/api/roles",
            get(role_list).post(role_define).delete(role_delete),
        )
        // The id of a standing agent is `<scope>/<name>`, which has a slash in
        // it, so these take it in the body rather than the path.
        .route("/api/agents/start", post(agent_start))
        .route(
            "/api/agents/configure",
            post(agent_configure).delete(agent_delete),
        )
        .route("/api/agents/stop", post(agent_stop))
        .route("/api/agents/role", post(agent_role))
        .route("/api/agents/input", post(agent_input))
        .route("/api/agents/output", post(agent_output))
        .route("/api/rpc", post(rpc))
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/{id}", get(get_task))
        .route("/api/tasks/{id}", patch(update_task))
        .route("/api/tasks/{id}", delete(delete_task))
        .route("/api/tasks/{id}/run", post(run_task))
        .route("/api/tasks/{id}/cancel", post(cancel_task))
        .route("/api/tasks/{id}/close", post(close_task))
        .route("/api/tasks/{id}/reopen", post(reopen_task))
        .route("/api/tasks/{id}/skip-next", post(skip_next_task))
        .route("/api/tasks/{id}/report", post(report_task))
        .route("/api/tasks/{id}/entries", get(task_entries))
        .route("/api/tasks/{id}/output", get(task_output))
        .route("/api/tasks/{id}/runs", get(task_runs))
        .route("/api/tasks/{id}/usage", get(task_usage))
        .route("/api/workflows", get(list_workflows).post(create_workflow))
        .route("/api/workflow-lint", get(workflow_lint))
        .route(
            "/api/workflows/{id}",
            get(get_workflow)
                .patch(update_workflow)
                .delete(delete_workflow),
        )
        .route("/api/workflows/{id}/run", post(start_workflow))
        .route("/api/workflows/{id}/runs", get(workflow_runs))
        .route("/api/workflow-runs/{id}", get(get_workflow_run))
        .route("/api/workflow-runs/{id}/cancel", post(cancel_workflow_run))
        .route("/api/runs/{id}", get(get_run))
        .route("/api/runs/{id}/entries", get(run_entries))
        .route("/api/runs/{id}/attestations", get(run_attestations))
        .route("/api/runs/{id}/usage", get(run_usage))
        .route("/api/runs/{id}/output", get(run_output))
        .route("/api/runs/{id}/input", post(run_input))
        .route("/api/runs/{id}/answer", post(run_answer))
        .route("/api/runs/{id}/approve", post(run_approve))
        .route("/api/runs/{id}/reject", post(run_reject))
        .route("/api/runs/{id}/rework", post(run_rework))
        .with_state(engine)
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        crate::ui::INDEX_HTML,
    )
}

/// The page's own files: stylesheet and ES modules, named in `ui::ASSETS`.
/// A path that is not in that list is a 404, never a guess at a file on disk --
/// the daemon serves what it was compiled with and nothing else.
async fn asset(Path(path): Path<String>) -> AxumResponse {
    match crate::ui::asset(&path) {
        Some((mime, body)) => ([(header::CONTENT_TYPE, mime)], body).into_response(),
        None => (StatusCode::NOT_FOUND, "no such file").into_response(),
    }
}

/// Every route funnels through here, so REST and the socket cannot drift apart.
async fn run(engine: &Arc<Engine>, request: Request) -> AxumResponse {
    run_as(engine, request, None).await
}

/// The same, for a caller that presented a token.
async fn run_as(engine: &Arc<Engine>, request: Request, token: Option<String>) -> AxumResponse {
    let response = engine.handle(Envelope { request, token }).await;
    let code = status_for(&response);
    (code, Json(response)).into_response()
}

fn status_for(response: &Response) -> StatusCode {
    match response {
        Response::Ok { .. } => StatusCode::OK,
        Response::Error { code, .. } => match code.as_str() {
            "task_not_found" | "no_such_adapter" | "no_such_scope" => StatusCode::NOT_FOUND,
            "bad_request" => StatusCode::BAD_REQUEST,
            "denied" => StatusCode::FORBIDDEN,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        },
    }
}

/// The raw envelope, token and all. This is the one route an agent uses when
/// it is not going through the CLI.
async fn rpc(State(engine): State<Arc<Engine>>, Json(env): Json<Envelope>) -> AxumResponse {
    let response = engine.handle(env).await;
    let code = status_for(&response);
    (code, Json(response)).into_response()
}

async fn status(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Status).await
}

async fn adapters(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Adapters).await
}

async fn runtime_connections(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::RuntimeConnections).await
}

async fn agents(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Agents).await
}

#[derive(serde::Deserialize)]
struct Window {
    minutes: Option<u32>,
    /// RFC 3339. A `+01:00` offset has to be written `%2B01:00` in a query
    /// string, where a bare `+` is a space; the UI sends `Z`.
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
}

async fn occupancy(
    State(engine): State<Arc<Engine>>,
    Query(window): Query<Window>,
) -> AxumResponse {
    run(
        &engine,
        Request::Occupancy {
            minutes: window.minutes,
            from: window.from,
            to: window.to,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct ProductionQuery {
    minutes: Option<u32>,
    bin: Option<ProductionBin>,
    scope: Option<String>,
}

async fn production(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<ProductionQuery>,
) -> AxumResponse {
    run(
        &engine,
        Request::Production {
            minutes: q.minutes,
            bin: q.bin,
            scope: q.scope,
        },
    )
    .await
}

async fn site_footprint(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::SiteFootprint).await
}

async fn environment(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Environment).await
}

#[derive(serde::Deserialize)]
struct DependenciesQuery { scope: String }

async fn dependencies(
    State(engine): State<Arc<Engine>>,
    Query(query): Query<DependenciesQuery>,
) -> AxumResponse {
    run(&engine, Request::Dependencies { scope: query.scope }).await
}

async fn infrastructure(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Infrastructure).await
}

async fn backup(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Backup).await
}

/// `POST /api/backup/run` -- takes no body, so it has no `Json` extractor to
/// refuse a bare POST with.
async fn backup_run(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::BackupRun).await
}

#[derive(serde::Deserialize)]
struct VerifyQuery {
    #[serde(default)]
    snapshot: Option<String>,
}

/// `POST /api/backup/verify?snapshot=` -- the newest when `snapshot` is left
/// out. A query parameter, like `withdraw_attestation`'s `reason`, so a bare
/// POST works.
async fn backup_verify(State(engine): State<Arc<Engine>>, Query(q): Query<VerifyQuery>) -> AxumResponse {
    run(&engine, Request::BackupVerify { snapshot: q.snapshot }).await
}

async fn knowledge(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Knowledge).await
}

#[derive(serde::Deserialize)]
struct KnowledgeSearchQuery {
    #[serde(default)]
    q: String,
    /// Comma-separated, since a query string's repeated keys do not
    /// deserialize into a list here.
    #[serde(default)]
    tags: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// `GET /api/knowledge/search?q=&tags=a,b&scope=&limit=` -- page ids and why
/// they matched, never page text.
async fn knowledge_search(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<KnowledgeSearchQuery>,
) -> AxumResponse {
    let tags = q
        .tags
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect();
    run(
        &engine,
        Request::KnowledgeSearch {
            text: q.q,
            tags,
            scope: q.scope.filter(|s| !s.trim().is_empty()),
            limit: q.limit,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct KnowledgeFileQuery {
    path: String,
    #[serde(default)]
    overwrite: bool,
}

/// `PUT /api/knowledge/files?path=&overwrite=`, the UI's "Add documents"
/// upload -- the raw body is the file's bytes, since a browser cannot name a
/// path on the daemon's own disk the way a CLI invocation can. This route
/// carries its own 50 MiB `DefaultBodyLimit`, applied in `router` rather than
/// here.
async fn knowledge_write_file(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<KnowledgeFileQuery>,
    body: axum::body::Bytes,
) -> AxumResponse {
    run(
        &engine,
        Request::KnowledgeWriteFile {
            path: q.path,
            overwrite: q.overwrite,
            bytes: body.to_vec(),
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct PolicyQuery {
    #[serde(default)]
    scope: Option<String>,
}

/// `GET /api/policy?scope=` -- the L6 status board: `scope` empty or absent
/// means the whole instance, like `Request::Policy` itself.
async fn policy(State(engine): State<Arc<Engine>>, Query(q): Query<PolicyQuery>) -> AxumResponse {
    run(
        &engine,
        Request::Policy {
            scope: q.scope.filter(|s| !s.trim().is_empty()),
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct PolicyControlQuery {
    scope: String,
}

/// `GET /api/policy/controls/{framework}/{id}?scope=` -- one control's full
/// detail at `scope`, which is required (unlike `GET /api/policy`): a
/// control's status and applicability are only meaningful at one scope, not
/// summed over a subtree. A missing `scope` is refused by the `Query`
/// extractor itself, the same way `path` is required on the knowledge file
/// upload above.
async fn policy_control(
    State(engine): State<Arc<Engine>>,
    Path((framework, id)): Path<(String, String)>,
    Query(q): Query<PolicyControlQuery>,
) -> AxumResponse {
    run(
        &engine,
        Request::PolicyControl {
            control: factory_core::policy::ControlRef::new(framework, id),
            scope: q.scope,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct AttestBody {
    /// `framework/id`.
    control: String,
    scope: String,
    /// A pointer to the evidence -- a document, a ticket, a page -- not the
    /// evidence itself.
    evidence: String,
    #[serde(default)]
    note: Option<String>,
    /// `30d`/`12w`, a bare date (`2027-01-01`), or a full RFC3339 timestamp
    /// -- the same three forms `factory policy attest --expires` accepts,
    /// turned into the wire's absolute `expires_at` by the same
    /// `policy::parse_expiry` the CLI uses. Chosen over requiring an
    /// already-absolute `expires_at` in the body so a `curl` caller does not
    /// have to compute one by hand; an RFC3339 timestamp is still accepted
    /// here exactly as it is on the CLI's `--expires`.
    expires: String,
}

/// `POST /api/policy/attestations` -- record an attestation. `control` and
/// `expires` are parsed before the request ever reaches the engine, the same
/// way `agent_input` below refuses a body with no `id` up front; a bad
/// `framework/id` or an unparseable `expires` becomes a 400 without a round
/// trip through `authorize`.
async fn create_attestation(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<AttestBody>,
) -> AxumResponse {
    let control: factory_core::policy::ControlRef = match body.control.parse() {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response()
        }
    };
    let expires_at = match factory_core::policy::parse_expiry(&body.expires, chrono::Utc::now()) {
        Ok(t) => t,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response()
        }
    };
    run(
        &engine,
        Request::PolicyAttest {
            control,
            scope: body.scope,
            evidence: body.evidence,
            note: body.note,
            expires_at,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct WithdrawQuery {
    #[serde(default)]
    reason: Option<String>,
}

/// `POST /api/policy/attestations/{id}/withdraw?reason=` -- `reason` is a
/// query parameter rather than a JSON body, since it is the only thing this
/// call carries and a bare `POST` with no body is otherwise refused before
/// it reaches here.
async fn withdraw_attestation(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(q): Query<WithdrawQuery>,
) -> AxumResponse {
    run(&engine, Request::PolicyWithdraw { id, reason: q.reason }).await
}

#[derive(serde::Deserialize)]
struct RemediateBody {
    /// `framework/id`.
    control: String,
    scope: String,
    #[serde(default)]
    agent: Option<String>,
}

/// `POST /api/policy/remediate` -- close a gap. Mirrors `create_attestation`'s
/// own shape: `control` is parsed before the request ever reaches the
/// engine, the same up-front 400 rather than a round trip through
/// `authorize` for a string that was never going to parse. Answers the same
/// way `POST /api/tasks` does, `{"kind":"task","task":{...}}` -- this is an
/// ordinary task in every way but how it was asked for.
async fn policy_remediate(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<RemediateBody>,
) -> AxumResponse {
    let control: factory_core::policy::ControlRef = match body.control.parse() {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response()
        }
    };
    run(
        &engine,
        Request::PolicyRemediate {
            control,
            scope: body.scope,
            agent: body.agent,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct PolicyExportQuery {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    format: Option<String>,
}

/// `GET /api/policy/export?scope=&format=` -- a snapshot for an auditor,
/// downloaded rather than wrapped in the ordinary `{"kind":...}` envelope
/// every other route here answers with: the body *is* the export (markdown
/// or JSON), with a `Content-Disposition` naming a file a browser click
/// saves directly, so this is the one route in this file that calls
/// `engine.handle` itself rather than going through `run` -- authorization
/// still runs exactly the same way, only the response shape is bespoke.
/// `format` defaults to `md`; anything `Request::PolicyExport` does not
/// recognize comes back as the ordinary JSON error every other refusal
/// here does.
async fn policy_export(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<PolicyExportQuery>,
) -> AxumResponse {
    let format = q.format.unwrap_or_else(|| "md".to_string());
    let response = engine
        .handle(Envelope {
            request: Request::PolicyExport {
                scope: q.scope,
                format,
            },
            token: None,
        })
        .await;
    match response {
        Response::Ok {
            data: Payload::PolicyExport { format, filename, body },
        } => {
            let content_type = match format.as_str() {
                "json" => "application/json; charset=utf-8",
                _ => "text/markdown; charset=utf-8",
            };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type.to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{filename}\""),
                    ),
                ],
                body,
            )
                .into_response()
        }
        other => (status_for(&other), Json(other)).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct CostsQuery {
    #[serde(default)]
    group_by: Option<String>,
    #[serde(default)]
    from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    scope: Option<String>,
}

/// `GET /api/costs?group_by=task|issue|scope|agent&from=&to=&scope=` --
/// usage and cost summed per group over the runs that started in the
/// window (#117). `from`/`to` are RFC 3339; the window defaults to the last
/// thirty days. A grouping v1 does not offer yet (`provider`, `workflow`)
/// is a 400 that says so, not an empty answer.
async fn costs(State(engine): State<Arc<Engine>>, Query(q): Query<CostsQuery>) -> AxumResponse {
    let group_by = match q.group_by.as_deref().filter(|g| !g.trim().is_empty()) {
        None => factory_core::usage::CostGroupBy::default(),
        Some(raw) => match raw.parse() {
            Ok(g) => g,
            Err(e) => return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response(),
        },
    };
    run(
        &engine,
        Request::Costs {
            group_by,
            from: q.from,
            to: q.to,
            scope: q.scope.filter(|s| !s.trim().is_empty()),
        },
    )
    .await
}

async fn task_usage(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::TaskUsage { id }).await
}

async fn run_usage(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::RunUsage { id }).await
}

#[derive(serde::Deserialize)]
struct MetricsQuery {
    /// Comma-separated, the same convention `KnowledgeSearchQuery::tags`
    /// already uses for a query string's repeated-key limitation. Empty (or
    /// absent) means `Request::Metrics`'s own default: every
    /// non-parameterised metric plus whatever the loaded goals and policy
    /// catalogues imply.
    #[serde(default)]
    ids: String,
}

/// `GET /api/metrics?ids=a,b` -- every named metric's computed value, its
/// history where it has one, and the definition behind it.
async fn metrics(State(engine): State<Arc<Engine>>, Query(q): Query<MetricsQuery>) -> AxumResponse {
    let mut ids = Vec::new();
    for raw in q.ids.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match raw.parse() {
            Ok(id) => ids.push(id),
            Err(e) => return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response(),
        }
    }
    run(&engine, Request::Metrics { ids }).await
}

#[derive(serde::Deserialize)]
struct GoalsQuery {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    cycle: Option<String>,
}

/// `GET /api/goals?scope=&cycle=` -- the L6 Goals tab's whole answer.
async fn goals(State(engine): State<Arc<Engine>>, Query(q): Query<GoalsQuery>) -> AxumResponse {
    run(
        &engine,
        Request::Goals {
            scope: q.scope.filter(|s| !s.trim().is_empty()),
            cycle: q.cycle.filter(|s| !s.trim().is_empty()),
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct GoalsCheckInBody {
    /// `objective/kr`.
    kr: String,
    value: f64,
    confidence: u8,
    #[serde(default)]
    note: Option<String>,
}

/// `POST /api/goals/checkins` -- record a check-in against a manual key
/// result. `kr` is parsed before the request ever reaches the engine, the
/// same up-front 400 `create_attestation`'s own `control` field gets.
async fn create_goals_checkin(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<GoalsCheckInBody>,
) -> AxumResponse {
    let kr: factory_core::goals::KrRef = match body.kr.parse() {
        Ok(kr) => kr,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response(),
    };
    run(
        &engine,
        Request::GoalsCheckIn {
            kr,
            value: body.value,
            confidence: body.confidence,
            note: body.note,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct ScenariosQuery {
    #[serde(default)]
    scope: Option<String>,
}

/// `GET /api/scenarios?scope=` -- the L6 Scenarios tab's whole answer
/// (`#100`), the same "empty or absent `scope` is the whole instance" rule
/// `GET /api/policy`/`GET /api/goals` already follow.
async fn scenarios(State(engine): State<Arc<Engine>>, Query(q): Query<ScenariosQuery>) -> AxumResponse {
    run(
        &engine,
        Request::Scenarios {
            scope: q.scope.filter(|s| !s.trim().is_empty()),
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct ScenarioPromoteBody {
    scenario: String,
    scope: String,
    #[serde(default)]
    agent: Option<String>,
}

#[derive(serde::Deserialize)]
struct OperationsQuery {
    scope: Option<String>,
    /// `7d` or `30d`; anything else is refused before the engine sees it.
    #[serde(default)]
    window: factory_core::operations::HealthWindow,
    /// `charts` adds the per-step and per-run detail the tab draws; any
    /// other value is refused like a bad window.
    #[serde(default)]
    detail: Option<OperationsDetail>,
}

#[derive(serde::Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum OperationsDetail {
    Charts,
}

/// `GET /api/operations?scope=&window=7d|30d[&detail=charts]` -- the L4
/// Operations tab's whole answer, the same report `factory stats` prints
/// (`#106`). The Inbox asks without `detail`.
async fn operations(State(engine): State<Arc<Engine>>, Query(q): Query<OperationsQuery>) -> AxumResponse {
    run(
        &engine,
        Request::Operations {
            scope: q.scope,
            window: q.window,
            detail: matches!(q.detail, Some(OperationsDetail::Charts)),
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct IntakeQuery {
    #[serde(default)]
    scope: Option<String>,
}

/// `GET /api/intake?scope=` -- the L4 Intake view's board (`#119`).
async fn intake_board(State(engine): State<Arc<Engine>>, Query(q): Query<IntakeQuery>) -> AxumResponse {
    run(&engine, Request::IntakeBoard { scope: q.scope }).await
}

/// `POST /api/intake` -- hand an item in. The web UI is the caller that
/// arrives here, so an item that names no source is recorded as `ui`.
async fn intake_add(
    State(engine): State<Arc<Engine>>,
    Json(mut new): Json<factory_core::intake::NewIntake>,
) -> AxumResponse {
    new.source.get_or_insert(factory_core::intake::SourceKind::Ui);
    run(&engine, Request::IntakeAdd(new)).await
}

#[derive(serde::Deserialize, Default)]
struct IntakeTriageBody {
    #[serde(default)]
    agent: Option<String>,
}

/// `POST /api/intake/{id}/triage` -- start the triage node, `{agent?}`.
async fn intake_triage(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> AxumResponse {
    let parsed = if body.iter().all(u8::is_ascii_whitespace) {
        Ok(IntakeTriageBody::default())
    } else {
        serde_json::from_slice::<IntakeTriageBody>(&body).map_err(|e| format!("not a triage body: {e}"))
    };
    match parsed {
        Ok(b) => run(&engine, Request::IntakeTriage { id, agent: b.agent }).await,
        Err(why) => refused(why),
    }
}

#[derive(serde::Deserialize)]
struct IntakeAssessBody {
    assessment: factory_core::intake::Assessment,
    #[serde(default)]
    decide: bool,
}

/// `POST /api/intake/{id}/assess` -- `{assessment, decide?}`.
async fn intake_assess(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<IntakeAssessBody>,
) -> AxumResponse {
    run(&engine, Request::IntakeAssess { id, assessment: body.assessment, decide: body.decide }).await
}

/// `POST /api/intake/{id}/decide` -- a `Decision`, tagged by `decision`.
async fn intake_decide(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(decision): Json<factory_core::intake::Decision>,
) -> AxumResponse {
    run(&engine, Request::IntakeDecide { id, decision }).await
}

#[derive(serde::Deserialize)]
struct IntakeInfoBody {
    text: String,
}

/// `POST /api/intake/{id}/info` -- `{text}`, the answer to a needs-info.
async fn intake_info(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<IntakeInfoBody>,
) -> AxumResponse {
    run(&engine, Request::IntakeInfo { id, text: body.text }).await
}

/// `POST /api/scenarios/promote` -- turn a scenario into real work. Answers
/// `{"kind":"scenario_promote","result":{...}}`; `result.created` is the
/// same task shape `POST /api/tasks`/`POST /api/policy/remediate` answer
/// with, one per newly-open control, and `result.skipped` names every
/// control a non-terminal task already covers.
async fn scenario_promote(State(engine): State<Arc<Engine>>, Json(body): Json<ScenarioPromoteBody>) -> AxumResponse {
    run(
        &engine,
        Request::ScenarioPromote {
            scenario: body.scenario,
            scope: body.scope,
            agent: body.agent,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct ScenarioWhatIfBody {
    #[serde(default)]
    scenario: Option<String>,
    /// Driver id to its raw, as-authored override (`×2`, `+20%`, `+5`,
    /// `=0.9`) -- the same syntax a scenario file's own `drivers:` map uses,
    /// parsed the same way (`scenario::parse_override`). A JSON object, not
    /// a query string, since a slider panel already holds this as a map in
    /// memory and the driver count is open-ended.
    #[serde(default)]
    drivers: std::collections::BTreeMap<String, String>,
}

/// `POST /api/scenarios/whatif` -- recompute driver outcomes, tornado and
/// forecast with slider overrides applied server-side. Pure and read-only;
/// the UI's driver panel is expected to call this on every slider change,
/// debounced client-side.
async fn scenario_whatif(State(engine): State<Arc<Engine>>, Json(body): Json<ScenarioWhatIfBody>) -> AxumResponse {
    run(
        &engine,
        Request::ScenarioWhatIf {
            scenario: body.scenario,
            drivers: body.drivers,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct QualityQuery {
    #[serde(default)]
    scope: Option<String>,
}

/// `GET /api/quality?scope=` -- the L6 Quality attributes tab's whole
/// answer. Read-only; an empty `scope` is the whole instance, the same as
/// leaving it out, like `/api/goals`.
async fn quality(State(engine): State<Arc<Engine>>, Query(q): Query<QualityQuery>) -> AxumResponse {
    run(
        &engine,
        Request::Quality {
            scope: q.scope.filter(|s| !s.trim().is_empty()),
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct QualityRemediateBody {
    scope: String,
    attribute: String,
    scenario: String,
    #[serde(default)]
    agent: Option<String>,
}

/// `POST /api/quality/remediate` -- the task that closes one scenario's gap.
/// Answers `{"kind":"quality_remediate","result":{task,created}}`;
/// `created: false` is the already-open task, returned rather than a
/// second one made (`#98`).
async fn quality_remediate(State(engine): State<Arc<Engine>>, Json(body): Json<QualityRemediateBody>) -> AxumResponse {
    run(
        &engine,
        Request::QualityRemediate {
            scope: body.scope,
            attribute: body.attribute,
            scenario: body.scenario,
            agent: body.agent,
        },
    )
    .await
}

async fn benchmarks(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Benchmarks).await
}

async fn list_datasets(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Datasets).await
}

async fn get_dataset(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> AxumResponse {
    run(&engine, Request::Dataset { name }).await
}

#[derive(serde::Deserialize)]
struct CreateDataset {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

async fn create_dataset(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<CreateDataset>,
) -> AxumResponse {
    run(
        &engine,
        Request::DatasetCreate {
            name: body.name,
            description: body.description,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct AddDatasetCases {
    cases: Vec<factory_core::dataset::Case>,
}

async fn add_dataset_cases(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
    Json(body): Json<AddDatasetCases>,
) -> AxumResponse {
    run(&engine, Request::DatasetAddCases { name, cases: body.cases }).await
}

#[derive(serde::Deserialize)]
struct ImportDataset {
    format: String,
    content: String,
    #[serde(default)]
    replace: bool,
}

async fn import_dataset(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
    Json(body): Json<ImportDataset>,
) -> AxumResponse {
    run(
        &engine,
        Request::DatasetImport {
            name,
            format: body.format,
            content: body.content,
            replace: body.replace,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct FromTasks {
    task_ids: Vec<String>,
}

async fn dataset_from_tasks(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
    Json(body): Json<FromTasks>,
) -> AxumResponse {
    run(&engine, Request::DatasetFromTasks { name, task_ids: body.task_ids }).await
}

async fn delete_dataset_case(
    State(engine): State<Arc<Engine>>,
    Path((name, id)): Path<(String, String)>,
) -> AxumResponse {
    run(&engine, Request::DatasetDeleteCase { name, id }).await
}

async fn delete_dataset(State(engine): State<Arc<Engine>>, Path(name): Path<String>) -> AxumResponse {
    run(&engine, Request::DatasetDelete { name }).await
}

#[derive(serde::Deserialize)]
struct StartBenchRun {
    dataset: String,
    agents: Vec<String>,
    #[serde(default)]
    attempts: Option<u32>,
    #[serde(default)]
    concurrency: Option<u32>,
    #[serde(default)]
    cases: Option<Vec<String>>,
}

async fn start_bench_run(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<StartBenchRun>,
) -> AxumResponse {
    run(
        &engine,
        Request::BenchRunStart {
            dataset: body.dataset,
            agents: body.agents,
            attempts: body.attempts,
            concurrency: body.concurrency,
            cases: body.cases,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct BenchRunsQuery {
    #[serde(default)]
    dataset: Option<String>,
}

async fn list_bench_runs(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<BenchRunsQuery>,
) -> AxumResponse {
    run(&engine, Request::BenchRuns { dataset: q.dataset }).await
}

async fn get_bench_run(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::BenchRunGet { id }).await
}

async fn cancel_bench_run(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::BenchRunCancel { id }).await
}

async fn clean_bench_run(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::BenchRunClean { id }).await
}

async fn list_tasks(
    State(engine): State<Arc<Engine>>,
    Query(filter): Query<TaskFilter>,
) -> AxumResponse {
    run(&engine, Request::TaskList(filter)).await
}

async fn create_task(State(engine): State<Arc<Engine>>, Json(new): Json<NewTask>) -> AxumResponse {
    run(&engine, Request::TaskCreate(new)).await
}

async fn get_task(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::TaskGet { id }).await
}

/// The patch, with an optional `reason` beside its fields: why, for the
/// journal, when the edit is one that journals -- pausing or resuming a
/// schedule (`#106`). Kept out of `TaskPatch` itself, which a store applies.
#[derive(serde::Deserialize)]
struct PatchBody {
    #[serde(flatten)]
    patch: TaskPatch,
    #[serde(default)]
    reason: Option<String>,
}

async fn update_task(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> AxumResponse {
    run(
        &engine,
        Request::TaskUpdate {
            id,
            patch: body.patch,
            reason: body.reason,
        },
    )
    .await
}

async fn delete_task(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::TaskDelete { id }).await
}

/// An action's optional body: `{"reason": "..."}`, or nothing at all.
#[derive(serde::Deserialize, Default)]
struct ReasonBody {
    #[serde(default)]
    reason: Option<String>,
}

/// The body of an action that may say why, and may say nothing -- read by
/// hand rather than through `Option<Json<_>>`, which refuses the empty body
/// the web UI sends with its JSON content type on every POST.
fn reason_of(body: &[u8]) -> std::result::Result<Option<String>, String> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice::<ReasonBody>(body)
        .map(|b| b.reason)
        .map_err(|e| format!("not a reason body: {e}"))
}

/// A body refused before the engine saw it, in the same envelope every
/// other refusal comes back in -- `core.js`'s `api()` reads `message`.
fn refused(why: String) -> AxumResponse {
    let response = Response::Error { code: "bad_request".into(), message: why };
    (StatusCode::BAD_REQUEST, Json(response)).into_response()
}

async fn run_task(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> AxumResponse {
    match reason_of(&body) {
        Ok(reason) => run(&engine, Request::TaskRun { id, reason }).await,
        Err(why) => refused(why),
    }
}

async fn cancel_task(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> AxumResponse {
    // Body `{reason?, run_id?}`, or none at all: `run_id` names the attempt
    // the caller means, so one that has since been replaced is not ended.
    let parsed = if body.iter().all(u8::is_ascii_whitespace) {
        Ok(CancelBody::default())
    } else {
        serde_json::from_slice::<CancelBody>(&body).map_err(|e| format!("not a cancel body: {e}"))
    };
    match parsed {
        Ok(b) => run(&engine, Request::TaskCancel { id, reason: b.reason, run: b.run_id }).await,
        Err(why) => refused(why),
    }
}

#[derive(serde::Deserialize, Default)]
struct CancelBody {
    #[serde(default)]
    reason: Option<String>,
    /// The run the caller means to cancel; see `Request::TaskCancel`.
    #[serde(default)]
    run_id: Option<String>,
}

/// `POST /api/tasks/{id}/close` -- close a task on purpose (`#122`). Body
/// `{reason, duplicate_of?, note?}`; `reason` is `completed`,
/// `not_planned` or `duplicate`.
async fn close_task(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> AxumResponse {
    match serde_json::from_slice::<CloseBody>(&body) {
        Ok(b) => run(&engine, Request::TaskClose { id, reason: b.reason, duplicate_of: b.duplicate_of, note: b.note }).await,
        Err(e) => refused(format!("not a close body: {e}")),
    }
}

#[derive(serde::Deserialize)]
struct CloseBody {
    reason: factory_core::task::CloseReason,
    #[serde(default)]
    duplicate_of: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

/// `POST /api/tasks/{id}/reopen` -- a closed task back to pending. Body
/// `{reason?}`, or none at all.
async fn reopen_task(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> AxumResponse {
    match reason_of(&body) {
        Ok(reason) => run(&engine, Request::TaskReopen { id, reason }).await,
        Err(why) => refused(why),
    }
}

#[derive(serde::Deserialize, Default)]
struct SkipBody {
    #[serde(default)]
    reason: Option<String>,
    /// The `next_run_at` the caller means to skip; see `Request::TaskSkipNext`.
    #[serde(default)]
    slot: Option<chrono::DateTime<chrono::Utc>>,
}

/// `POST /api/tasks/{id}/skip-next` -- pass over the schedule's next slot.
/// Body `{reason?, slot?}`, or none at all.
async fn skip_next_task(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> AxumResponse {
    let parsed = if body.iter().all(u8::is_ascii_whitespace) {
        Ok(SkipBody::default())
    } else {
        serde_json::from_slice::<SkipBody>(&body).map_err(|e| format!("not a skip body: {e}"))
    };
    match parsed {
        Ok(b) => run(&engine, Request::TaskSkipNext { id, reason: b.reason, slot: b.slot }).await,
        Err(why) => refused(why),
    }
}

async fn report_task(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(report): Json<TaskReport>,
) -> AxumResponse {
    run(&engine, Request::TaskReport { id, report }).await
}

#[derive(serde::Deserialize)]
struct Lines {
    lines: Option<u32>,
    limit: Option<u32>,
    /// `GET /api/tasks/{id}/entries?task_only=true`: only the task's own
    /// entries, the ones that belong to no run.
    #[serde(default)]
    task_only: bool,
}

async fn task_entries(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(q): Query<Lines>,
) -> AxumResponse {
    run(&engine, Request::TaskEntries { id, limit: q.limit, task_only: q.task_only }).await
}

async fn task_output(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(q): Query<Lines>,
) -> AxumResponse {
    run(&engine, Request::TaskOutput { id, lines: q.lines }).await
}

async fn task_runs(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(q): Query<Lines>,
) -> AxumResponse {
    run(
        &engine,
        Request::RunList {
            task_id: id,
            limit: q.limit,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct WorkflowQuery {
    scope: Option<String>,
    limit: Option<u32>,
}

async fn list_workflows(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<WorkflowQuery>,
) -> AxumResponse {
    run(&engine, Request::WorkflowList { scope: q.scope }).await
}

async fn create_workflow(
    State(engine): State<Arc<Engine>>,
    Json(workflow): Json<WorkflowDraft>,
) -> AxumResponse {
    run(&engine, Request::WorkflowCreate(workflow)).await
}

async fn get_workflow(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::WorkflowGet { id }).await
}

async fn update_workflow(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(workflow): Json<WorkflowDraft>,
) -> AxumResponse {
    run(&engine, Request::WorkflowUpdate { id, workflow }).await
}

async fn delete_workflow(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> AxumResponse {
    run(&engine, Request::WorkflowDelete { id }).await
}

/// `POST /api/workflows/{id}/run`, with an optional `{"inputs": {...}}`
/// body (`#140`). An empty body starts a workflow that declares no inputs.
async fn start_workflow(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> AxumResponse {
    #[derive(serde::Deserialize, Default)]
    struct Start {
        #[serde(default)]
        inputs: std::collections::BTreeMap<String, String>,
    }
    let start = if body.iter().all(u8::is_ascii_whitespace) {
        Start::default()
    } else {
        match serde_json::from_slice::<Start>(&body) {
            Ok(start) => start,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e.to_string()))).into_response()
            }
        }
    };
    run(&engine, Request::WorkflowStart { id, inputs: start.inputs }).await
}

async fn workflow_runs(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(q): Query<WorkflowQuery>,
) -> AxumResponse {
    run(
        &engine,
        Request::WorkflowRunList {
            workflow_id: Some(id),
            scope: None,
            limit: q.limit,
        },
    )
    .await
}

async fn get_workflow_run(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> AxumResponse {
    run(&engine, Request::WorkflowRunGet { id }).await
}

async fn cancel_workflow_run(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> AxumResponse {
    run(&engine, Request::WorkflowRunCancel { id }).await
}

#[derive(serde::Deserialize)]
struct LintQuery {
    workflow: Option<String>,
    task: Option<String>,
    scope: Option<String>,
    category: Option<String>,
}

/// `#118`: `?workflow=<id>`, `?task=<id>` or `?scope=<name>`, each with an
/// optional `&category=`.
async fn workflow_lint(State(engine): State<Arc<Engine>>, Query(q): Query<LintQuery>) -> AxumResponse {
    run(
        &engine,
        Request::WorkflowLint { workflow: q.workflow, task: q.task, scope: q.scope, category: q.category },
    )
    .await
}

async fn run_attestations(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::RunAttestations { id }).await
}

async fn get_run(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::RunGet { id }).await
}

async fn run_entries(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(q): Query<Lines>,
) -> AxumResponse {
    run(&engine, Request::RunEntries { id, limit: q.limit }).await
}

async fn run_output(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(q): Query<Lines>,
) -> AxumResponse {
    run(&engine, Request::RunOutput { id, lines: q.lines }).await
}

#[derive(serde::Deserialize)]
struct StartAgent {
    scope: String,
    name: String,
}

#[derive(serde::Deserialize)]
struct ConfigureAgent {
    scope: String,
    agent: ScopeAgent,
}

#[derive(serde::Deserialize)]
struct DeleteAgent {
    scope: String,
    name: String,
}

#[derive(serde::Deserialize)]
struct RolesQuery {
    scope: Option<String>,
}

#[derive(serde::Deserialize)]
struct DefineRole {
    scope: String,
    name: String,
    role: RoleSpec,
    #[serde(default)]
    replace: bool,
}

async fn role_list(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<RolesQuery>,
) -> AxumResponse {
    run(
        &engine,
        Request::RoleList {
            scope: q.scope.filter(|s| !s.is_empty()),
        },
    )
    .await
}

async fn role_define(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<DefineRole>,
) -> AxumResponse {
    run(
        &engine,
        Request::RoleDefine {
            scope: body.scope,
            name: body.name,
            role: body.role,
            replace: body.replace,
        },
    )
    .await
}

async fn role_delete(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<DeleteAgent>,
) -> AxumResponse {
    run(
        &engine,
        Request::RoleDelete {
            scope: body.scope,
            name: body.name,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct AgentRole {
    id: String,
    #[serde(default)]
    role: Option<String>,
}

#[derive(serde::Deserialize)]
struct AgentId {
    id: String,
    #[serde(default)]
    lines: Option<u32>,
}

#[derive(serde::Deserialize)]
struct Input {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    keys: Vec<String>,
}

async fn agent_start(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<StartAgent>,
) -> AxumResponse {
    run(
        &engine,
        Request::AgentStart {
            scope: body.scope,
            name: body.name,
        },
    )
    .await
}

async fn agent_configure(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<ConfigureAgent>,
) -> AxumResponse {
    run(
        &engine,
        Request::AgentConfigure {
            scope: body.scope,
            agent: body.agent,
        },
    )
    .await
}

async fn agent_delete(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<DeleteAgent>,
) -> AxumResponse {
    run(
        &engine,
        Request::AgentDelete {
            scope: body.scope,
            name: body.name,
        },
    )
    .await
}

async fn agent_stop(State(engine): State<Arc<Engine>>, Json(body): Json<AgentId>) -> AxumResponse {
    run(&engine, Request::AgentStop { id: body.id }).await
}

async fn agent_role(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<AgentRole>,
) -> AxumResponse {
    run(
        &engine,
        Request::AgentRole {
            id: body.id,
            // An empty pick from a `<select>` means "back to the config's",
            // which is the same thing as naming none.
            role: body.role.filter(|r| !r.is_empty()),
        },
    )
    .await
}

async fn agent_output(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<AgentId>,
) -> AxumResponse {
    run(
        &engine,
        Request::AgentOutput {
            id: body.id,
            lines: body.lines,
        },
    )
    .await
}

async fn agent_input(State(engine): State<Arc<Engine>>, Json(body): Json<Input>) -> AxumResponse {
    let Some(id) = body.id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(Response::error("bad_request", "which agent? pass `id`")),
        )
            .into_response();
    };
    run(
        &engine,
        Request::AgentInput {
            id,
            text: body.text,
            keys: body.keys,
        },
    )
    .await
}

async fn run_input(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<Input>,
) -> AxumResponse {
    run(
        &engine,
        Request::RunInput {
            id,
            text: body.text,
            keys: body.keys,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct AnswerBody {
    text: String,
    reason: String,
}

#[derive(serde::Deserialize)]
struct DecisionBody {
    reason: String,
}

async fn run_approve(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<DecisionBody>,
) -> AxumResponse {
    run(
        &engine,
        Request::RunApprove {
            id,
            reason: body.reason,
        },
    )
    .await
}

async fn run_reject(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<DecisionBody>,
) -> AxumResponse {
    run(
        &engine,
        Request::RunReject {
            id,
            reason: body.reason,
        },
    )
    .await
}

async fn run_rework(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::RunRework { id }).await
}

/// `POST /api/runs/{id}/answer` -- `{text, reason}`, both required.
async fn run_answer(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<AnswerBody>,
) -> AxumResponse {
    run(
        &engine,
        Request::RunAnswer {
            id,
            text: body.text,
            reason: body.reason,
        },
    )
    .await
}

async fn ws_upgrade(State(engine): State<Arc<Engine>>, ws: WebSocketUpgrade) -> AxumResponse {
    ws.on_upgrade(move |socket| ws_stream(engine, socket))
}

#[derive(serde::Deserialize)]
struct TermTarget {
    /// `agent` or `run`.
    kind: String,
    id: String,
}

async fn term_upgrade(
    State(engine): State<Arc<Engine>>,
    Query(target): Query<TermTarget>,
    ws: WebSocketUpgrade,
) -> AxumResponse {
    ws.on_upgrade(move |socket| term_stream(engine, socket, target))
}

/// A terminal, not a transcript.
///
/// The runtime renders whole frames, so this polls one and sends it only when
/// it differs from the last. Each frame stands alone: a viewer that joins late
/// or misses one is correct on the next tick, which is what lets a terminal be
/// mirrored over a socket without replaying a byte stream.
///
/// What comes back the other way is bytes, exactly as a terminal would send
/// them -- `ESC [ A` for an arrow, `\x03` for Ctrl-C. herdr passes them to the
/// pane untouched, so there is no table of key names in the middle to fall
/// behind what a keyboard can do.
async fn term_stream(engine: Arc<Engine>, socket: WebSocket, target: TermTarget) {
    /// Fast enough to feel live, slow enough that a person typing does not
    /// race their own echo. One poll per viewer, which is the prototype's
    /// trade: two people watching one agent is two polls.
    const FRAME_MS: u64 = 220;

    let (mut tx, mut rx) = socket.split();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(FRAME_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last: Option<String> = None;

    loop {
        tokio::select! {
            incoming = rx.next() => {
                match incoming {
                    Some(Ok(Message::Text(bytes))) => {
                        let request = match target.kind.as_str() {
                            "run" => Request::RunInput {
                                id: target.id.clone(),
                                text: Some(bytes.to_string()),
                                keys: Vec::new(),
                            },
                            _ => Request::AgentInput {
                                id: target.id.clone(),
                                text: Some(bytes.to_string()),
                                keys: Vec::new(),
                            },
                        };
                        // A keystroke that cannot be delivered is worth saying
                        // once, on the channel the viewer is already watching.
                        if let Response::Error { message, .. } = engine.handle_request(request).await {
                            let _ = tx.send(Message::Text(
                                format!("{{\"error\":{}}}", serde_json::json!(message)).into(),
                            )).await;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => return,
                }
            }
            _ = ticker.tick() => {
                let request = match target.kind.as_str() {
                    "run" => Request::RunScreen { id: target.id.clone() },
                    _ => Request::AgentScreen { id: target.id.clone() },
                };
                let text = match engine.handle_request(request).await {
                    Response::Ok { data: Payload::Screen { screen } } => {
                        let Ok(text) = serde_json::to_string(&screen) else { continue };
                        text
                    }
                    // No session to show is a state, not a failure: say it once
                    // and let the page decide what to put in its place.
                    Response::Error { message, .. } => {
                        let text = format!("{{\"error\":{}}}", serde_json::json!(message));
                        if last.as_deref() == Some(text.as_str()) { continue }
                        text
                    }
                    _ => continue,
                };
                if last.as_deref() == Some(text.as_str()) { continue }
                if tx.send(Message::Text(text.clone().into())).await.is_err() { return }
                last = Some(text);
            }
        }
    }
}

/// Live events, plus a snapshot first so a page that connects late is not
/// looking at an empty table until something happens.
async fn ws_stream(engine: Arc<Engine>, socket: WebSocket) {
    let (mut tx, mut rx) = socket.split();
    let mut events = engine.bus.subscribe();

    let snapshot = engine
        .handle_request(Request::TaskList(TaskFilter::default()))
        .await;
    if let Ok(text) = serde_json::to_string(&snapshot) {
        if tx.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            incoming = rx.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    // The UI does not speak on this channel; it posts instead.
                    _ => {}
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        let msg = Response::ok(Payload::Event { event });
                        let Ok(text) = serde_json::to_string(&msg) else { continue };
                        if tx.send(Message::Text(text.into())).await.is_err() {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Re-send the whole list rather than leave the page
                        // showing state that skipped a step.
                        let snapshot = engine.handle_request(Request::TaskList(TaskFilter::default())).await;
                        let Ok(text) = serde_json::to_string(&snapshot) else { continue };
                        if tx.send(Message::Text(text.into())).await.is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! The routes themselves, over a real socket: the router served on an
    //! ephemeral loopback port and spoken to in plain HTTP/1.1, so a route
    //! test needs no client crate beyond tokio. For the Operations routes
    //! the query string and the optional reason body are what this file
    //! adds, so they are what is checked here -- the report itself is
    //! `operations.rs`'s to test.

    use super::*;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, PolicyDeclaration, Scope};
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn engine_with_quality() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-http-test-{}", uuid::Uuid::new_v4()));
        let dir = root.join(".factory").join("quality");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("baseline.yaml"),
            "attributes:\n  - id: reliability\n    importance: H\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: sandboxed, measure: { check: sandbox } }\n",
        )
        .unwrap();
        let mut company: Scope = serde_yaml_ng::from_str("id: company-id\nname: company\n").unwrap();
        company.path = PathBuf::from(".");
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(company.clone()),
            scopes: vec![company],
            roles: Default::default(),
            policies: PolicyDeclaration::default(),
            quality: vec!["baseline".into()],
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(Factory { root, config }, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new()))
    }

    /// Serve `router(engine)` once on an ephemeral port and send it one
    /// request; the status code and the JSON body back.
    async fn request(engine: Arc<Engine>, method: &str, path: &str, body: Option<&str>) -> (u16, serde_json::Value) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router(engine)).await.unwrap() });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let text = String::from_utf8(raw).unwrap();
        let status: u16 = text.split(' ').nth(1).unwrap().parse().unwrap();
        let (_, payload) = text.split_once("\r\n\r\n").unwrap();
        (status, serde_json::from_str(payload).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn get_api_costs_groups_and_refuses_a_grouping_it_does_not_offer() {
        let engine = engine_with_quality();
        let (status, json) = request(
            engine.clone(),
            "GET",
            "/api/costs?group_by=scope&from=2026-01-01T00:00:00Z&to=2026-02-01T00:00:00Z",
            None,
        )
        .await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["kind"], "costs");
        assert_eq!(json["data"]["report"]["group_by"], "scope");
        assert_eq!(json["data"]["report"]["total"]["runs"], 0);
        assert_eq!(json["data"]["report"]["from"], "2026-01-01T00:00:00Z");

        let (status, json) = request(engine.clone(), "GET", "/api/costs?group_by=provider", None).await;
        assert_eq!(status, 400, "{json}");
        assert!(json["message"].as_str().unwrap().contains("provider"), "{json}");

        let (status, _) = request(engine, "GET", "/api/tasks/nope/usage", None).await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn get_api_quality_answers_the_report_and_an_unknown_scope_is_a_404() {
        let engine = engine_with_quality();
        let (status, json) = request(engine.clone(), "GET", "/api/quality", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["kind"], "quality");
        let report = &json["data"]["report"];
        assert_eq!(report["catalogue"].as_array().unwrap().len(), 9, "every column, declared or not");
        assert_eq!(report["scopes"][0]["scope"], "company");
        assert_eq!(report["scopes"][0]["attributes"][0]["scenarios"][0]["status"], "met", "no agent is unsandboxed");

        let (status, _) = request(engine, "GET", "/api/quality?scope=nope", None).await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn post_api_quality_remediate_refuses_a_met_scenario_with_a_400() {
        let engine = engine_with_quality();
        let body = r#"{"scope":"company","attribute":"reliability","scenario":"sandboxed"}"#;
        let (status, json) = request(engine, "POST", "/api/quality/remediate", Some(body)).await;
        assert_eq!(status, 400, "{json}");
        assert!(json["message"].as_str().unwrap_or_default().contains("already met"), "{json}");
    }

    async fn serve() -> std::net::SocketAddr {
        let root = std::env::temp_dir().join(format!("factory-http-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
            scope: None,
            scopes: vec![serde_yaml_ng::from_str("id: demo-id\nname: demo\npath: .\n").unwrap()],
            roles: Default::default(),
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = Arc::new(Engine::new(Factory { root, config }, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router(engine)).await.unwrap() });
        addr
    }

    /// One HTTP/1.1 request, `Connection: close`; the status code and body.
    async fn call(addr: std::net::SocketAddr, method: &str, path: &str, content_type: bool, body: &str) -> (u16, String) {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let ct = if content_type { "Content-Type: application/json\r\n" } else { "" };
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n{ct}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).await.unwrap();
        let code = raw.split(' ').nth(1).unwrap().parse().unwrap();
        let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
        (code, body)
    }

    #[tokio::test]
    async fn the_operations_route_answers_the_report_for_either_window_and_refuses_another() {
        let addr = serve().await;
        let (code, body) = call(addr, "GET", "/api/operations", false, "").await;
        assert_eq!(code, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["data"]["kind"], "operations", "{body}");
        assert_eq!(v["data"]["report"]["health"]["window"], "7d");

        let (code, body) = call(addr, "GET", "/api/operations?scope=demo&window=30d", false, "").await;
        assert_eq!(code, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["data"]["report"]["health"]["window"], "30d");
        assert_eq!(v["data"]["report"]["scope"], "demo");

        let (code, _) = call(addr, "GET", "/api/operations?window=9d", false, "").await;
        assert_eq!(code, 400);
        let (code, _) = call(addr, "GET", "/api/operations?detail=charts", false, "").await;
        assert_eq!(code, 200);
        let (code, _) = call(addr, "GET", "/api/operations?detail=everything", false, "").await;
        assert_eq!(code, 400, "only `charts` is a detail");
        let (code, _) = call(addr, "GET", "/api/operations?scope=nowhere", false, "").await;
        assert_eq!(code, 404, "an unknown scope is not an empty report");
    }

    #[tokio::test]
    async fn the_occupancy_route_takes_an_explicit_window_and_projects_schedules_across_it() {
        let root = std::env::temp_dir().join(format!("factory-http-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
            scope: None,
            scopes: vec![serde_yaml_ng::from_str(
                "id: demo-id\nname: demo\npath: .\nagents:\n  - name: builder\n    harness: shell\n",
            )
            .unwrap()],
            roles: Default::default(),
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = Arc::new(Engine::new(Factory { root, config }, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new()));
        // Whole seconds, so the window read back compares equal to the one sent.
        let now = chrono::SubsecRound::trunc_subsecs(chrono::Utc::now(), 0);
        let mut task = factory_core::adapter::store::task_from_new(
            factory_core::task::NewTask {
                title: "hourly".into(),
                schedule: Some(factory_core::task::Schedule::Every { seconds: 3600 }),
                ..Default::default()
            },
            "demo".into(),
            "builder".into(),
            "herdr".into(),
        );
        task.next_run_at = Some(now + chrono::Duration::minutes(30));
        engine.store.create(&task).await.unwrap();
        let iso = |t: chrono::DateTime<chrono::Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        // A day ahead, well beyond the quarter the live window keeps: the
        // window comes back as asked, and the hourly task is in it six times.
        let (from, to) = (now + chrono::Duration::days(1), now + chrono::Duration::days(1) + chrono::Duration::hours(6));
        let path = format!("/api/occupancy?from={}&to={}", iso(from), iso(to));
        let (status, json) = request(engine.clone(), "GET", &path, None).await;
        assert_eq!(status, 200, "{json}");
        let occ = &json["data"]["occupancy"];
        assert_eq!(occ["from"].as_str().unwrap().parse::<chrono::DateTime<chrono::Utc>>().unwrap(), from);
        assert_eq!(occ["to"].as_str().unwrap().parse::<chrono::DateTime<chrono::Utc>>().unwrap(), to);
        let planned = |json: &serde_json::Value| {
            let rows = json["data"]["occupancy"]["scopes"][0]["rows"].as_array().unwrap().clone();
            let row = rows.into_iter().find(|r| r["agent"] == "builder").expect("the builder's row");
            row["planned"].as_array().unwrap().len()
        };
        assert_eq!(planned(&json), 6, "{json}");

        // The live window still has only the next firing in its quarter ahead.
        let (status, json) = request(engine.clone(), "GET", "/api/occupancy?minutes=60", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(planned(&json), 0, "fifteen minutes ahead does not reach it");
        let (_, json) = request(engine.clone(), "GET", "/api/occupancy?minutes=240", None).await;
        assert_eq!(planned(&json), 1, "an hour ahead reaches the next firing only");

        // Backwards is refused by the engine; nonsense by the query parser.
        let path = format!("/api/occupancy?from={}&to={}", iso(to), iso(from));
        let (status, json) = request(engine.clone(), "GET", &path, None).await;
        assert_eq!(status, 400, "{json}");
        let (status, _) = request(engine, "GET", "/api/occupancy?from=yesterday&to=today", None).await;
        assert_eq!(status, 400);
    }

    #[tokio::test]
    async fn run_and_cancel_take_an_empty_body_or_a_reason_and_refuse_anything_else() {
        let addr = serve().await;
        // The web UI's POST: a JSON content type and no body at all. It must
        // reach the engine, which then says there is no such task.
        let (code, body) = call(addr, "POST", "/api/tasks/nope/run", true, "").await;
        assert_eq!(code, 404, "{body}");
        let (_, body) = call(addr, "POST", "/api/tasks/nope/cancel", true, r#"{"reason":"wrong branch"}"#).await;
        assert!(body.contains("no run to cancel"), "the engine answered, not the body parser: {body}");
        let (code, body) = call(addr, "POST", "/api/tasks/nope/skip-next", false, "").await;
        assert_eq!(code, 404, "{body}");
        let (code, body) = call(addr, "POST", "/api/tasks/nope/run", true, "not json").await;
        assert_eq!(code, 400);
        let v: serde_json::Value = serde_json::from_str(&body).expect("the error envelope, not bare text");
        assert_eq!(v["status"], "error");
        assert!(v["message"].as_str().unwrap().contains("not a reason body"), "{body}");
        let (code, body) =
            call(addr, "POST", "/api/tasks/nope/skip-next", true, r#"{"reason":"r","slot":"2026-09-25T09:00:00Z"}"#).await;
        assert_eq!(code, 404, "{body}");
        let (_, body) = call(addr, "POST", "/api/tasks/nope/cancel", true, r#"{"reason":"r","run_id":"r1"}"#).await;
        assert!(body.contains("no run to cancel"), "a cancel naming its run reaches the engine: {body}");
        let (code, _) = call(addr, "POST", "/api/tasks/nope/cancel", true, r#"{"run_id":5}"#).await;
        assert_eq!(code, 400);
        // The task form's PATCH, with and without a reason beside the
        // patch's own fields, reaches the engine: the flattened body
        // must not refuse what a bare `TaskPatch` took.
        let (code, body) = call(addr, "PATCH", "/api/tasks/nope", true, r#"{"title":"x"}"#).await;
        assert_eq!(code, 404, "{body}");
        let (code, body) = call(
            addr,
            "PATCH",
            "/api/tasks/nope",
            true,
            r#"{"schedule_paused":true,"schedule":{"cron":{"expr":"0 9 * * 1","timezone":"Europe/Berlin"}},"reason":"r"}"#,
        )
        .await;
        assert_eq!(code, 404, "{body}");
        let (code, body) =
            call(addr, "PATCH", "/api/tasks/nope", true, r#"{"schedule":{"cron":"0 7 * * 1"},"clear_estimate":true}"#).await;
        assert_eq!(code, 404, "{body}");
        // An answer's reason is not optional.
        let (code, _) = call(addr, "POST", "/api/runs/nope/answer", true, r#"{"text":"yes"}"#).await;
        assert_eq!(code, 422);
    }
}
