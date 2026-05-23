# nucel-agent-sdk

[![crates.io](https://img.shields.io/crates/v/nucel-agent-sdk.svg)](https://crates.io/crates/nucel-agent-sdk)
[![docs.rs](https://img.shields.io/docsrs/nucel-agent-sdk)](https://docs.rs/nucel-agent-sdk)
[![License](https://img.shields.io/crates/l/nucel-agent-sdk.svg)](../../LICENSE)

Umbrella crate for the [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk).

**Provider-agnostic Rust SDK for AI coding agents.** Spawn Claude Code, Codex,
or OpenCode through a single `AgentExecutor` trait — swap providers via config,
your code stays the same.

This crate re-exports:

- Everything from [`nucel-agent-core`](https://crates.io/crates/nucel-agent-core)
  (traits, types, errors)
- [`ClaudeCodeExecutor`](https://crates.io/crates/nucel-agent-claude-code)
- [`CodexExecutor`](https://crates.io/crates/nucel-agent-codex)
- [`OpencodeExecutor`](https://crates.io/crates/nucel-agent-opencode)

…plus a `build_executor()` factory for runtime provider selection.

## Install

```toml
[dependencies]
nucel-agent-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
```

## Quick Start

```rust
use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executor = ClaudeCodeExecutor::new();

    let session = executor.spawn(
        Path::new("/my/repo"),
        "Fix the failing tests",
        &SpawnConfig {
            model: Some("claude-opus-4-6".into()),
            budget_usd: Some(5.0),
            max_turns: Some(10),
            ..Default::default()
        },
    ).await?;

    let resp = session.query("Did CI pass?").await?;
    println!("{}", resp.content);

    let cost = session.total_cost().await?;
    println!("spent ${:.4}", cost.total_usd);

    session.close().await?;
    Ok(())
}
```

## Pick a provider at runtime

```rust
use nucel_agent_sdk::{build_executor, available_providers};

let executor = build_executor("claude-code", None).unwrap();
let executor = build_executor("codex", None).unwrap();
let executor = build_executor("opencode", Some("http://localhost:4096".into())).unwrap();

for name in available_providers() {
    println!("provider: {name}");
}
```

Accepted aliases for Claude Code: `"claude-code"`, `"claude_code"`, `"claudecode"`
(case-sensitive). `"codex"` and `"opencode"` are exact.

## Feature matrix

| Capability                  | Claude Code | Codex | OpenCode |
|-----------------------------|:-----------:|:-----:|:--------:|
| `session_resume`            | yes         | no    | yes      |
| `token_usage`               | yes         | yes   | yes      |
| `mcp_support`               | yes         | no    | yes      |
| `autonomous_mode`           | yes         | yes   | yes      |
| `structured_output`         | no          | yes   | no       |
| Transport                   | subprocess  | subprocess | HTTP |
| Required runtime            | `claude` CLI | `codex` CLI | `opencode serve` |

## More

- Workspace README + full feature matrix: <https://github.com/nucel-dev/agent-sdk>
- Architecture & "add a new provider": [`docs/architecture.md`](https://github.com/nucel-dev/agent-sdk/blob/main/docs/architecture.md)
- Changelog: [`CHANGELOG.md`](https://github.com/nucel-dev/agent-sdk/blob/main/CHANGELOG.md)

## License

Apache-2.0
