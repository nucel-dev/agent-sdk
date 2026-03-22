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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Agent must prompt user for each operation.
    Prompt,
    /// Auto-approve file edits only.
    AcceptEdits,
    /// Bypass all permission checks (sandbox mode).
    BypassPermissions,
    /// Reject all operations (dry run).
    RejectAll,
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::Prompt
    }
}

/// Cost breakdown for a single query or session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentCost {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_usd: f64,
}

impl std::ops::Add for AgentCost {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self {
            input_tokens: self.input_tokens + rhs.input_tokens,
            output_tokens: self.output_tokens + rhs.output_tokens,
            total_usd: self.total_usd + rhs.total_usd,
        }
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
