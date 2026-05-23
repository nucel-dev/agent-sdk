# nucel-agent-codex

[![crates.io](https://img.shields.io/crates/v/nucel-agent-codex.svg)](https://crates.io/crates/nucel-agent-codex)
[![docs.rs](https://img.shields.io/docsrs/nucel-agent-codex)](https://docs.rs/nucel-agent-codex)
[![License](https://img.shields.io/crates/l/nucel-agent-codex.svg)](../../LICENSE)

Codex provider for the [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk) —
a subprocess wrapper for OpenAI's `codex` CLI.

## Features

- **JSONL event parsing** — handles `thread.started`, `turn.started`,
  `item.completed`, `turn.completed`, `turn.failed`, and `error` events
- **Token tracking** — input / output tokens from `turn.completed.token_usage`
- **Budget enforcement** — client-side cost guard before each query
- **Sandbox + approval modes** — maps `PermissionMode` to the official
  `--sandbox` and `--full-auto` flags
- **Structured output** — Codex supports JSON Schema responses
  (capabilities: `structured_output: true`)
- **Model selection** — `--model` flag
- **`--skip-git-repo-check`** — works in arbitrary working directories

## How it works

Each query spawns a fresh `codex exec` invocation:

```bash
codex exec \
  --json \
  --skip-git-repo-check \
  --model <model> \
  --sandbox workspace-write \
  --cd <working_dir> \
  "<prompt>"
```

Then parses the official JSONL event stream:

```text
thread.started  →  turn.started  →  item.completed (agent_message, reasoning, …)  →  turn.completed
```

The provider extracts:

- Agent text from `item.completed` items where `item.type == "agent_message"`
- Token usage from `turn.completed.token_usage.{input_tokens, output_tokens}`
- Errors from `turn.failed` / `error` events

Reference: [Codex CLI docs](https://developers.openai.com/codex/cli/reference/).

### CLI flag mapping

| `SpawnConfig` field | CLI flag |
|---------------------|----------|
| `model` | `--model` |
| `permission_mode` | see sandbox table below |
| `env` | passed as subprocess environment |
| working dir | `--cd <path>` |

| `PermissionMode` | Codex flag(s) |
|---|---|
| `Prompt` *(default)* | `--sandbox workspace-write` |
| `AcceptEdits` | `--full-auto` |
| `BypassPermissions` | `--dangerously-bypass-approvals-and-sandbox` |
| `RejectAll` | `--sandbox read-only` |

### Session resume

Codex CLI resume (`codex exec resume`) is not yet wired up — calling
`resume()` will log an info message and **spawn a fresh session**. Hence:

```text
capabilities.session_resume = false
```

## Usage

```toml
[dependencies]
nucel-agent-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
use nucel_agent_sdk::{CodexExecutor, AgentExecutor, PermissionMode, SpawnConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executor = CodexExecutor::new();
    // Or supply an API key explicitly:
    // let executor = CodexExecutor::with_api_key(std::env::var("OPENAI_API_KEY")?);

    let avail = executor.availability();
    if !avail.available {
        eprintln!("{}", avail.reason.unwrap_or_default());
        // → "`codex` CLI not found. Install: npm install -g @openai/codex"
        return Ok(());
    }

    let session = executor.spawn(
        Path::new("/my/repo"),
        "Fix the failing tests",
        &SpawnConfig {
            model: Some("gpt-5-codex".into()),
            budget_usd: Some(3.0),
            permission_mode: Some(PermissionMode::AcceptEdits),
            ..Default::default()
        },
    ).await?;

    let resp = session.query("Now add more tests").await?;
    println!("{}", resp.content);

    session.close().await?;
    Ok(())
}
```

## Capabilities

```text
session_resume:     false  (CLI resume not yet wired up)
token_usage:        true
mcp_support:        false
autonomous_mode:    true
structured_output:  true
```

## Requirements

- `codex` CLI on PATH: `npm install -g @openai/codex`
- `OPENAI_API_KEY` (also exported as `CODEX_API_KEY` for `codex exec` compatibility)

## Source

- Repo: <https://github.com/nucel-dev/agent-sdk>
- Docs: <https://docs.rs/nucel-agent-codex>

## License

Apache-2.0
