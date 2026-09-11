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
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::{Payload, Request, Response};
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
            .map(|a| a.to_string())
            .unwrap_or(bind.clone());
        tracing::info!(url = %format!("http://{addr}"), "http interface listening");

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

fn router(engine: Arc<Engine>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_upgrade))
        .route("/api/status", get(status))
        .route("/api/adapters", get(adapters))
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
        .with_state(engine)
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        crate::ui::INDEX_HTML,
    )
}

/// Every route funnels through here, so REST and the socket cannot drift apart.
async fn run(engine: &Arc<Engine>, request: Request) -> AxumResponse {
    let response = engine.handle(request).await;
    let code = match &response {
        Response::Ok { .. } => StatusCode::OK,
        Response::Error { code, .. } => match code.as_str() {
            "task_not_found" | "no_such_adapter" | "no_such_scope" => StatusCode::NOT_FOUND,
            "bad_request" => StatusCode::BAD_REQUEST,
            "denied" => StatusCode::FORBIDDEN,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        },
    };
    (code, Json(response)).into_response()
}

async fn rpc(State(engine): State<Arc<Engine>>, Json(req): Json<Request>) -> AxumResponse {
    run(&engine, req).await
}

async fn status(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Status).await
}

async fn adapters(State(engine): State<Arc<Engine>>) -> AxumResponse {
    run(&engine, Request::Adapters).await
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

async fn ws_upgrade(State(engine): State<Arc<Engine>>, ws: WebSocketUpgrade) -> AxumResponse {
    ws.on_upgrade(move |socket| ws_stream(engine, socket))
}

/// Live events, plus a snapshot first so a page that connects late is not
/// looking at an empty table until something happens.
async fn ws_stream(engine: Arc<Engine>, socket: WebSocket) {
    let (mut tx, mut rx) = socket.split();
    let mut events = engine.bus.subscribe();

    let snapshot = engine.handle(Request::TaskList(TaskFilter::default())).await;
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
                        let snapshot = engine.handle(Request::TaskList(TaskFilter::default())).await;
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
