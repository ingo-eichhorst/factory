//! A thin client for the daemon's unix socket. One request per line, one
//! response per line.

use anyhow::{anyhow, Context, Result};
use factory_core::config::Factory;
use factory_core::protocol::{Envelope, Payload, Request, Response};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub struct Client {
    pub socket: PathBuf,
    /// Which agent is calling. Absent means the person at the keyboard.
    pub token: Option<String>,
}

impl Client {
    /// `--socket`, then `FACTORY_SOCKET`, then the nearest instance's own.
    pub fn locate(
        explicit: Option<PathBuf>,
        root: Option<PathBuf>,
        token: Option<String>,
    ) -> Result<Self> {
        // Factory sets FACTORY_TOKEN in every session it opens, so an agent
        // identifies itself without being told to.
        let token = token
            .or_else(|| std::env::var("FACTORY_TOKEN").ok())
            .or_else(|| std::env::var("FACTORY_TASK_TOKEN").ok())
            .filter(|t| !t.is_empty());
        if let Some(p) = explicit {
            return Ok(Self { socket: p, token });
        }
        if let Ok(p) = std::env::var("FACTORY_SOCKET") {
            if !p.is_empty() {
                return Ok(Self {
                    socket: PathBuf::from(p),
                    token,
                });
            }
        }
        let root = match root {
            Some(r) => r,
            None => Factory::discover(&std::env::current_dir()?)?.ok_or_else(|| {
                anyhow!("no .factory/ here or in any parent, and no --socket given")
            })?,
        };
        Ok(Self {
            socket: Factory::load(&root)?.socket_path(),
            token,
        })
    }

    async fn connect(&self) -> Result<UnixStream> {
        UnixStream::connect(&self.socket).await.with_context(|| {
            format!(
                "no daemon on {}. Start one with `factory-daemon run`.",
                self.socket.display()
            )
        })
    }

    pub async fn send(&self, request: Request) -> Result<Payload> {
        let stream = self.connect().await?;
        let (read_half, mut write_half) = stream.into_split();
        let envelope = Envelope {
            request,
            token: self.token.clone(),
        };
        let line = format!("{}\n", serde_json::to_string(&envelope)?);
        write_half.write_all(line.as_bytes()).await?;
        write_half.flush().await?;

        let mut lines = BufReader::new(read_half).lines();
        let reply = lines
            .next_line()
            .await?
            .ok_or_else(|| anyhow!("the daemon closed the connection without answering"))?;
        match serde_json::from_str::<Response>(&reply)? {
            Response::Ok { data } => Ok(data),
            Response::Error { code, message } => Err(anyhow!("{message} [{code}]")),
        }
    }

    /// Stream events until the connection drops or `on_event` says to stop.
    pub async fn subscribe<F>(&self, mut on_event: F) -> Result<()>
    where
        F: FnMut(Response) -> bool,
    {
        let stream = self.connect().await?;
        let (read_half, mut write_half) = stream.into_split();
        let line = format!(
            "{}\n",
            serde_json::to_string(&Envelope {
                request: Request::Subscribe,
                token: self.token.clone(),
            })?
        );
        write_half.write_all(line.as_bytes()).await?;
        write_half.flush().await?;

        let mut lines = BufReader::new(read_half).lines();
        // The first line is the acknowledgement, not an event.
        let _ = lines.next_line().await?;
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let msg: Response = serde_json::from_str(&line)?;
            if !on_event(msg) {
                break;
            }
        }
        Ok(())
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }
}
