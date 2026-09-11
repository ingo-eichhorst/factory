use thiserror::Error;

/// Every adapter fails through this type, so the daemon can tell an adapter's
/// fault from its own without downcasting.
#[derive(Debug, Error)]
pub enum FactoryError {
    #[error("no such task: {0}")]
    TaskNotFound(String),

    #[error("no {kind} adapter named {name:?}; registered: {available}")]
    NoSuchAdapter {
        kind: &'static str,
        name: String,
        available: String,
    },

    #[error("no scope named {0:?}")]
    NoSuchScope(String),

    #[error("{adapter}: {message}")]
    Adapter { adapter: String, message: String },

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("not permitted: {0}")]
    Denied(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl FactoryError {
    pub fn adapter(adapter: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Adapter {
            adapter: adapter.into(),
            message: message.into(),
        }
    }

    /// A short, stable string the wire protocol can carry instead of the prose.
    pub fn code(&self) -> &'static str {
        match self {
            Self::TaskNotFound(_) => "task_not_found",
            Self::NoSuchAdapter { .. } => "no_such_adapter",
            Self::NoSuchScope(_) => "no_such_scope",
            Self::Adapter { .. } => "adapter_failed",
            Self::BadRequest(_) => "bad_request",
            Self::Denied(_) => "denied",
            Self::Other(_) => "internal",
        }
    }
}

pub type Result<T, E = FactoryError> = std::result::Result<T, E>;
