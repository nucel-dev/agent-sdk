# nucel-agent-claude-code

Claude Code provider for [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk) — subprocess wrapper for the `claude` CLI.

## Features

- **Streaming JSONL** — real-time parsing of `system/init`, `assistant`, `result` messages
- **Token tracking** — input, output, cache_read, cache_creation tokens
- **Budget enforcement** — automatic cost checks before each query
- **Timeout protection** — configurable timeout per query (default: 10 minutes)
- **Permission modes** — `prompt`, `accept_edits`, `bypass_permissions`, `reject_all`
- **Stderr capture** — for debugging CLI errors

## How it works

Spawns `claude -p "<prompt>" --output-format stream-json --verbose` as a subprocess, parses the JSONL output line-by-line, and extracts:
- Text responses from `assistant` messages
- Cost from `result.total_cost_usd`
- Token usage from `result.usage` and `result.modelUsage`

## Usage

```toml
[dependencies]
nucel-agent-sdk = "0.1"
```

```rust
use nucel_agent_sdk::{ClaudeCodeExecutor, AgentExecutor, SpawnConfig};

let executor = ClaudeCodeExecutor::new();

// Check availability
let avail = executor.availability();
if !avail.available {
    eprintln!("Install: npm install -g @anthropic-ai/claude-code");
    return;
}

// Spawn and query
let session = executor.spawn(
    std::path::Path::new("/my/repo"),
    "Fix the failing tests",
    &SpawnConfig {
        model: Some("claude-opus-4-6".into()),
        budget_usd: Some(5.0),
        permission_mode: Some(nucel_agent_sdk::PermissionMode::AcceptEdits),
        ..Default::default()
    },
).await?;

let cost = session.total_cost().await?;
println!("Cost: ${:.4}", cost.total_usd);
session.close().await?;
```

## CLI Requirements

- `claude` CLI installed: `npm install -g @anthropic-ai/claude-code`
- Valid `ANTHROPIC_API_KEY` or Claude Max/Pro subscription

## License

Apache-2.0
