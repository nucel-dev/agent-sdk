# nucel-agent-opencode

[![crates.io](https://img.shields.io/crates/v/nucel-agent-opencode.svg)](https://crates.io/crates/nucel-agent-opencode)
[![docs.rs](https://img.shields.io/docsrs/nucel-agent-opencode)](https://docs.rs/nucel-agent-opencode)
[![License](https://img.shields.io/crates/l/nucel-agent-opencode.svg)](../../LICENSE)

OpenCode provider for the [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk) —
an HTTP client for the [OpenCode](https://opencode.ai) server.

## Features

- **HTTP REST client** — talks to `opencode serve` (no subprocess management)
- **Session management** — `POST /session` then `POST /session/{id}/prompt`
- **Native session resume** — `resume()` re-uses the existing OpenCode session ID
- **MCP support** — OpenCode's native MCP integration is exposed to the agent
- **Budget enforcement** — client-side cost guard before each query
- **Directory header** — sets `x-opencode-directory` so the server scopes file
  operations to the right working directory
- **System prompt + model override** — passed in the JSON body

## How it works

OpenCode runs as a long-lived server (`opencode serve`, default port `4096`).
This provider:

1. Issues `POST {base_url}/session` to create a new session, returning a server
   session ID.
2. Issues `POST {base_url}/session/{id}/prompt` with:
   ```json
   {
     "parts": [{ "type": "text", "text": "<prompt>" }],
     "model":  { "modelID": "<model>" },   // optional
     "system": "<system prompt>"           // optional
   }
   ```
3. Parses the response — concatenates all `parts[].text` entries with
   `type == "text"`, falls back to the top-level `text` field if `parts` is empty.
4. Reads cost from the response's `cost` field.

For `resume(session_id, …)`, the existing OpenCode session ID is reused instead
of creating a new one.

## Usage

```toml
[dependencies]
nucel-agent-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
use nucel_agent_sdk::{OpencodeExecutor, AgentExecutor, SpawnConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Default: http://127.0.0.1:4096
    let executor = OpencodeExecutor::new();

    // Or point at a remote server:
    let executor = OpencodeExecutor::with_base_url("http://my-server:8080")
        .with_api_key("secret-token");

    let session = executor.spawn(
        Path::new("/my/repo"),
        "Fix the failing tests",
        &SpawnConfig {
            model: Some("claude-sonnet-4".into()),
            budget_usd: Some(5.0),
            system_prompt: Some("You are a senior Rust engineer.".into()),
            ..Default::default()
        },
    ).await?;

    // Follow-up query — reuses the same OpenCode session on the server.
    let resp = session.query("Now add more tests").await?;
    println!("{}", resp.content);

    session.close().await?;
    Ok(())
}
```

## Capabilities

```text
session_resume:     true   (native via session ID)
token_usage:        true
mcp_support:        true
autonomous_mode:    true
structured_output:  false
```

## Requirements

- OpenCode installed: `npm install -g opencode`
- Server running: `opencode serve` (default `http://127.0.0.1:4096`)
- (Optional) API key if your OpenCode server enforces auth

> `availability()` always returns `available: true` for OpenCode — the executor
> can't easily probe the network here, so the `reason` field is used as a hint
> ("Run `opencode serve` to start server at …") rather than a hard error.
> Connection errors surface from `spawn()` / `query()` as `AgentError::Provider`.

## Source

- Repo: <https://github.com/nucel-dev/agent-sdk>
- Docs: <https://docs.rs/nucel-agent-opencode>

## License

Apache-2.0
