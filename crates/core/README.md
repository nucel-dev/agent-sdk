# nucel-agent-core

[![crates.io](https://img.shields.io/crates/v/nucel-agent-core.svg)](https://crates.io/crates/nucel-agent-core)
[![docs.rs](https://img.shields.io/docsrs/nucel-agent-core)](https://docs.rs/nucel-agent-core)
[![License](https://img.shields.io/crates/l/nucel-agent-core.svg)](../../LICENSE)

Core traits and types for the [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk) —
the provider-agnostic abstraction layer.

This crate has **zero provider dependencies**. Implement the `AgentExecutor` trait
here to add support for any coding-agent backend (CLI, HTTP server, library, etc.).

## What's in this crate

- **`AgentExecutor` trait** — `spawn()`, `resume()`, `capabilities()`, `availability()`
- **`AgentSession`** — follow-up `query()`, `total_cost()`, `close()`
- **`SessionImpl` trait** — what providers actually implement per-session
- **`SpawnConfig`** — model, budget, permission mode, env, system prompt, `max_turns`, …
- **`ExecutorConfig`** — provider-level configuration (`api_key`, `base_url`)
- **`AgentCapabilities`** — feature flags reported by each provider
- **`AvailabilityStatus`** — is the underlying CLI / server reachable?
- **Types** — `AgentCost`, `AgentResponse`, `ToolCall`, `ToolResult`, `ExecutorType`,
  `PermissionMode`, `SessionMetadata`
- **`AgentError`** — unified error type (`Provider`, `BudgetExceeded`, `CliNotFound`,
  `Timeout`, `EscalationRequested`, …)

## Usage

Most users should depend on the umbrella crate
[`nucel-agent-sdk`](https://crates.io/crates/nucel-agent-sdk) instead:

```toml
[dependencies]
nucel-agent-sdk = "0.1"
```

Use `nucel-agent-core` directly only if you're **implementing a new provider**:

```toml
[dependencies]
nucel-agent-core = "0.1"
async-trait = "0.1"
```

```rust
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse,
    AgentSession, AvailabilityStatus, ExecutorType, Result, SessionImpl, SpawnConfig,
};

pub struct MyExecutor;

struct MySession;

#[async_trait]
impl SessionImpl for MySession {
    async fn query(&self, _prompt: &str) -> Result<AgentResponse> {
        Ok(AgentResponse::default())
    }
    async fn total_cost(&self) -> Result<AgentCost> { Ok(AgentCost::default()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}

#[async_trait]
impl AgentExecutor for MyExecutor {
    fn executor_type(&self) -> ExecutorType { ExecutorType::ClaudeCode }

    async fn spawn(
        &self,
        working_dir: &Path,
        _prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession> {
        Ok(AgentSession::new(
            uuid::Uuid::new_v4().to_string(),
            ExecutorType::ClaudeCode,
            working_dir.to_path_buf(),
            config.model.clone(),
            Arc::new(MySession),
        ))
    }

    async fn resume(
        &self,
        working_dir: &Path,
        _session_id: &str,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession> {
        self.spawn(working_dir, prompt, config).await
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            session_resume: false,
            token_usage: false,
            mcp_support: false,
            autonomous_mode: false,
            structured_output: false,
        }
    }

    fn availability(&self) -> AvailabilityStatus {
        AvailabilityStatus { available: true, reason: None }
    }
}
```

## `SpawnConfig` fields

```rust
pub struct SpawnConfig {
    pub model: Option<String>,                 // "claude-opus-4-6", "gpt-5-codex", …
    pub max_tokens: Option<u32>,
    pub budget_usd: Option<f64>,               // session-wide USD budget
    pub permission_mode: Option<PermissionMode>,
    pub env: Vec<(String, String)>,            // extra subprocess env vars
    pub system_prompt: Option<String>,
    pub reasoning: Option<String>,             // provider-specific
    pub max_turns: Option<u32>,                // autonomous turns (added in 0.1.3)
}
```

## Provider crates

| Provider    | Crate                                                                  | Transport            |
|-------------|------------------------------------------------------------------------|----------------------|
| Claude Code | [`nucel-agent-claude-code`](https://crates.io/crates/nucel-agent-claude-code) | `claude` CLI subprocess |
| Codex       | [`nucel-agent-codex`](https://crates.io/crates/nucel-agent-codex)             | `codex` CLI subprocess  |
| OpenCode    | [`nucel-agent-opencode`](https://crates.io/crates/nucel-agent-opencode)       | `opencode serve` HTTP   |

## Source

- Repo: <https://github.com/nucel-dev/agent-sdk>
- Docs: <https://docs.rs/nucel-agent-core>

## License

Apache-2.0
