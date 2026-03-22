# Nucel Agent SDK

Provider-agnostic Rust SDK for AI coding agents. One trait, multiple backends.

Part of the [Nucel](https://github.com/nucel-dev) ecosystem.

## Supported Providers

| Provider | Executor | Protocol | CLI Required |
|----------|----------|----------|--------------|
| **Claude Code** | `ClaudeCodeExecutor` | JSONL subprocess | `claude` |
| **Codex** | `CodexExecutor` | JSONL subprocess | `codex` |
| **OpenCode** | `OpencodeExecutor` | HTTP + SSE | `opencode serve` |

## Quick Start

```rust
use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executor = ClaudeCodeExecutor::new();

    // Check availability
    let avail = executor.availability();
    if !avail.available {
        eprintln!("Not available: {:?}", avail.reason);
        return Ok(());
    }

    // Spawn session with first prompt
    let session = executor.spawn(
        Path::new("/my/repo"),
        "Fix the failing tests in src/lib.rs",
        &SpawnConfig {
            model: Some("claude-opus-4-6".into()),
            budget_usd: Some(5.0),
            ..Default::default()
        },
    ).await?;

    println!("Response: {}", session.query("Did CI pass?").await?.content);

    let cost = session.total_cost().await?;
    println!("Total cost: ${:.4}", cost.total_usd);

    session.close().await?;
    Ok(())
}
```

## Provider Selection

```rust
use nucel_agent_sdk::*;

// From config string (like agent-operator)
let executor = build_executor("claude-code", None).unwrap();
let executor = build_executor("codex", None).unwrap();
let executor = build_executor("opencode", Some("http://localhost:4096".into())).unwrap();
```

## Architecture

```
nucel-agent-sdk (unified - re-exports)
├── nucel-agent-core      (traits + types, zero provider deps)
├── nucel-agent-claude-code (Claude Code CLI wrapper)
├── nucel-agent-codex       (Codex CLI wrapper)
└── nucel-agent-opencode    (OpenCode HTTP client)
```

### Core Trait

```rust
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    fn executor_type(&self) -> ExecutorType;
    async fn spawn(&self, working_dir: &Path, prompt: &str, config: &SpawnConfig) -> Result<AgentSession>;
    async fn resume(&self, working_dir: &Path, session_id: &str, prompt: &str, config: &SpawnConfig) -> Result<AgentSession>;
    fn capabilities(&self) -> AgentCapabilities;
    fn availability(&self) -> AvailabilityStatus;
}
```

### Session API

```rust
let session = executor.spawn(working_dir, "fix bug", &config).await?;

// Follow-up queries
let resp = session.query("now add tests").await?;

// Cost tracking
let cost = session.total_cost().await?;

// Cleanup
session.close().await?;
```

## Crates

| Crate | Description |
|-------|-------------|
| `nucel-agent-core` | Core traits (`AgentExecutor`), types (`AgentResponse`, `AgentCost`), session |
| `nucel-agent-claude-code` | Claude Code CLI subprocess wrapper |
| `nucel-agent-codex` | OpenAI Codex CLI subprocess wrapper |
| `nucel-agent-opencode` | OpenCode HTTP server client |
| `nucel-agent-sdk` | Unified re-export crate + `build_executor()` |

## Adding a New Provider

1. Create `crates/my-provider/`
2. Implement `AgentExecutor` trait
3. Add to workspace `Cargo.toml`
4. Re-export in `crates/unified/src/lib.rs`
5. Add to `build_executor()` match

## Integration with agent-operator

```toml
# In agent-operator Cargo.toml
[dependencies]
nucel-agent-sdk = { path = "../agent-sdk/crates/unified" }
```

```rust
// In adapter
use nucel_agent_sdk::{AgentExecutor, build_executor};

let executor = build_executor(&config.providers.agent, None)?;
let session = executor.spawn(working_dir, prompt, &spawn_config).await?;
```

## License

Apache-2.0
