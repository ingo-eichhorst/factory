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
        let bind = ctx.config.http_bind();

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
        .route("/api/secrets/{name}", put(secret_set))
        .route("/api/dependencies", get(dependencies))
        .route("/api/dependencies/documents/{id}", get(dependency_document))
        .route("/api/doctor", get(doctor))
        // The L1 Mac tab (`#260`): read the power mode, or set it.
        .route("/api/host/power-mode", get(host_power_mode).post(host_power_mode_set))
        .route("/api/important-dates", get(important_dates))
        .route("/api/infrastructure", get(infrastructure))
        .route("/api/environments", get(environments))
        .route("/api/environments/promote", post(environment_promote))
        .route("/api/environments/recover", post(environment_recover))
        .route("/api/environments/check", post(environment_check))
        .route("/api/environments/samples", get(environment_samples))
        .route("/api/deployments", post(deploy_start))
        .route("/api/deployments/{id}/finish", post(deploy_finish))
        .route("/api/deployments/{id}/mirror-plan", get(deploy_mirror_plan))
        .route("/api/deployments/{id}/publish", post(deploy_publish))
        .route("/api/releases", post(release_add))
        .route("/api/releases/detail", get(release_detail))
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
        // The CRA Art. 14 reporting clock (`#157`, phase 1) -- read-only,
        // the same subtree-or-instance resolution `GET /api/policy` uses.
        .route("/api/policy/clock", get(policy_clock))
        .route("/api/policy/controls/{framework}/{id}", get(policy_control))
        .route("/api/policy/attestations", post(create_attestation))
        .route(
            "/api/policy/attestations/{id}/withdraw",
            post(withdraw_attestation),
        )
        .route("/api/policy/remediate", post(policy_remediate))
        .route("/api/policy/export", get(policy_export))
        .route("/api/metrics", get(metrics))
        .route("/api/dashboard", get(dashboard).put(dashboard_set).delete(dashboard_reset))
        .route("/api/costs", get(costs))
        .route("/api/budget", get(budget))
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
        .route("/api/intake/security-reports", get(intake_security_reports))
        .route("/api/intake/{id}/triage", post(intake_triage))
        .route("/api/intake/{id}/assess", post(intake_assess))
        .route("/api/intake/{id}/decide", post(intake_decide))
        .route("/api/intake/{id}/info", post(intake_info))
        .route("/api/intake/{id}/flag-security", post(intake_flag_security))
        .route("/api/intake/{id}/security", post(intake_security))
        .route("/api/intake/{id}/publish", post(intake_publish))
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
        .route("/api/runs/{id}/provenance", get(run_provenance))
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
    // Keep the protocol future off axum's deeper extractor/routing stack.
    // Review/approval orchestration grows that future even for read-only
    // requests such as the roster used at browser startup.
    let response = Box::pin(engine.handle(Envelope { request, token })).await;
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
    // Boxed for the same reason `run_as` boxes: unboxed, the whole request
    // future sits on axum's own frames, and a debug build overflows the
    // worker's stack on it. This is the route a sandboxed run reports
    // through (`#218`), so it must hold up as well as the socket does.
    //
    // And in a task of its own, so it runs to the end whatever happens to
    // the caller (`#234`). A sandboxed run's `task report --status done`
    // closes the run's session, whose teardown deletes the very sandbox the
    // reporting CLI is still waiting in; the dropped connection would
    // otherwise cancel this handler between closing the session and
    // recording `done`, and the watchdog would fail a finished run as
    // `session_gone`.
    let response = match tokio::spawn(async move { Box::pin(engine.handle(env)).await }).await {
        Ok(response) => response,
        Err(e) => Response::error("internal", format!("the request failed: {e}")),
    };
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

/// `PUT /api/secrets/{name}` -- a declared secret's `expires`, `renew` and
/// `note`, replaced whole (`#244`). The body is metadata; there is no field
/// a value could be sent in.
async fn secret_set(
    State(engine): State<Arc<Engine>>,
    Path(name): Path<String>,
    Json(metadata): Json<factory_core::secrets::SecretMetadata>,
) -> AxumResponse {
    run(&engine, Request::SecretSet { name, metadata }).await
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

#[derive(serde::Deserialize)]
struct EnvironmentsQuery {
    #[serde(default)]
    scope: Option<String>,
}

/// `GET /api/environments?scope=` -- the Operations tab (`#185`).
async fn environments(State(engine): State<Arc<Engine>>, Query(q): Query<EnvironmentsQuery>) -> AxumResponse {
    run(&engine, Request::Environments { scope: q.scope }).await
}

async fn environment_promote(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<factory_core::environments::Promote>,
) -> AxumResponse {
    run(&engine, Request::EnvironmentPromote(body)).await
}

async fn environment_recover(State(engine): State<Arc<Engine>>, Json(body): Json<factory_core::environments::Recover>) -> AxumResponse {
    run(&engine, Request::EnvironmentRecover(body)).await
}

#[derive(serde::Deserialize)]
struct EnvironmentCheckBody { environment: String }
async fn environment_check(State(engine): State<Arc<Engine>>, Json(body): Json<EnvironmentCheckBody>) -> AxumResponse {
    run(&engine, Request::EnvironmentCheck { environment: body.environment }).await
}

async fn environment_samples(State(engine): State<Arc<Engine>>, Query(query): Query<factory_core::environments::SampleQuery>) -> AxumResponse {
    run(&engine, Request::EnvironmentSamples(query)).await
}

async fn release_detail(State(engine): State<Arc<Engine>>, Query(query): Query<factory_core::environments::ReleaseQuery>) -> AxumResponse {
    run(&engine, Request::ReleaseDetail(query)).await
}

#[derive(serde::Deserialize)]
struct DependencyDocumentQuery { scope: String }
async fn dependency_document(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Query(query): Query<DependencyDocumentQuery>) -> AxumResponse {
    run(&engine, Request::DependencyDocument { scope: query.scope, id }).await
}

/// `POST /api/deployments` with a `deploy.start` body.
async fn deploy_start(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<factory_core::environments::DeployStart>,
) -> AxumResponse {
    run(&engine, Request::DeployStart(body)).await
}

#[derive(serde::Deserialize)]
struct DeployFinishBody {
    status: factory_core::environments::DeployStatus,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    verify: Option<bool>,
}

async fn deploy_mirror_plan(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::DeployMirrorPlan { id }).await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeployPublishBody { approval: String }

async fn deploy_publish(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<DeployPublishBody>) -> AxumResponse {
    run(&engine, Request::DeployPublish { id, approval: body.approval }).await
}

/// `POST /api/deployments/{id}/finish` `{status, reason?, verify?}`.
async fn deploy_finish(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<DeployFinishBody>,
) -> AxumResponse {
    let req = factory_core::environments::DeployFinish {
        id,
        status: body.status,
        reason: body.reason,
        verify: body.verify.unwrap_or(true),
    };
    run(&engine, Request::DeployFinish(req)).await
}

/// `POST /api/releases` with a `release.add` body.
async fn release_add(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<factory_core::environments::ReleaseAdd>,
) -> AxumResponse {
    run(&engine, Request::ReleaseAdd(body)).await
}

async fn doctor(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Doctor).await
}

async fn important_dates(State(engine): State<Arc<Engine>>, Query(q): Query<PolicyQuery>) -> AxumResponse {
    run(&engine, Request::ImportantDates { scope: q.scope.filter(|scope| !scope.trim().is_empty()) }).await
}

async fn host_power_mode(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::HostPowerMode).await
}

/// `POST /api/host/power-mode`'s body: `{"mode": "automatic" |
/// "high_performance" | "energy_saving"}` and nothing else.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PowerModeBody {
    mode: factory_core::protocol::PowerMode,
}

/// Read by hand, so an empty body, a number, an unknown name or an extra
/// field is a 400 in the usual envelope -- refused here, before the engine
/// is asked and long before any command could be built (`#260`).
fn power_mode_of(body: &[u8]) -> std::result::Result<factory_core::protocol::PowerMode, String> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err("a power mode is required: {\"mode\": \"automatic\" | \"high_performance\" | \"energy_saving\"}".into());
    }
    serde_json::from_slice::<PowerModeBody>(body)
        .map(|b| b.mode)
        .map_err(|e| format!("not a power mode: {e}; use automatic, high_performance or energy_saving"))
}

async fn host_power_mode_set(State(engine): State<Arc<Engine>>, body: axum::body::Bytes) -> AxumResponse {
    match power_mode_of(&body) {
        Ok(mode) => run(&engine, Request::HostPowerModeSet { mode }).await,
        Err(why) => refused(why),
    }
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
/// POST works. `identity` (`#152`) is never accepted over HTTP: decrypting
/// an encrypted snapshot is CLI-only, like restore.
async fn backup_verify(State(engine): State<Arc<Engine>>, Query(q): Query<VerifyQuery>) -> AxumResponse {
    run(&engine, Request::BackupVerify { snapshot: q.snapshot, identity: None }).await
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

/// `GET /api/policy/clock?scope=` -- the CRA Art. 14 reporting clock
/// (`#157`, phase 1): `scope` empty or absent means the whole instance,
/// exactly like `GET /api/policy` itself.
async fn policy_clock(State(engine): State<Arc<Engine>>, Query(q): Query<PolicyQuery>) -> AxumResponse {
    run(
        &engine,
        Request::PolicyClock {
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
    /// A CRA Art. 14 reporting-clock item this attestation also submits
    /// against (`#157`, phase 1): `finding:<scope>:<vulnerability>` or
    /// `report:<task-id>`. Given together with `deadline` or not at all.
    #[serde(default)]
    clock_item: Option<String>,
    /// Which deadline `clock_item` submits: early_warning, notification
    /// or final_report (hyphenated spellings are accepted too).
    #[serde(default)]
    deadline: Option<String>,
    #[serde(default)]
    corrective_item: Option<String>,
    #[serde(default)]
    available_at: Option<chrono::DateTime<chrono::Utc>>,
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
    let clock = match (body.clock_item, body.deadline) {
        (Some(item), Some(deadline)) => {
            let item: factory_core::reporting_clock::ClockItemRef = match item.parse() {
                Ok(v) => v,
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response()
                }
            };
            let deadline: factory_core::reporting_clock::ClockDeadlineKind = match deadline.parse() {
                Ok(v) => v,
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response()
                }
            };
            Some(factory_core::reporting_clock::ClockMark { item, deadline })
        }
        (None, None) => None,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(Response::error("bad_request", "clock_item and deadline must be given together")),
            )
                .into_response()
        }
    };
    let corrective = match corrective_mark(body.corrective_item, body.available_at) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response(),
    };
    run(
        &engine,
        Request::PolicyAttest {
            control,
            scope: body.scope,
            evidence: body.evidence,
            note: body.note,
            expires_at,
            clock,
            corrective,
        },
    )
    .await
}

fn corrective_mark(
    item: Option<String>,
    available_at: Option<chrono::DateTime<chrono::Utc>>,
) -> std::result::Result<Option<factory_core::reporting_clock::CorrectiveMeasureMark>, String> {
    match (item, available_at) {
        (Some(item), Some(available_at)) => {
            Ok(Some(factory_core::reporting_clock::CorrectiveMeasureMark { item: item.parse()?, available_at }))
        }
        (None, None) => Ok(None),
        _ => Err("corrective_item and available_at must be given together".into()),
    }
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

/// `GET /api/costs?group_by=task|issue|scope|agent|provider|workflow&from=&to=&scope=`
/// -- usage and cost summed per group over the runs that started in the
/// window (#117; `workflow` since #164). `from`/`to` are RFC 3339; the
/// window defaults to the last thirty days. A grouping this endpoint does
/// not offer is a 400 that says so, not an empty answer.
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

#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetQuery {
    scope: Option<String>,
    group_by: Option<String>,
}

async fn budget(State(engine): State<Arc<Engine>>, Query(q): Query<BudgetQuery>) -> AxumResponse {
    let group_by = match q.group_by.as_deref().unwrap_or("scope").parse() {
        Ok(value) => value,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response(),
    };
    run(&engine, Request::Budget { scope: q.scope.filter(|s| !s.trim().is_empty()), group_by }).await
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
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    window: Option<factory_core::metrics::MetricsWindow>,
}

/// `GET /api/metrics?ids=a,b&scope=&window=day|14d|90d` -- every named
/// metric's computed value, its history where it has one, and the definition
/// behind it.
async fn metrics(State(engine): State<Arc<Engine>>, Query(q): Query<MetricsQuery>) -> AxumResponse {
    let mut ids = Vec::new();
    for raw in q.ids.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match raw.parse() {
            Ok(id) => ids.push(id),
            Err(e) => return (StatusCode::BAD_REQUEST, Json(Response::error("bad_request", e))).into_response(),
        }
    }
    run(
        &engine,
        Request::Metrics {
            ids,
            scope: q.scope.filter(|s| !s.trim().is_empty()),
            window: q.window,
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct DashboardQuery {
    #[serde(default)]
    scope: Option<String>,
}

/// `GET /api/dashboard?scope=` -- the resolved dashboard layout for `scope`
/// (the instance root's own when left out), and where it came from
/// (`#159`). An unknown scope is a 404, like `/api/quality`'s own.
async fn dashboard(State(engine): State<Arc<Engine>>, Query(q): Query<DashboardQuery>) -> AxumResponse {
    run(
        &engine,
        Request::Dashboard {
            scope: q.scope.filter(|s| !s.trim().is_empty()),
        },
    )
    .await
}

#[derive(serde::Deserialize)]
struct DashboardSetBody {
    tiles: Vec<factory_core::dashboard::Tile>,
}

/// `scope` is required on the write side of `/api/dashboard`, unlike the
/// read: saving or resetting means naming which scope's own block this is,
/// not falling back to the caller's (`#160`). `None` for missing or blank;
/// the caller turns that into the 400 -- kept a plain `Option` rather than a
/// `Result<_, AxumResponse>` so a missing scope does not make every success
/// path carry a whole HTTP response's worth of `Err` around with it.
fn required_scope(q: &DashboardQuery) -> Option<String> {
    q.scope.clone().filter(|s| !s.trim().is_empty())
}

fn scope_required_response() -> AxumResponse {
    (
        StatusCode::BAD_REQUEST,
        Json(Response::error("bad_request", "scope is required")),
    )
        .into_response()
}

/// `PUT /api/dashboard?scope=` -- save `scope`'s own layout whole (`#160`).
/// An unknown metric or an empty `tiles` is refused with a 400 naming the
/// tile; an unknown scope is a 404, the same as the read side.
async fn dashboard_set(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<DashboardQuery>,
    Json(body): Json<DashboardSetBody>,
) -> AxumResponse {
    let Some(scope) = required_scope(&q) else {
        return scope_required_response();
    };
    run(&engine, Request::DashboardSet { scope, tiles: body.tiles }).await
}

/// `DELETE /api/dashboard?scope=` -- remove `scope`'s own block and reveal
/// whatever it was overriding (`#160`). Refused with a 400 when `scope`
/// writes no block of its own to remove.
async fn dashboard_reset(State(engine): State<Arc<Engine>>, Query(q): Query<DashboardQuery>) -> AxumResponse {
    let Some(scope) = required_scope(&q) else {
        return scope_required_response();
    };
    run(&engine, Request::DashboardReset { scope }).await
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

#[derive(serde::Deserialize)]
struct IntakeFlagSecurityBody {
    #[serde(default)]
    reason: String,
}

/// `POST /api/intake/{id}/flag-security` -- `{reason}` (`#170`). Flagging
/// only adds scrutiny, so it needs no more than `intake.assess` already does.
async fn intake_flag_security(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<IntakeFlagSecurityBody>,
) -> AxumResponse {
    run(&engine, Request::IntakeFlagSecurity { id, reason: body.reason }).await
}

#[derive(serde::Deserialize)]
struct IntakeSecurityBody {
    verdict: factory_core::intake::SecurityVerdict,
    #[serde(default)]
    evidence: String,
}

/// `POST /api/intake/{id}/security` -- `{verdict, evidence?}` (`#170`).
/// Confirming or dismissing is the owner's alone: the UI sends no token, so
/// every browser caller already is the owner (`run`, below).
async fn intake_security(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<IntakeSecurityBody>,
) -> AxumResponse {
    run(&engine, Request::IntakeSecurity { id, verdict: body.verdict, evidence: body.evidence }).await
}

/// `GET /api/intake/security-reports?scope=` -- every confirmed security
/// report over the scope's subtree (`#170`), the L4 fact `#157`'s reporting
/// clock will read.
async fn intake_security_reports(State(engine): State<Arc<Engine>>, Query(q): Query<IntakeQuery>) -> AxumResponse {
    run(&engine, Request::IntakeSecurityReports { scope: q.scope }).await
}

/// `POST /api/intake/{id}/publish` -- approve and post a decided GitHub
/// item's triage comment and labels to the issue it came from (`#171`).
/// Every browser caller is the owner (`run`, above, sends no token), which
/// always passes; posting here *is* the approval.
async fn intake_publish(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::IntakePublish { id }).await
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
    scope: Option<String>,
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
            scope: body.scope,
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

/// `POST /api/tasks/{id}/run`'s body: `{"reason": "...", "continue": true}`,
/// either field optional, or nothing at all. `#178`'s `continue` mirrors the
/// CLI's `--continue` and the socket's `task.run.continue`.
#[derive(serde::Deserialize, Default)]
struct RunTaskBody {
    #[serde(default)]
    reason: Option<String>,
    #[serde(default, rename = "continue")]
    continue_run: bool,
    #[serde(default)]
    override_wait: bool,
}

/// Read the same way `reason_of` reads `ReasonBody` -- an empty body is
/// every field at its default, not a parse error.
fn run_task_body_of(body: &[u8]) -> std::result::Result<RunTaskBody, String> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(RunTaskBody::default());
    }
    serde_json::from_slice::<RunTaskBody>(body).map_err(|e| format!("not a reason body: {e}"))
}

async fn run_task(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> AxumResponse {
    match run_task_body_of(&body) {
        Ok(body) => run(&engine, Request::TaskRun { id, reason: body.reason, continue_run: body.continue_run, override_wait: body.override_wait }).await,
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

async fn run_provenance(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::RunProvenance { id }).await
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

    #[test]
    fn run_body_keeps_overrides_explicit_and_backward_compatible() {
        assert!(!super::run_task_body_of(b"").unwrap().override_wait);
        assert!(!super::run_task_body_of(br#"{"continue":true}"#).unwrap().override_wait);
        let body = super::run_task_body_of(br#"{"override_wait":true,"reason":"investigate"}"#).unwrap();
        assert!(body.override_wait);
        assert_eq!(body.reason.as_deref(), Some("investigate"));
    }

    #[test]
    fn corrective_measure_http_requires_both_fields_and_valid_item() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-01T09:00:00Z").unwrap().with_timezone(&chrono::Utc);
        assert!(super::corrective_mark(None, None).unwrap().is_none());
        assert!(super::corrective_mark(Some("report:t1".into()), Some(at)).unwrap().is_some());
        assert!(super::corrective_mark(Some("report:t1".into()), None).is_err());
        assert!(super::corrective_mark(None, Some(at)).is_err());
        assert!(super::corrective_mark(Some("unknown:t1".into()), Some(at)).is_err());
    }

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
            dashboard: None,
            policies: PolicyDeclaration::default(),
            quality: vec!["baseline".into()],
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
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

    /// A runtime whose `stop` takes a while, the way closing a sandboxed
    /// run's pane and starting its teardown does.
    struct SlowStop;

    #[async_trait::async_trait]
    impl factory_core::adapter::AgentRuntime for SlowStop {
        fn name(&self) -> &str {
            "slow-stop"
        }
        async fn start(&self, req: &factory_core::adapter::runtime::StartRequest) -> Result<factory_core::task::SessionRef> {
            Ok(factory_core::task::SessionRef { runtime: "slow-stop".into(), handle: req.id.clone(), meta: Default::default() })
        }
        async fn submit(&self, _: &factory_core::task::SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _: &factory_core::task::SessionRef) -> Result<factory_core::adapter::runtime::RuntimeStatus> {
            Ok(factory_core::adapter::runtime::RuntimeStatus::Working)
        }
        async fn send_text(&self, _: &factory_core::task::SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn send_keys(&self, _: &factory_core::task::SessionRef, _: &[String]) -> Result<()> {
            Ok(())
        }
        async fn read(&self, _: &factory_core::task::SessionRef, _: u32) -> Result<String> {
            Ok(String::new())
        }
        async fn stop(&self, _: &factory_core::task::SessionRef) -> Result<()> {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok(())
        }
        async fn watch(&self) -> Result<Option<factory_core::adapter::runtime::RuntimeEventStream>> {
            Ok(None)
        }
    }

    /// `#234`: a caller that goes away mid-request -- a sandboxed run's
    /// CLI, in the sandbox its own `done` is tearing down -- does not
    /// cancel the request. A `done` dropped while its session closes is
    /// still recorded, rather than leaving the run for the watchdog to fail
    /// as `session_gone`.
    #[tokio::test]
    async fn a_done_whose_caller_disconnects_while_its_session_closes_is_still_recorded() {
        use futures_util::FutureExt;
        let base = engine_with_quality();
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(SlowStop), "test");
        let engine = Arc::new(Engine::new(base.factory_snapshot(), registry, base.store.clone(), PathBuf::from("factory"), Vec::new()));
        let task = engine
            .create(factory_core::task::NewTask {
                title: "reports from a sandbox".into(),
                instructions: "true".into(),
                scope: Some("company".into()),
                agent: Some("shell".into()),
                runtime: Some("slow-stop".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let run = engine
            .dispatch(&task.id, factory_core::run::Trigger::Manual, crate::engine::Due::now(), None)
            .await
            .unwrap();
        let envelope: Envelope = serde_json::from_value(serde_json::json!({
            "op": "task.report",
            "params": { "id": task.id, "report": { "status": "done", "message": "finished", "token": run.token } },
            "token": run.token,
        }))
        .unwrap();
        assert!(rpc(State(engine.clone()), Json(envelope)).now_or_never().is_none(), "still in flight when dropped");
        let mut status = None;
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            status = engine.store.get_run(&run.id).await.unwrap().map(|r| r.status);
            if status == Some(factory_core::run::RunStatus::Done) {
                break;
            }
        }
        assert_eq!(status, Some(factory_core::run::RunStatus::Done), "the dropped report was cancelled half way");
    }

    #[tokio::test]
    async fn provenance_http_reads_the_completed_run_and_rejects_invalid_reports() {
        let engine = engine_with_quality();
        let task = engine
            .create(factory_core::task::NewTask {
                title: "release".into(),
                instructions: "true".into(),
                scope: Some("company".into()),
                agent: Some("shell".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let run = engine
            .store
            .create_run(&factory_core::run::NewRun {
                task_id: task.id.clone(),
                trigger: factory_core::run::Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "herdr".into(),
                token: "callback".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        let path = format!("/api/runs/{}/provenance", run.id);
        let (status, json) = request(engine.clone(), "GET", &path, None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["kind"], "run_provenance");
        assert_eq!(json["data"]["records"], serde_json::json!([]));
        let (status, _) = request(
            engine.clone(),
            "GET",
            "/api/runs/nonexistent/provenance",
            None,
        )
        .await;
        assert_eq!(status, 404);
        let report = format!("/api/tasks/{}/report", task.id);
        let (status, json) = request(
            engine.clone(),
            "POST",
            &report,
            Some(r#"{"status":"running","artifacts":["release.bin"],"token":"callback"}"#),
        )
        .await;
        assert_eq!(status, 400, "{json}");
        let (status, json) = request(
            engine.clone(),
            "POST",
            &report,
            Some(r#"{"status":"done","token":"callback"}"#),
        )
        .await;
        assert_eq!(status, 200, "{json}");
        let (status, json) = request(engine, "GET", &path, None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["records"], serde_json::json!([]));
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
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["report"]["group_by"], "provider");

        let (status, json) = request(engine.clone(), "GET", "/api/costs?group_by=workflow", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["report"]["group_by"], "workflow");

        let (status, json) = request(engine.clone(), "GET", "/api/costs?group_by=bogus", None).await;
        assert_eq!(status, 400, "{json}");
        assert!(json["message"].as_str().unwrap().contains("bogus"), "{json}");

        let (status, _) = request(engine, "GET", "/api/tasks/nope/usage", None).await;
        assert_eq!(status, 404);
    }

    /// `#260`: anything but one of the three names is a 400 that runs
    /// nothing; a name runs exactly its one command.
    #[tokio::test]
    async fn post_api_host_power_mode_refuses_every_other_value_before_any_command() {
        use crate::host_power::testing::FakeHost;
        let engine = engine_with_quality();
        let host = FakeHost::mac();
        host.install_rule();
        engine.host_power.replace_runner(host.clone());

        for body in [
            r#"{"mode":"3"}"#,
            r#"{"mode":3}"#,
            r#"{"mode":"auto; rm"}"#,
            r#"{"mode":""}"#,
            r#"{"mode":"Automatic"}"#,
            r#"{"mode":"automatic","extra":1}"#,
            r#"{}"#,
            "",
            "3",
            "not json",
        ] {
            let (status, json) = request(engine.clone(), "POST", "/api/host/power-mode", Some(body)).await;
            assert_eq!(status, 400, "{body:?}: {json}");
            assert_eq!(json["code"], "bad_request", "{body:?}: {json}");
            assert!(host.calls().is_empty(), "{body:?} reached a command: {:?}", host.calls());
        }

        let (status, json) = request(engine.clone(), "GET", "/api/host/power-mode", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["kind"], "host_power_mode");
        assert_eq!(json["data"]["report"]["ac"], "automatic");
        assert_eq!(json["data"]["report"]["can_change"], true);
        assert!(host.writes().is_empty(), "a read writes nothing");

        let (status, json) =
            request(engine.clone(), "POST", "/api/host/power-mode", Some(r#"{"mode":"high_performance"}"#)).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["report"]["ac"], "high_performance");
        assert_eq!(json["data"]["report"]["battery"], "high_performance");
        assert_eq!(json["data"]["report"]["changes"][0]["to"], "high_performance");
        assert_eq!(
            host.writes(),
            vec![vec!["/usr/bin/sudo", "-n", "-k", "-u", "root", "--", "/usr/bin/pmset", "-a", "powermode", "2"]]
        );
    }

    #[tokio::test]
    async fn budget_http_default_group_all_groups_and_strict_bad_queries() {
        let engine = engine_with_quality();
        for group in ["scope", "agent", "issue", "workflow", "provider"] {
            let (status,json) = request(engine.clone(), "GET", &format!("/api/budget?group_by={group}"), None).await;
            assert_eq!(status, 200, "{json}");
            assert_eq!(json["data"]["kind"], "budget");
            assert_eq!(json["data"]["report"]["group_by"], group);
            assert_eq!(json["data"]["report"]["spend"]["total"]["runs"], 0);
        }
        let (status,json) = request(engine.clone(), "GET", "/api/budget", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["report"]["group_by"], "scope");
        let (status,_) = request(engine.clone(), "GET", "/api/budget?scope=nope", None).await;
        assert_eq!(status, 404);
        for uri in ["/api/budget?group_by=bogus", "/api/budget?group_by=", "/api/budget?monthly_usd=500"] {
            let (status, _) = request(engine.clone(), "GET", uri, None).await;
            assert_eq!(status, 400, "{uri}");
        }
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
    async fn get_api_metrics_accepts_scope_and_window_and_rejects_unknown_values() {
        let engine = engine_with_quality();
        let (status, json) = request(
            engine.clone(),
            "GET",
            "/api/metrics?ids=agent_hours,bench.resolve_rate.missing&scope=company&window=14d",
            None,
        )
        .await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["kind"], "metrics");
        let registry = json["data"]["registry"].as_array().unwrap();
        assert_eq!(registry[0]["id"], "agent_hours");
        assert_eq!(registry[0]["unit"], "hours");
        assert_eq!(registry[0]["coverage"], "scope_aware");
        assert_eq!(registry[1]["coverage"], "instance_wide");

        let (status, _) = request(engine.clone(), "GET", "/api/metrics?window=7d", None).await;
        assert_eq!(status, 400);
        let (status, _) = request(engine, "GET", "/api/metrics?scope=missing", None).await;
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

    fn engine_with_dashboard() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-http-test-{}", uuid::Uuid::new_v4()));
        let mut company: Scope = serde_yaml_ng::from_str("id: company-id\nname: company\n").unwrap();
        company.path = PathBuf::from(".");
        let mut demo: Scope = serde_yaml_ng::from_str(
            "id: demo-id\nname: demo\ndashboard:\n  tiles:\n    - { view: kpis, size: m }\n",
        )
        .unwrap();
        demo.path = PathBuf::from("demo");
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(company.clone()),
            scopes: vec![company, demo],
            roles: Default::default(),
            dashboard: Some(
                serde_yaml_ng::from_str("tiles:\n  - { metric: throughput_week, size: s }\n").unwrap(),
            ),
            policies: PolicyDeclaration::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(Factory { root, config }, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new()))
    }

    #[tokio::test]
    async fn get_api_dashboard_resolves_the_layout_and_an_unknown_scope_is_a_404() {
        let engine = engine_with_dashboard();
        let (status, json) = request(engine.clone(), "GET", "/api/dashboard", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["kind"], "dashboard");
        // The root's own configured scope is named "company", so that is
        // what `source` says answered -- never the magic string "root".
        assert_eq!(json["data"]["source"], "company", "no scope given resolves the instance root's own");
        assert_eq!(json["data"]["tiles"][0]["metric"], "throughput_week");

        // The root's own scope carries no override of its own, so naming it
        // explicitly falls through to the same root block.
        let (status, json) = request(engine.clone(), "GET", "/api/dashboard?scope=company", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["source"], "company");

        let (status, json) = request(engine.clone(), "GET", "/api/dashboard?scope=demo", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["source"], "demo", "demo's own override wins over the root's");
        assert_eq!(json["data"]["tiles"][0]["view"], "kpis");
        assert_eq!(json["data"]["tiles"][0]["size"], "m");

        let (status, _) = request(engine, "GET", "/api/dashboard?scope=nope", None).await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn get_api_dashboard_with_no_block_anywhere_answers_null_tiles_and_null_source() {
        // `engine_with_quality` declares no `dashboard:` at all, root or scope.
        let engine = engine_with_quality();
        let (status, json) = request(engine, "GET", "/api/dashboard", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["tiles"], serde_json::Value::Null, "use the UI's built-in default");
        assert_eq!(json["data"]["source"], serde_json::Value::Null, "never the word \"default\"");
    }

    /// A real instance on disk, root and one nested scope, so `PUT`/`DELETE`
    /// (which read and rewrite a real config file, unlike the in-memory-only
    /// `engine_with_dashboard` above) have something to write to.
    fn engine_with_dashboard_files() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-http-dashboard-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory")).unwrap();
        std::fs::write(
            root.join(".factory/config.yaml"),
            "version: 1\ninstance:\n  id: test\n  name: test\nscope:\n  id: company-id\n  name: company\n\
             dashboard:\n  tiles:\n    - { metric: throughput_week, size: s }\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("demo/.factory")).unwrap();
        std::fs::write(
            root.join("demo/.factory/config.yaml"),
            "version: 1\nscope:\n  id: demo-id\n  name: demo\n",
        )
        .unwrap();
        let mut factory = factory_core::config::Factory::load(&root).unwrap();
        crate::discovery::apply(&mut factory).unwrap();
        factory.config.validate().unwrap();
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new()))
    }

    #[tokio::test]
    async fn put_api_dashboard_saves_and_round_trips_through_a_get() {
        let engine = engine_with_dashboard_files();
        let body = r#"{"tiles":[{"view":"kpis","size":"m"}]}"#;
        let (status, json) = request(engine.clone(), "PUT", "/api/dashboard?scope=demo", Some(body)).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["kind"], "dashboard");
        assert_eq!(json["data"]["source"], "demo");
        assert_eq!(json["data"]["tiles"][0]["view"], "kpis");

        let (status, json) = request(engine, "GET", "/api/dashboard?scope=demo", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["source"], "demo");
        assert_eq!(json["data"]["tiles"][0]["view"], "kpis");
    }

    #[tokio::test]
    async fn put_api_dashboard_needs_scope_and_refuses_an_unknown_metric_or_scope() {
        let engine = engine_with_dashboard_files();

        let (status, json) = request(engine.clone(), "PUT", "/api/dashboard", Some(r#"{"tiles":[]}"#)).await;
        assert_eq!(status, 400, "{json}");
        assert!(json["message"].as_str().unwrap_or_default().contains("scope"), "{json}");

        let bad = r#"{"tiles":[{"metric":"not_a_metric","size":"s"}]}"#;
        let (status, json) = request(engine.clone(), "PUT", "/api/dashboard?scope=demo", Some(bad)).await;
        assert_eq!(status, 400, "{json}");
        let message = json["message"].as_str().unwrap_or_default();
        assert!(message.contains("dashboard.tiles[0].metric"), "names the block and the tile: {json}");
        assert!(message.contains("not_a_metric"), "{json}");

        let ok = r#"{"tiles":[{"view":"kpis","size":"s"}]}"#;
        let (status, _) = request(engine, "PUT", "/api/dashboard?scope=nope", Some(ok)).await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn delete_api_dashboard_resets_and_is_refused_with_no_block_of_its_own() {
        let engine = engine_with_dashboard_files();

        // `demo` writes no `dashboard:` of its own yet.
        let (status, json) = request(engine.clone(), "DELETE", "/api/dashboard?scope=demo", None).await;
        assert_eq!(status, 400, "{json}");
        assert!(json["message"].as_str().unwrap_or_default().contains("defines no dashboard"), "{json}");

        let put = r#"{"tiles":[{"view":"kpis","size":"m"}]}"#;
        let (status, _) = request(engine.clone(), "PUT", "/api/dashboard?scope=demo", Some(put)).await;
        assert_eq!(status, 200);

        let (status, json) = request(engine.clone(), "DELETE", "/api/dashboard?scope=demo", None).await;
        assert_eq!(status, 200, "{json}");
        assert_eq!(json["data"]["source"], "company", "falls back to the root's own layout");

        let (status, _) = request(engine, "DELETE", "/api/dashboard", None).await;
        assert_eq!(status, 400, "scope is required on the write side, unlike the read");
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
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
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
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
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
