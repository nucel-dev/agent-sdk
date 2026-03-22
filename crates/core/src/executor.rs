use std::path::Path;

use async_trait::async_trait;

use crate::error::Result;
use crate::session::AgentSession;
use crate::types::{ExecutorType, PermissionMode};

/// Capability flags for a provider implementation.
#[derive(Debug, Clone)]
pub struct AgentCapabilities {
    /// Can resume/fork from an existing session.
    pub session_resume: bool,
    /// Exposes token usage information.
    pub token_usage: bool,
    /// Supports MCP tool integration.
    pub mcp_support: bool,
    /// Can run autonomously (bash, file ops).
    pub autonomous_mode: bool,
    /// Supports structured output via JSON Schema.
    pub structured_output: bool,
}

/// Runtime availability of the provider.
#[derive(Debug, Clone)]
pub struct AvailabilityStatus {
    pub available: bool,
    pub reason: Option<String>,
}

/// Configuration for spawning a new session.
#[derive(Debug, Clone, Default)]
pub struct SpawnConfig {
    /// Model to use (e.g. "claude-opus-4-6", "gpt-5-codex").
    pub model: Option<String>,
    /// Maximum tokens for responses.
    pub max_tokens: Option<u32>,
    /// Budget limit in USD for this session.
    pub budget_usd: Option<f64>,
    /// Permission mode for file/command operations.
    pub permission_mode: Option<PermissionMode>,
    /// Extra environment variables for the subprocess.
    pub env: Vec<(String, String)>,
    /// System prompt override.
    pub system_prompt: Option<String>,
    /// Reasoning effort level (provider-specific).
    pub reasoning: Option<String>,
}

/// Configuration for the executor itself (not per-session).
#[derive(Debug, Clone, Default)]
pub struct ExecutorConfig {
    /// API key for authentication.
    pub api_key: Option<String>,
    /// Base URL override (for OpenCode server, etc.).
    pub base_url: Option<String>,
    /// Working directory for CLI discovery.
    pub working_dir: Option<String>,
}

/// Core trait — every provider implements this.
///
/// This is intentionally narrow: spawn, resume, capabilities, availability.
/// Provider-specific features stay in the provider crate.
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    /// Which provider this executor implements.
    fn executor_type(&self) -> ExecutorType;

    /// Create a new session and send the first prompt.
    async fn spawn(
        &self,
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession>;

    /// Resume an existing session with a follow-up prompt.
    async fn resume(
        &self,
        working_dir: &Path,
        session_id: &str,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession>;

    /// Static capability flags.
    fn capabilities(&self) -> AgentCapabilities;

    /// Check if the runtime dependency (CLI, server) is available.
    fn availability(&self) -> AvailabilityStatus;
}
