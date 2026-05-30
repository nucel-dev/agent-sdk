use std::path::Path;

use async_trait::async_trait;

use crate::error::Result;
use crate::retry::RetryPolicy;
use crate::session::AgentSession;
use crate::types::{CachePoint, ExecutorType, HookConfig, PermissionMode};

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
    /// Supports event-level streaming via `query_stream()`.
    pub streaming: bool,
    /// Supports user-defined hooks (pre/post tool, on stop).
    pub hooks: bool,
    /// Supports Anthropic-style prompt cache breakpoints.
    pub prompt_caching: bool,
    /// Supports extended-thinking budget.
    pub extended_thinking: bool,
}

impl Default for AgentCapabilities {
    fn default() -> Self {
        Self {
            session_resume: false,
            token_usage: false,
            mcp_support: false,
            autonomous_mode: false,
            structured_output: false,
            streaming: false,
            hooks: false,
            prompt_caching: false,
            extended_thinking: false,
        }
    }
}

/// Runtime availability of the provider.
#[derive(Debug, Clone)]
pub struct AvailabilityStatus {
    /// Whether the provider's runtime dependency is usable right now.
    pub available: bool,
    /// Human-readable explanation. Always `Some` when `available` is `false`;
    /// may also carry an informational note when `available` is `true`.
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
    /// Maximum autonomous turns before returning (0 or None = provider default).
    pub max_turns: Option<u32>,
    /// Hook handlers (Claude Code today; no-op on other providers).
    pub hook_config: Option<HookConfig>,
    /// Prompt-cache breakpoints (Claude Code today; no-op elsewhere).
    pub cache_breakpoints: Vec<CachePoint>,
    /// Extended-thinking budget in tokens (Claude Code's
    /// `--thinking-budget-tokens`). Ignored by other providers.
    pub thinking_budget: Option<u32>,
    /// Retry policy for *transient*, pre-side-effect request failures
    /// (connection errors, timeouts, `429`/`502`/`503`/`504` before any
    /// response body is consumed). Honored by the network providers (Vertex,
    /// OpenCode); ignored by subprocess providers that delegate retry to their
    /// CLI. Additive: defaults to [`RetryPolicy::default`], so existing
    /// `..Default::default()` construction is unaffected.
    ///
    /// (Equivalent to `#[serde(default)]` were these structs ever
    /// Serde-derived — `RetryPolicy: Default` makes the field optional.)
    pub retry_policy: RetryPolicy,
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
    /// Retry policy for *transient*, pre-side-effect request failures applied
    /// uniformly by the network providers. Additive: defaults to
    /// [`RetryPolicy::default`], so existing `..Default::default()`
    /// construction is unaffected.
    pub retry_policy: RetryPolicy,
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
