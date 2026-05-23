# nucel-agent-claude-code

[![crates.io](https://img.shields.io/crates/v/nucel-agent-claude-code.svg)](https://crates.io/crates/nucel-agent-claude-code)
[![docs.rs](https://img.shields.io/docsrs/nucel-agent-claude-code)](https://docs.rs/nucel-agent-claude-code)
[![License](https://img.shields.io/crates/l/nucel-agent-claude-code.svg)](../../LICENSE)

Claude Code provider for the [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk) —
a subprocess wrapper for Anthropic's `claude` CLI.

## Features

- **Streaming JSONL protocol** — parses `system/init`, `assistant`, `result`,
  and rate-limit events line-by-line
- **Session resume** — uses the official `--resume <session_id>` CLI flag
- **Multi-turn** — keeps the subprocess alive and writes prompts to stdin
  (`start_interactive` mode)
- **Token tracking** — input / output tokens reported by the CLI
- **Budget enforcement** — passes `--max-budget-usd` to the CLI **and**
  guards client-side before/after every query
- **Timeout protection** — configurable timeout per query (default: 10 minutes)
- **Permission modes** — `Prompt`, `AcceptEdits`, `BypassPermissions`, `RejectAll`
  → mapped to the official `--permission-mode` values
- **`max_turns` control** — passes `--max-turns <n>` (defaults to 1 for the
  initial spawn)
- **Graceful shutdown** — SIGTERM then SIGKILL fallback

## How it works

Spawns the `claude` CLI in **print mode** with streaming JSON output:

```bash
claude \
  --model <model> \
  --permission-mode <mode> \
  --max-budget-usd <amount> \
  --system-prompt <prompt> \
  -p "<user prompt>" \
  --output-format stream-json \
  --verbose \
  --max-turns <n>
```

Then parses the JSONL stream and extracts:

- Text content from `assistant` messages
- Cost from `result.total_cost_usd`
- Token usage from `result.usage`
- Session ID from `system/init` (for resume)

For session resume, `--resume <session_id>` is added.

### CLI flag mapping

| `SpawnConfig` field | CLI flag |
|---------------------|----------|
| `model` | `--model` |
| `permission_mode` | `--permission-mode {default\|acceptEdits\|bypassPermissions\|plan}` |
| `budget_usd` | `--max-budget-usd` |
| `system_prompt` | `--system-prompt` |
| `max_turns` | `--max-turns` |
| `env` | passed as subprocess environment |

| `PermissionMode` | `--permission-mode` value |
|---|---|
| `Prompt` | `default` |
| `AcceptEdits` | `acceptEdits` |
| `BypassPermissions` | `bypassPermissions` |
| `RejectAll` | `plan` |

Reference: [Claude Code CLI docs](https://code.claude.com/docs/en/cli-reference).

## Usage

```toml
[dependencies]
nucel-agent-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
use nucel_agent_sdk::{ClaudeCodeExecutor, AgentExecutor, PermissionMode, SpawnConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executor = ClaudeCodeExecutor::new();
    // Or supply an API key explicitly:
    // let executor = ClaudeCodeExecutor::with_api_key(std::env::var("ANTHROPIC_API_KEY")?);

    // Check availability
    let avail = executor.availability();
    if !avail.available {
        eprintln!("{}", avail.reason.unwrap_or_default());
        // → "`claude` CLI not found. Install: npm install -g @anthropic-ai/claude-code"
        return Ok(());
    }

    let session = executor.spawn(
        Path::new("/my/repo"),
        "Fix the failing tests in src/lib.rs",
        &SpawnConfig {
            model: Some("claude-opus-4-6".into()),
            budget_usd: Some(5.0),
            permission_mode: Some(PermissionMode::AcceptEdits),
            max_turns: Some(10),
            ..Default::default()
        },
    ).await?;

    let resp = session.query("Did the tests pass?").await?;
    println!("{}", resp.content);

    let cost = session.total_cost().await?;
    println!("Total cost: ${:.4}", cost.total_usd);

    session.close().await?;
    Ok(())
}
```

## Capabilities

```text
session_resume:     true   (via --resume <id>)
token_usage:        true
mcp_support:        true
autonomous_mode:    true
structured_output:  false
```

## Requirements

- `claude` CLI on PATH: `npm install -g @anthropic-ai/claude-code`
- A valid `ANTHROPIC_API_KEY` env var **or** a logged-in Claude Max / Pro
  subscription (via `claude login`)

## Source

- Repo: <https://github.com/nucel-dev/agent-sdk>
- Docs: <https://docs.rs/nucel-agent-claude-code>

## License

Apache-2.0
