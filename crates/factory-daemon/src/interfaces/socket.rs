//! The `cli` interface: a unix socket carrying one JSON request per line.
//!
//! It is the interface the `factory` binary speaks, and the one a dispatched
//! agent reports back on -- which is why it is a socket in `.factory/` with
//! filesystem permissions, and not a port.

use async_trait::async_trait;
use factory_core::adapter::interface::{Interface, InterfaceContext};
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::{Payload, Request, Response};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::engine::Engine;

/// Where the socket goes is the instance's decision, not this adapter's: the
/// CLI has to compute the same path without asking anyone.
pub struct SocketInterface {
    pub default_path: PathBuf,
}

impl SocketInterface {
    pub fn new(default_path: PathBuf) -> Self {
        Self { default_path }
    }

    pub fn socket_path(&self, ctx: &InterfaceContext) -> PathBuf {
        match ctx.config.string("socket") {
            Some(p) => ctx.resolve(p),
            None => self.default_path.clone(),
        }
    }
}

#[async_trait]
impl Interface<Engine> for SocketInterface {
    fn name(&self) -> &str {
        "cli"
    }

    fn description(&self) -> String {
        "unix socket, one JSON request per line".into()
    }

    async fn serve(
        self: Arc<Self>,
        engine: Arc<Engine>,
        ctx: InterfaceContext,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        let path = self.socket_path(&ctx);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FactoryError::Other(e.into()))?;
        }
        // A socket left behind by a daemon that did not shut down cleanly would
        // make bind fail; a live one would have been caught before we got here.
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }

        let listener = UnixListener::bind(&path).map_err(|e| {
            FactoryError::Other(anyhow::anyhow!("binding {}: {e}", path.display()))
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Only the owner talks to their own control plane.
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        tracing::info!(socket = %path.display(), "cli interface listening");

        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _)) => {
                            let engine = engine.clone();
                            tokio::spawn(async move {
                                if let Err(e) = serve_connection(engine, stream).await {
                                    tracing::debug!("cli connection ended: {e}");
                                }
                            });
                        }
                        Err(e) => tracing::warn!("accept failed: {e}"),
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        let _ = std::fs::remove_file(&path);
                                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn serve_connection(engine: Arc<Engine>, stream: UnixStream) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                let response = Response::error("bad_request", format!("unreadable request: {e}"));
                write_half
                    .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
                    .await?;
                continue;
            }
        };

        // Subscribe takes over the connection: from here it is one event per
        // line until the client goes away.
        if matches!(request, Request::Subscribe) {
            let ack = Response::ok(Payload::Ok);
            write_half
                .write_all(format!("{}\n", serde_json::to_string(&ack)?).as_bytes())
                .await?;
            let mut events = engine.bus.subscribe();
            loop {
                match events.recv().await {
                    Ok(event) => {
                        let msg = Response::ok(Payload::Event { event });
                        if write_half
                            .write_all(format!("{}\n", serde_json::to_string(&msg)?).as_bytes())
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                    // A slow reader loses the oldest events rather than the
                    // daemon; say so and keep going.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        let msg = Response::error("lagged", format!("{n} events were dropped"));
                        if write_half
                            .write_all(format!("{}\n", serde_json::to_string(&msg)?).as_bytes())
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                    Err(_) => return Ok(()),
                }
            }
        }

        let response = engine.handle(request).await;
        write_half
            .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
            .await?;
    }
    Ok(())
}
