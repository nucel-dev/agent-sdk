use thiserror::Error;

pub type Result<T> = std::result::Result<T, AgentError>;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("provider error ({provider}): {message}")]
    Provider { provider: String, message: String },

    #[error("budget exceeded: spent ${spent:.2} of ${limit:.2}")]
    BudgetExceeded { limit: f64, spent: f64 },

    #[error("session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("agent CLI not found: {cli_name}")]
    CliNotFound { cli_name: String },

    #[error("configuration error: {0}")]
    Config(String),

    #[error("timeout after {seconds}s")]
    Timeout { seconds: u64 },

    #[error("agent requested escalation")]
    EscalationRequested,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
