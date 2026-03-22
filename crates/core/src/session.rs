use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;
use crate::types::{AgentCost, AgentResponse, ExecutorType};

/// Metadata about a session (persistable, cloneable).
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub session_id: String,
    pub executor_type: ExecutorType,
    pub working_dir: PathBuf,
    pub created_at: DateTime<Utc>,
    pub model: Option<String>,
}

/// Session implementation trait.
/// Providers implement this to control query/cost/close behavior.
#[async_trait]
pub trait SessionImpl: Send + Sync {
    async fn query(&self, prompt: &str) -> Result<AgentResponse>;
    async fn total_cost(&self) -> Result<AgentCost>;
    async fn close(&self) -> Result<()>;
}

/// Active agent session.
///
/// Returned by `AgentExecutor::spawn()` or `resume()`.
/// Use `query()` for follow-up prompts, `total_cost()` for spend tracking,
/// and `close()` to clean up.
pub struct AgentSession {
    /// Unique session identifier.
    pub session_id: String,
    /// Which executor created this session.
    pub executor_type: ExecutorType,
    /// Working directory.
    pub working_dir: PathBuf,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Model being used.
    pub model: Option<String>,

    pub(crate) inner: Arc<dyn SessionImpl>,
}

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSession")
            .field("session_id", &self.session_id)
            .field("executor_type", &self.executor_type)
            .field("working_dir", &self.working_dir)
            .field("created_at", &self.created_at)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl AgentSession {
    /// Create a new session with an inner implementation.
    pub fn new(
        session_id: impl Into<String>,
        executor_type: ExecutorType,
        working_dir: impl Into<PathBuf>,
        model: Option<String>,
        inner: Arc<dyn SessionImpl>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            executor_type,
            working_dir: working_dir.into(),
            created_at: Utc::now(),
            model,
            inner,
        }
    }

    /// Send a follow-up prompt to the agent.
    pub async fn query(&self, prompt: &str) -> Result<AgentResponse> {
        self.inner.query(prompt).await
    }

    /// Get the accumulated cost of this session.
    pub async fn total_cost(&self) -> Result<AgentCost> {
        self.inner.total_cost().await
    }

    /// Close the session and release resources.
    pub async fn close(self) -> Result<()> {
        self.inner.close().await
    }

    /// Session metadata snapshot.
    pub fn metadata(&self) -> SessionMetadata {
        SessionMetadata {
            session_id: self.session_id.clone(),
            executor_type: self.executor_type,
            working_dir: self.working_dir.clone(),
            created_at: self.created_at,
            model: self.model.clone(),
        }
    }
}
