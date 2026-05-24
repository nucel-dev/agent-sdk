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
//!
//! # Runnable examples
//!
//! Basics:
//!
//! - [`examples/claude_basic.rs`](https://github.com/nucel-dev/agent-sdk/blob/main/crates/unified/examples/claude_basic.rs) — spawn + query + close against Claude Code.
//! - [`examples/codex_resume.rs`](https://github.com/nucel-dev/agent-sdk/blob/main/crates/unified/examples/codex_resume.rs) — spawn, save the `session_id`, resume, query.
//! - [`examples/opencode_http.rs`](https://github.com/nucel-dev/agent-sdk/blob/main/crates/unified/examples/opencode_http.rs) — point at a local `opencode serve` and send a prompt.
//! - [`examples/build_executor.rs`](https://github.com/nucel-dev/agent-sdk/blob/main/crates/unified/examples/build_executor.rs) — runtime provider selection via [`build_executor`].
//!
//! 0.2.0 features:
//!
//! - [`examples/streaming_claude.rs`](https://github.com/nucel-dev/agent-sdk/blob/main/crates/unified/examples/streaming_claude.rs) — `query_stream()`: tokens, tool-use, and cost events as they arrive.
//! - [`examples/multi_provider_handoff.rs`](https://github.com/nucel-dev/agent-sdk/blob/main/crates/unified/examples/multi_provider_handoff.rs) — run the same prompt against all 3 providers and compare cost.
//! - [`examples/with_hooks.rs`](https://github.com/nucel-dev/agent-sdk/blob/main/crates/unified/examples/with_hooks.rs) — pre/post tool-use, `on_stop`, `user_prompt_submit` (Claude Code only).
//! - [`examples/budget_control.rs`](https://github.com/nucel-dev/agent-sdk/blob/main/crates/unified/examples/budget_control.rs) — hit the `budget_usd` cap mid-loop and handle [`AgentError::BudgetExceeded`].
//! - [`examples/resume_session.rs`](https://github.com/nucel-dev/agent-sdk/blob/main/crates/unified/examples/resume_session.rs) — spawn → save id → close → resume → continue.
//!
//! Run any of them with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example claude_basic
//! ```
//!
//! # See also
//!
//! - [Workspace README](https://github.com/nucel-dev/agent-sdk#readme)
//! - [`docs/tutorials/`](https://github.com/nucel-dev/agent-sdk/tree/main/docs/tutorials) — getting started, multi-turn, streaming, hooks, cost & tokens, budget control, provider comparison.
//! - [`CONTRIBUTING.md`](https://github.com/nucel-dev/agent-sdk/blob/main/CONTRIBUTING.md) — adding a new provider.

#![cfg_attr(docsrs, feature(doc_cfg))]

// Re-export core types.
pub use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse, AgentSession,
    AvailabilityStatus, CachePoint, EventStream, ExecutorType, HookConfig, HookHandler,
    MessageEvent, PermissionMode, Result, SessionImpl, SessionMetadata, SpawnConfig,
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

    #[test]
    fn build_executor_empty_string_returns_none() {
        assert!(build_executor("", None).is_none());
    }

    #[test]
    fn build_executor_case_sensitive() {
        assert!(build_executor("Claude-Code", None).is_none());
        assert!(build_executor("CODEX", None).is_none());
        assert!(build_executor("OpenCode", None).is_none());
    }

    #[test]
    fn all_executors_have_capabilities() {
        for provider in available_providers() {
            let exec = build_executor(provider, None).unwrap();
            let caps = exec.capabilities();
            // All providers should support token usage
            assert!(caps.token_usage, "{provider} should support token_usage");
            // All providers should support autonomous mode
            assert!(caps.autonomous_mode, "{provider} should support autonomous_mode");
        }
    }

    #[test]
    fn all_executors_report_availability() {
        for provider in available_providers() {
            let exec = build_executor(provider, None).unwrap();
            let status = exec.availability();
            // Either available or has a reason
            if !status.available {
                assert!(status.reason.is_some(), "{provider} unavailable but no reason");
            }
        }
    }

    #[test]
    fn claude_code_api_key_ignored_by_build_executor() {
        // build_executor for claude-code ignores the api_key_or_url param
        let exec = build_executor("claude-code", Some("sk-test".into())).unwrap();
        assert_eq!(exec.executor_type(), ExecutorType::ClaudeCode);
    }

    #[test]
    fn codex_api_key_ignored_by_build_executor() {
        let exec = build_executor("codex", Some("sk-test".into())).unwrap();
        assert_eq!(exec.executor_type(), ExecutorType::Codex);
    }
}
