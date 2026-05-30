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
    is_transient, AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse,
    AgentSession, AvailabilityStatus, CachePoint, EventStream, ExecutorType, HookConfig,
    HookHandler, MessageEvent, PermissionMode, Result, RetryPolicy, SessionImpl, SessionMetadata,
    SpawnConfig,
};

/// Re-export of the retry policy module for callers that want the
/// [`is_transient`] classifier and [`RetryPolicy`] helpers under a namespace.
pub use nucel_agent_core::retry;

// Re-export provider executors.
pub use nucel_agent_claude_code::ClaudeCodeExecutor;
pub use nucel_agent_codex::CodexExecutor;
pub use nucel_agent_opencode::OpencodeExecutor;

#[cfg(feature = "bedrock")]
#[cfg_attr(docsrs, doc(cfg(feature = "bedrock")))]
pub use nucel_agent_bedrock::BedrockExecutor;

#[cfg(feature = "vertex")]
#[cfg_attr(docsrs, doc(cfg(feature = "vertex")))]
pub use nucel_agent_vertex::VertexExecutor;

/// Build an executor from a config string (like `providers.agent = "claude-code"`).
///
/// - `"claude-code"` → `ClaudeCodeExecutor`
/// - `"codex"` → `CodexExecutor`
/// - `"opencode"` → `OpencodeExecutor` (second arg is base URL)
/// - `"bedrock"` → `BedrockExecutor` (feature `bedrock`; uses AWS default chain)
/// - `"vertex"` → `VertexExecutor` (feature `vertex`; second arg is
///   `"<project>:<region>"`)
///
/// For `"bedrock"` and `"vertex"` this function spins a tiny `tokio`
/// runtime to satisfy the async credential lookups — fine for one-shot
/// startup. Async callers should construct the executor directly via the
/// provider crate to avoid the nested runtime.
///
/// Returns `None` for unknown providers, or for `"bedrock"`/`"vertex"`
/// when the feature is disabled or required config is missing.
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
        #[cfg(feature = "bedrock")]
        "bedrock" => {
            // Async credential lookup — done on a dedicated short-lived
            // runtime so this stays sync at the API boundary.
            let exec = build_bedrock_blocking()?;
            Some(Box::new(exec))
        }
        #[cfg(feature = "vertex")]
        "vertex" => {
            // Expect `api_key_or_url == "project:region"` (e.g.
            // `"my-proj:us-east5"`). Without this we can't form an endpoint.
            let spec = api_key_or_url?;
            let (project, region) = spec.split_once(':')?;
            if project.is_empty() || region.is_empty() {
                return None;
            }
            let exec = build_vertex_blocking(project, region)?;
            Some(Box::new(exec))
        }
        _ => None,
    }
}

#[cfg(feature = "bedrock")]
fn build_bedrock_blocking() -> Option<BedrockExecutor> {
    // If we're already inside a tokio runtime, `block_on` would deadlock;
    // spawn_blocking + Handle::block_on is the safe pattern.
    let exec = if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|s| {
            s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?;
                Some(rt.block_on(BedrockExecutor::new()))
            })
            .join()
            .ok()
            .flatten()
        })
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        Some(rt.block_on(BedrockExecutor::new()))
    };
    exec
}

#[cfg(feature = "vertex")]
fn build_vertex_blocking(project: &str, region: &str) -> Option<VertexExecutor> {
    let project = project.to_string();
    let region = region.to_string();
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|s| {
            s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?;
                rt.block_on(VertexExecutor::with_adc(project, region)).ok()
            })
            .join()
            .ok()
            .flatten()
        })
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        rt.block_on(VertexExecutor::with_adc(project, region)).ok()
    }
}

/// List all available provider names — feature-gated providers only show
/// up when their crate feature is enabled.
pub fn available_providers() -> &'static [&'static str] {
    #[cfg(all(feature = "bedrock", feature = "vertex"))]
    {
        return &["claude-code", "codex", "opencode", "bedrock", "vertex"];
    }
    #[cfg(all(feature = "bedrock", not(feature = "vertex")))]
    {
        return &["claude-code", "codex", "opencode", "bedrock"];
    }
    #[cfg(all(not(feature = "bedrock"), feature = "vertex"))]
    {
        return &["claude-code", "codex", "opencode", "vertex"];
    }
    #[cfg(not(any(feature = "bedrock", feature = "vertex")))]
    {
        &["claude-code", "codex", "opencode"]
    }
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
        // Three base providers are always present; bedrock/vertex appear
        // when their features are enabled.
        assert!(providers.len() >= 3);
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

    /// Providers that are constructed sync with no config — the three
    /// process/HTTP-based ones. Bedrock/Vertex need cloud credentials and
    /// async setup so we cover them separately.
    fn locally_constructible_providers() -> &'static [&'static str] {
        &["claude-code", "codex", "opencode"]
    }

    #[test]
    fn all_executors_have_capabilities() {
        for provider in locally_constructible_providers() {
            let exec = build_executor(provider, None).unwrap();
            let caps = exec.capabilities();
            // All process-based providers should support token usage
            assert!(caps.token_usage, "{provider} should support token_usage");
            // All process-based providers should support autonomous mode
            assert!(
                caps.autonomous_mode,
                "{provider} should support autonomous_mode"
            );
        }
    }

    #[test]
    fn all_executors_report_availability() {
        for provider in locally_constructible_providers() {
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

    #[cfg(feature = "vertex")]
    #[test]
    fn vertex_requires_project_and_region_spec() {
        // Missing spec → None
        assert!(build_executor("vertex", None).is_none());
        // Malformed spec → None
        assert!(build_executor("vertex", Some("only-project".into())).is_none());
        // Empty project → None
        assert!(build_executor("vertex", Some(":us-east5".into())).is_none());
        // Empty region → None
        assert!(build_executor("vertex", Some("p:".into())).is_none());
    }

    #[cfg(feature = "vertex")]
    #[test]
    fn vertex_string_appears_in_available_providers() {
        assert!(available_providers().contains(&"vertex"));
    }

    #[cfg(feature = "bedrock")]
    #[test]
    fn bedrock_string_appears_in_available_providers() {
        assert!(available_providers().contains(&"bedrock"));
    }

    #[cfg(not(feature = "bedrock"))]
    #[test]
    fn bedrock_unavailable_without_feature() {
        assert!(build_executor("bedrock", None).is_none());
    }

    #[cfg(not(feature = "vertex"))]
    #[test]
    fn vertex_unavailable_without_feature() {
        assert!(build_executor("vertex", None).is_none());
    }
}
