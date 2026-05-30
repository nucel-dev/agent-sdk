use serde::{Deserialize, Serialize};

/// Supported executor backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutorType {
    ClaudeCode,
    Codex,
    OpenCode,
}

impl std::fmt::Display for ExecutorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClaudeCode => write!(f, "claude-code"),
            Self::Codex => write!(f, "codex"),
            Self::OpenCode => write!(f, "opencode"),
        }
    }
}

/// Permission mode for file system and command operations.
///
/// This enum is `#[non_exhaustive]` — new variants may be added in minor
/// versions. Downstream matches MUST include a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PermissionMode {
    /// Agent must prompt user for each operation.
    Prompt,
    /// Auto-approve file edits only (still prompts for bash, etc.).
    AcceptEdits,
    /// Bypass all permission checks (sandbox mode).
    BypassPermissions,
    /// Plan / dry-run mode — analysis only, no edits/execution.
    /// Note: historically misnamed; Claude Code maps this to `plan` mode (still reads).
    /// For a true deny-without-prompting mode, use [`PermissionMode::DontAsk`].
    RejectAll,
    /// Deny instead of prompting — provider-side equivalent of "always say no".
    /// Maps to Claude Code's `dontAsk`.
    DontAsk,
    /// Provider's automatic / default policy — let the provider pick a sensible
    /// default (e.g. workspace-write sandbox for Codex, default permission mode
    /// for Claude Code). Useful when you don't want to be opinionated.
    Auto,
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::Prompt
    }
}

/// Cost breakdown for a single query or session.
///
/// `cache_read_tokens` and `cache_creation_tokens` were added in 0.2.0 to
/// surface Anthropic prompt-caching effects. They default to 0 for providers
/// that don't report cache stats (Codex, OpenCode today).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentCost {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Tokens served from prompt cache (cheap reads).
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Tokens written to the prompt cache (one-off cost).
    #[serde(default)]
    pub cache_creation_tokens: u64,
    pub total_usd: f64,
}

impl std::ops::Add for AgentCost {
    type Output = Self;

    /// Sum two cost breakdowns. Token counts use saturating addition so an
    /// accumulator that runs for a very long session can never panic on
    /// overflow in a debug build (it pins at `u64::MAX` instead).
    fn add(self, rhs: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_add(rhs.input_tokens),
            output_tokens: self.output_tokens.saturating_add(rhs.output_tokens),
            cache_read_tokens: self.cache_read_tokens.saturating_add(rhs.cache_read_tokens),
            cache_creation_tokens: self
                .cache_creation_tokens
                .saturating_add(rhs.cache_creation_tokens),
            total_usd: self.total_usd + rhs.total_usd,
        }
    }
}

impl std::ops::AddAssign for AgentCost {
    /// In-place accumulation — the idiom every provider's session uses to fold
    /// each turn's cost into the running total. Mirrors [`Add`] (saturating
    /// token math).
    fn add_assign(&mut self, rhs: Self) {
        *self = self.clone() + rhs;
    }
}

/// Response from a coding agent query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// Text response from the agent.
    pub content: String,
    /// Cost of this specific query.
    pub cost: AgentCost,
    /// Agent's self-reported confidence (0.0 to 1.0).
    pub confidence: Option<f64>,
    /// Whether the agent is requesting human escalation.
    pub requests_escalation: bool,
    /// Tool calls made during this query.
    pub tool_calls: Vec<ToolCall>,
}

/// A tool call made by the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub args: serde_json::Value,
    pub result: Option<ToolResult>,
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
}

impl Default for AgentResponse {
    fn default() -> Self {
        Self {
            content: String::new(),
            cost: AgentCost::default(),
            confidence: None,
            requests_escalation: false,
            tool_calls: vec![],
        }
    }
}

/// Event emitted by a streaming query.
///
/// `#[non_exhaustive]` — new variants may be added in minor versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MessageEvent {
    /// A chunk of assistant text content.
    TextChunk { text: String },
    /// The model started a tool call.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A tool finished executing.
    ToolResult {
        tool_use_id: String,
        success: bool,
        output: String,
    },
    /// Provider is retrying an API request.
    ApiRetry { attempt: u32, message: String },
    /// Upstream rate limit notification.
    RateLimit { message: String },
    /// Extended-thinking content (Claude only).
    Thinking { text: String },
    /// Terminal event: the query is done. Contains final cost.
    ResultDone {
        cost: AgentCost,
        content: String,
        is_error: bool,
    },
    /// Terminal event: the query failed.
    Error { message: String },
}

/// A single cache-breakpoint marker for prompt caching.
///
/// Maps to Anthropic's `cache_control` blocks. Providers that don't support
/// prompt caching (Codex, OpenCode) ignore these.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CachePoint {
    /// Logical name (e.g. `"system"`, `"tools"`, `"history"`).
    pub label: String,
    /// Cache type — Anthropic accepts `"ephemeral"` today.
    #[serde(default = "default_cache_type")]
    pub cache_type: String,
}

fn default_cache_type() -> String {
    "ephemeral".to_string()
}

impl CachePoint {
    pub fn ephemeral(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            cache_type: "ephemeral".to_string(),
        }
    }
}

/// Hook configuration for tool/lifecycle interception.
///
/// Currently honored only by Claude Code (serialized into `--settings <json>`).
/// Codex/OpenCode treat all hooks as no-ops with a debug-level log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookConfig {
    /// Run before every tool use.
    pub pre_tool_use: Option<HookHandler>,
    /// Run after every tool use.
    pub post_tool_use: Option<HookHandler>,
    /// Run when the session terminates.
    pub on_stop: Option<HookHandler>,
    /// Run when the user submits a prompt.
    pub user_prompt_submit: Option<HookHandler>,
}

/// A single hook handler — a shell command the provider will execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookHandler {
    /// The command to execute. Receives hook context on stdin as JSON.
    pub command: String,
    /// Optional matcher (tool name regex, etc.). Provider-specific.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// Timeout in seconds (None = provider default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
}

impl HookHandler {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            matcher: None,
            timeout_seconds: None,
        }
    }

    pub fn with_matcher(mut self, matcher: impl Into<String>) -> Self {
        self.matcher = Some(matcher.into());
        self
    }

    pub fn with_timeout(mut self, secs: u32) -> Self {
        self.timeout_seconds = Some(secs);
        self
    }
}
