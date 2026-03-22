# nucel-agent-codex

Codex provider for [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk) — subprocess wrapper for the OpenAI `codex` CLI.

## Features

- **JSONL parsing** — `item.completed` (agent_message) and `turn.completed` events
- **Token tracking** — input/output tokens from usage reports
- **Budget enforcement** — automatic cost checks before each query
- **Structured output** — supports JSON Schema output (Codex-specific)
- **Model selection** — configurable model per query

## How it works

Spawns `codex exec --experimental-json "<prompt>"` as a subprocess, parses the JSONL output, and extracts:
- Agent text responses from `item.completed` → `agent_message` events
- Token usage from `turn.completed` → `usage` fields

## Usage

```toml
[dependencies]
nucel-agent-sdk = "0.1"
```

```rust
use nucel_agent_sdk::{CodexExecutor, AgentExecutor, SpawnConfig};

let executor = CodexExecutor::new();

// Check availability
let avail = executor.availability();
if !avail.available {
    eprintln!("Install: npm install -g @openai/codex");
    return;
}

// Spawn and query
let session = executor.spawn(
    std::path::Path::new("/my/repo"),
    "Fix the failing tests",
    &SpawnConfig {
        model: Some("gpt-5-codex".into()),
        budget_usd: Some(3.0),
        ..Default::default()
    },
).await?;

let resp = session.query("Now add more tests").await?;
println!("{}", resp.content);

session.close().await?;
```

## CLI Requirements

- `codex` CLI installed: `npm install -g @openai/codex`
- Valid `CODEX_API_KEY` or OpenAI API key

## License

Apache-2.0
