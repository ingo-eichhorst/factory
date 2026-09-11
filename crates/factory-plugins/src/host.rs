//! The out-of-process plugin host: line-delimited JSON over the child's stdin
//! and stdout.
//!
//! A plugin is any program that reads one JSON object per line and answers with
//! one JSON object per line. No shared ABI, no Rust, no rebuild of the daemon --
//! which is the whole point of putting the seam here rather than in a dynamic
//! library.
//!
//!   in   {"id":1,"method":"agent.prompt","params":{...}}
//!   out  {"id":1,"result":{...}}
//!   out  {"id":1,"error":{"message":"..."}}
//!
//! A line the host cannot parse is logged and dropped, so a plugin that writes
//! debug output to stdout degrades to "slow" rather than "broken". stderr is
//! left to the plugin and inherited by the daemon's own.

use factory_core::error::{FactoryError, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{oneshot, Mutex};

use crate::manifest::LoadedPlugin;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>>>;

struct Running {
    child: Child,
    stdin: ChildStdin,
}

pub struct PluginProcess {
    plugin: LoadedPlugin,
    next_id: AtomicU64,
    pending: Pending,
    running: Mutex<Option<Running>>,
}

impl PluginProcess {
    pub fn new(plugin: LoadedPlugin) -> Self {
        Self {
            plugin,
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
            running: Mutex::new(None),
        }
    }

    pub fn name(&self) -> &str {
        &self.plugin.manifest.name
    }

    pub fn description(&self) -> String {
        if self.plugin.manifest.description.is_empty() {
            format!("{} plugin", self.plugin.manifest.name)
        } else {
            self.plugin.manifest.description.clone()
        }
    }

    pub fn source(&self) -> String {
        format!("plugin:{}", self.plugin.manifest_path.display())
    }

    fn fail(&self, message: impl std::fmt::Display) -> FactoryError {
        FactoryError::adapter(self.plugin.manifest.name.clone(), message.to_string())
    }

    /// Start the child if it is not up. Called on every request, so a plugin
    /// that crashes is restarted on the next call instead of staying dead.
    async fn ensure_started(&self) -> Result<()> {
        let mut guard = self.running.lock().await;
        if let Some(r) = guard.as_mut() {
            match r.child.try_wait() {
                Ok(None) => return Ok(()),
                // Exited or unqueryable: drop it and start again below.
                _ => {
                    *guard = None;
                }
            }
        }

        let argv = self.plugin.argv();
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(&self.plugin.dir)
            .envs(&self.plugin.manifest.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| self.fail(format!("could not start `{}`: {e}", argv.join(" "))))?;
        let stdin = child.stdin.take().ok_or_else(|| self.fail("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| self.fail("no stdout"))?;

        let pending = self.pending.clone();
        let name = self.plugin.manifest.name.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    tracing::debug!(plugin = %name, line = %line, "ignoring unparseable plugin output");
                    continue;
                };
                let Some(id) = msg.get("id").and_then(Value::as_u64) else {
                    tracing::debug!(plugin = %name, "plugin message without an id, dropped");
                    continue;
                };
                let reply = if let Some(err) = msg.get("error") {
                    Err(err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or(&err.to_string())
                        .to_string())
                } else {
                    Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                };
                if let Some(tx) = pending.lock().await.remove(&id) {
                    let _ = tx.send(reply);
                }
            }
            // The child is gone: wake everyone still waiting rather than let
            // them sit until their timeout.
            let mut p = pending.lock().await;
            for (_, tx) in p.drain() {
                let _ = tx.send(Err(format!("{name} exited while a call was in flight")));
            }
        });

        *guard = Some(Running { child, stdin });
        Ok(())
    }

    /// One request, one reply.
    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let raw = self.call_raw(method, params).await?;
        serde_json::from_value(raw).map_err(|e| {
            self.fail(format!("reply to {method} did not match what Factory expects: {e}"))
        })
    }

    pub async fn call_raw(&self, method: &str, params: Value) -> Result<Value> {
        self.ensure_started().await?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let line = format!("{}\n", json!({ "id": id, "method": method, "params": params }));
        {
            let mut guard = self.running.lock().await;
            let running = guard.as_mut().ok_or_else(|| self.fail("not running"))?;
            if let Err(e) = running.stdin.write_all(line.as_bytes()).await {
                self.pending.lock().await.remove(&id);
                *guard = None;
                return Err(self.fail(format!("writing to plugin: {e}")));
            }
            if let Err(e) = running.stdin.flush().await {
                self.pending.lock().await.remove(&id);
                *guard = None;
                return Err(self.fail(format!("flushing to plugin: {e}")));
            }
        }

        let timeout = Duration::from_secs(self.plugin.manifest.timeout_seconds.max(1));
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => Err(self.fail(message)),
            Ok(Err(_)) => Err(self.fail("plugin closed the call")),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(self.fail(format!("no reply to {method} within {}s", timeout.as_secs())))
            }
        }
    }

    /// Ask the plugin to confirm it is alive and speaks the protocol.
    pub async fn describe(&self) -> Result<Value> {
        self.call_raw("describe", json!({})).await
    }

    pub async fn shutdown(&self) {
        let mut guard = self.running.lock().await;
        if let Some(mut r) = guard.take() {
            let _ = r.stdin.shutdown().await;
            let _ = r.child.kill().await;
        }
    }
}
