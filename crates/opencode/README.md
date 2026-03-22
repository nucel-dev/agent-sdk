# nucel-agent-opencode

OpenCode provider for [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk) — HTTP client to the OpenCode server.

## Features

- **HTTP client** — connects to `opencode serve` via REST API
- **Session management** — create sessions, send prompts, get responses
- **Native resume** — OpenCode supports resuming existing sessions by ID
- **MCP support** — OpenCode's MCP integration is available
- **Budget enforcement** — automatic cost checks before each query
- **Directory header** — sends `x-opencode-directory` header for context

## How it works

OpenCode runs as a server (`opencode serve` on `:4096`). This provider:
1. Creates a session via `POST /session`
2. Sends prompts via `POST /session/{id}/prompt`
3. Parses the response's `parts` array for text content
4. Tracks cost from the `cost` field in responses

## Usage

```toml
[dependencies]
nucel-agent-sdk = "0.1"
```

```rust
use nucel_agent_sdk::{OpencodeExecutor, AgentExecutor, SpawnConfig};

let executor = OpencodeExecutor::new(); // default: http://127.0.0.1:4096

// Or custom URL
let executor = OpencodeExecutor::with_base_url("http://my-server:8080");

// Spawn and query
let session = executor.spawn(
    std::path::Path::new("/my/repo"),
    "Fix the failing tests",
    &SpawnConfig {
        model: Some("claude-sonnet-4".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    },
).await?;

// Follow-up query (uses existing session)
let resp = session.query("Now add more tests").await?;
println!("{}", resp.content);

session.close().await?;
```

## Server Requirements

- OpenCode installed: `npm install -g opencode`
- Server running: `opencode serve` (default port 4096)

## License

Apache-2.0
