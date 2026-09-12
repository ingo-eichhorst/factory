//! The `http` interface: REST over the same request envelope, a WebSocket for
//! the event stream, and the web UI on `/`.
//!
//! Bound to loopback unless the config says otherwise. Nothing here decides
//! anything -- every route builds a `Request` and hands it to the engine.

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response as AxumResponse};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use factory_core::adapter::interface::{Interface, InterfaceContext};
use factory_core::config::ScopeAgent;
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::{Envelope, Payload, ProductionBin, Request, Response};
use factory_core::task::{NewTask, TaskFilter, TaskPatch, TaskReport};
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

        let listener = tokio::net::TcpListener::bind(&bind).await.map_err(|e| {
            FactoryError::Other(anyhow::anyhow!("binding {bind}: {e}"))
        })?;
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
        .route("/api/agents", get(agents))
        .route("/api/occupancy", get(occupancy))
        .route("/api/production", get(production))
        .route("/api/site", get(site_footprint))
        // The id of a standing agent is `<scope>/<name>`, which has a slash in
        // it, so these take it in the body rather than the path.
        .route("/api/agents/start", post(agent_start))
        .route("/api/agents/configure", post(agent_configure))
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
        .route("/api/tasks/{id}/report", post(report_task))
        .route("/api/tasks/{id}/entries", get(task_entries))
        .route("/api/tasks/{id}/output", get(task_output))
        .route("/api/tasks/{id}/runs", get(task_runs))
        .route("/api/runs/{id}", get(get_run))
        .route("/api/runs/{id}/entries", get(run_entries))
        .route("/api/runs/{id}/output", get(run_output))
        .route("/api/runs/{id}/input", post(run_input))
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

async fn agents(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Agents).await
}

#[derive(serde::Deserialize)]
struct Window {
    minutes: Option<u32>,
}

async fn occupancy(
    State(engine): State<Arc<Engine>>,
    Query(window): Query<Window>,
) -> AxumResponse {
    run(
        &engine,
        Request::Occupancy {
            minutes: window.minutes,
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

async fn update_task(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(patch): Json<TaskPatch>,
) -> AxumResponse {
    run(&engine, Request::TaskUpdate { id, patch }).await
}

async fn delete_task(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::TaskDelete { id }).await
}

async fn run_task(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::TaskRun { id }).await
}

async fn cancel_task(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> AxumResponse {
    run(&engine, Request::TaskCancel { id }).await
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
}

async fn task_entries(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(q): Query<Lines>,
) -> AxumResponse {
    run(&engine, Request::TaskEntries { id, limit: q.limit }).await
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
