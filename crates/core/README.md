# nucel-agent-core

Core traits and types for [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk) — provider-agnostic AI coding agent abstraction.

## What's in this crate

- **`AgentExecutor` trait** — `spawn()`, `resume()`, `capabilities()`, `availability()`
- **`AgentSession`** — follow-up queries, cost tracking, cleanup
- **Types** — `AgentCost`, `AgentResponse`, `AgentError`, `ExecutorType`, `PermissionMode`, `SpawnConfig`

## Usage

This crate is not used directly — import `nucel-agent-sdk` instead:

```toml
[dependencies]
nucel-agent-sdk = "0.1"
```

```rust
use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};

let executor = ClaudeCodeExecutor::new();
let session = executor.spawn(
    std::path::Path::new("/my/repo"),
    "Fix the failing tests",
    &SpawnConfig {
        model: Some("claude-opus-4-6".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    },
).await?;

let resp = session.query("Did CI pass?").await?;
println!("{}", resp.content);

let cost = session.total_cost().await?;
println!("Total: ${:.4}", cost.total_usd);
```

## Supported Providers

| Provider | Crate | CLI/Server |
|----------|-------|------------|
| Claude Code | `nucel-agent-claude-code` | `claude` CLI |
| Codex | `nucel-agent-codex` | `codex` CLI |
| OpenCode | `nucel-agent-opencode` | `opencode serve` |

## License

Apache-2.0
