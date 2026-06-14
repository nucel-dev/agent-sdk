use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, Stream, StreamExt};

use crate::error::Result;
use crate::types::{AgentCost, AgentResponse, ExecutorType, MessageEvent};

/// Boxed event stream — what `query_stream()` returns.
pub type EventStream = Pin<Box<dyn Stream<Item = Result<MessageEvent>> + Send>>;

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

    /// Streaming variant of `query()`.
    ///
    /// Returns a stream of [`MessageEvent`]s as they arrive. The stream MUST
    /// terminate with either [`MessageEvent::ResultDone`] or
    /// [`MessageEvent::Error`].
    ///
    /// Default impl is back-compat: it calls `query()` and replays a single
    /// `TextChunk` + `ResultDone`. Providers override this to emit events
    /// live as they arrive on the wire.
    async fn query_stream(&self, prompt: &str) -> Result<EventStream> {
        let resp = self.query(prompt).await?;
        let events = vec![
            Ok(MessageEvent::TextChunk {
                text: resp.content.clone(),
            }),
            Ok(MessageEvent::ResultDone {
                cost: resp.cost.clone(),
                content: resp.content,
                is_error: false,
            }),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

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

    /// Send a follow-up prompt and stream events as they arrive.
    pub async fn query_stream(&self, prompt: &str) -> Result<EventStream> {
        self.inner.query_stream(prompt).await
    }

    /// Convenience: collect a stream into an `AgentResponse`.
    pub async fn collect_stream(mut stream: EventStream) -> Result<AgentResponse> {
        let mut content = String::new();
        let mut cost = AgentCost::default();
        let mut final_content: Option<String> = None;
        let mut tool_calls: Vec<crate::types::ToolCall> = Vec::new();
        let mut pending_tool: Option<crate::types::ToolCall> = None;
        while let Some(evt) = stream.next().await {
            match evt? {
                MessageEvent::TextChunk { text } => content.push_str(&text),
                MessageEvent::ToolUse { name, input, .. } => {
                    pending_tool = Some(crate::types::ToolCall {
                        name,
                        args: input,
                        result: None,
                    });
                }
                MessageEvent::ToolResult {
                    success, output, ..
                } => {
                    if let Some(mut t) = pending_tool.take() {
                        t.result = Some(crate::types::ToolResult { success, output });
                        tool_calls.push(t);
                    }
                }
                MessageEvent::ResultDone {
                    cost: c,
                    content: final_text,
                    is_error,
                } => {
                    cost = c;
                    if is_error {
                        return Err(crate::error::AgentError::Provider {
                            provider: "stream".into(),
                            message: final_text,
                        });
                    }
                    final_content = Some(final_text);
                    break;
                }
                MessageEvent::Error { message } => {
                    return Err(crate::error::AgentError::Provider {
                        provider: "stream".into(),
                        message,
                    });
                }
                MessageEvent::RateLimit { message } => {
                    return Err(crate::error::AgentError::RateLimited { message });
                }
                _ => {}
            }
        }
        if let Some(c) = final_content
            && !c.is_empty()
        {
            content = c;
        }
        Ok(AgentResponse {
            content,
            cost,
            confidence: None,
            requests_escalation: false,
            tool_calls,
        })
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
