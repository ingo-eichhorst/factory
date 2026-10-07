//! The command port from L2 to L1 (#193 phase 6, S8c): what Environment asks of Infrastructure.
//!
//! A level may command only the level directly below it, so the host's power policy is
//! reached by L4 -> L3 -> L2 -> L1, each asking the next. The command carries a run id and
//! returns nothing: whether the host was told to stay awake is L1's own business.
//!
//! A port at any other level cannot implement it:
//!
//! ```compile_fail
//! use factory_infrastructure::host_commands::HostCommands;
//! use factory_kernel::{CommandPort, L2};
//! struct Wrong;
//! impl CommandPort for Wrong { type Level = L2; }
//! #[async_trait::async_trait]
//! impl HostCommands for Wrong {
//!     async fn keep_awake(&self, _: &str) {}
//!     async fn allow_sleep(&self, _: &str) {}
//! }
//! ```
use factory_kernel::{CommandPort, L1};

#[async_trait::async_trait]
pub trait HostCommands: CommandPort<Level = L1> + Send + Sync {
    /// A run is starting: keep the host awake for it (`#61`). Refcounted by run id.
    async fn keep_awake(&self, run_id: &str);
    /// A run just ended, however it ended: let go of its share. A run id never given to
    /// `keep_awake` is a no-op, so every terminal path may call it unconditionally.
    async fn allow_sleep(&self, run_id: &str);
}
