//! Outside-stack wire protocol, observer stream and the unchanged Interface
//! seam. Engine::handle(Envelope) and access.rs remain in the daemon, the one
//! entry point and authorization check. No concrete HTTP/socket implementation
//! or level service is hidden here, and levels cannot depend on this package.
pub mod event;
pub mod interface;
pub mod protocol;

pub use event::{Event, EventBus};
pub use interface::{Interface, InterfaceContext};
