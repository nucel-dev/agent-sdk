//! Nucel Agent SDK — Unified
//!
//! One import for all providers. Swap coding agents via configuration.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};
//! use std::path::Path;
//!
//! # async fn example() -> nucel_agent_sdk::Result<()> {
//! let executor = ClaudeCodeExecutor::new();
//!
//! let session = executor.spawn(
//!     Path::new("/my/repo"),
//!     "Fix the failing tests",
//!     &SpawnConfig {
//!         model: Some("claude-opus-4-6".into()),
//!         budget_usd: Some(5.0),
//!         ..Default::default()
//!     },
//! ).await?;
//!
//! println!("Response: {}", session.query("Check if CI passes now").await?.content);
//! session.close().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Provider Selection
//!
//! ```rust,no_run
//! use nucel_agent_sdk::*;
//!
//! # fn example() {
//! // Via config string (like agent-operator does)
//! let executor = build_executor("claude-code", None);
//! let executor = build_executor("codex", Some("sk-...".into()));
//! let executor = build_executor("opencode", Some("http://localhost:4096".into()));
//! # }
//! ```

// Re-export core types.
pub use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse, AgentSession,
    AvailabilityStatus, ExecutorType, PermissionMode, Result, SessionMetadata, SpawnConfig,
};

// Re-export provider executors.
pub use nucel_agent_claude_code::ClaudeCodeExecutor;
pub use nucel_agent_codex::CodexExecutor;
pub use nucel_agent_opencode::OpencodeExecutor;

/// Build an executor from a config string (like `providers.agent = "claude-code"`).
///
/// - `"claude-code"` → `ClaudeCodeExecutor`
/// - `"codex"` → `CodexExecutor`
/// - `"opencode"` → `OpencodeExecutor` (second arg is base URL)
///
/// Returns `None` for unknown providers.
pub fn build_executor(
    provider: &str,
    api_key_or_url: Option<String>,
) -> Option<Box<dyn AgentExecutor>> {
    match provider {
        "claude-code" | "claude_code" | "claudecode" => Some(Box::new(ClaudeCodeExecutor::new())),
        "codex" => Some(Box::new(CodexExecutor::new())),
        "opencode" => {
            let mut exec = OpencodeExecutor::new();
            if let Some(url) = api_key_or_url {
                exec = OpencodeExecutor::with_base_url(url);
            }
            Some(Box::new(exec))
        }
        _ => None,
    }
}

/// List all available provider names.
pub fn available_providers() -> &'static [&'static str] {
    &["claude-code", "codex", "opencode"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_claude_code_executor() {
        let exec = build_executor("claude-code", None).unwrap();
        assert_eq!(exec.executor_type(), ExecutorType::ClaudeCode);
    }

    #[test]
    fn build_codex_executor() {
        let exec = build_executor("codex", None).unwrap();
        assert_eq!(exec.executor_type(), ExecutorType::Codex);
    }

    #[test]
    fn build_opencode_executor() {
        let exec = build_executor("opencode", None).unwrap();
        assert_eq!(exec.executor_type(), ExecutorType::OpenCode);
    }

    #[test]
    fn build_opencode_with_url() {
        let exec = build_executor("opencode", Some("http://my-server:8080".into())).unwrap();
        assert_eq!(exec.executor_type(), ExecutorType::OpenCode);
    }

    #[test]
    fn unknown_provider_returns_none() {
        assert!(build_executor("gpt-4", None).is_none());
    }

    #[test]
    fn claude_code_aliases_work() {
        assert!(build_executor("claude_code", None).is_some());
        assert!(build_executor("claudecode", None).is_some());
    }

    #[test]
    fn available_providers_list() {
        let providers = available_providers();
        assert_eq!(providers.len(), 3);
        assert!(providers.contains(&"claude-code"));
        assert!(providers.contains(&"codex"));
        assert!(providers.contains(&"opencode"));
    }
}
