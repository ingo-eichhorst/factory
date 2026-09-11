use crate::config::InterfaceConfig;
use crate::error::Result;
use std::path::PathBuf;
use std::sync::Arc;

/// What an interface adapter is handed when it is mounted.
pub struct InterfaceContext {
    pub config: InterfaceConfig,
    /// The instance root, for resolving relative paths in settings.
    pub root: PathBuf,
    pub factory_dir: PathBuf,
}

impl InterfaceContext {
    pub fn resolve(&self, p: impl AsRef<std::path::Path>) -> PathBuf {
        let p = p.as_ref();
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        }
    }
}

/// Adapter seam 4: how the outside reaches the daemon. `cli` and `http` are
/// two mounts of the same API; mcp or anything else is another one.
///
/// `Api` is left generic so this trait lives in core without core depending on
/// the daemon; the daemon instantiates it with its own engine handle.
#[async_trait::async_trait]
pub trait Interface<Api>: Send + Sync
where
    Api: Send + Sync + 'static,
{
    fn name(&self) -> &str;

    fn description(&self) -> String {
        format!("{} interface", self.name())
    }

    /// Serve until `shutdown` resolves. Returning `Ok` before then is treated
    /// as the interface having stopped on its own.
    async fn serve(
        self: Arc<Self>,
        api: Arc<Api>,
        ctx: InterfaceContext,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()>;
}
