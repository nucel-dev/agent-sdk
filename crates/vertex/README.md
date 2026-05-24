# nucel-agent-vertex

GCP Vertex AI provider for the [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk).

Implements `AgentExecutor` against Vertex's Claude `rawPredict` endpoint:

```
https://<region>-aiplatform.googleapis.com/v1/projects/<project>/locations/<region>/publishers/anthropic/models/<model>:rawPredict
```

Same trait surface as Claude Code, Codex, OpenCode, and Bedrock — swap providers via configuration.

## Quick start

```toml
[dependencies]
nucel-agent-vertex = "0.1.0"
nucel-agent-core   = "0.2.0"
tokio = { version = "1", features = ["full"] }
```

```rust,no_run
use nucel_agent_vertex::VertexExecutor;
use nucel_agent_core::{AgentExecutor, SpawnConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> nucel_agent_core::Result<()> {
    let executor = VertexExecutor::with_adc("my-gcp-project", "us-east5").await?;

    let session = executor.spawn(
        Path::new("/my/repo"),
        "Summarize this codebase.",
        &SpawnConfig {
            model: Some("claude-opus-4-7@20251024".into()),
            budget_usd: Some(2.0),
            ..Default::default()
        },
    ).await?;

    println!("{}", session.query("Any TODOs?").await?.content);
    session.close().await?;
    Ok(())
}
```

## Authentication

Uses Google Application Default Credentials via `gcp_auth`:

- `GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json`
- `gcloud auth application-default login`
- GCE / GKE workload identity / metadata server

If creds are unreachable, `VertexExecutor::with_adc()` returns `AgentError::Config` with a remediation hint. Tests and sidecar flows can swap in a static token via `VertexExecutor::with_static_token(...)`.

## Capabilities

| Capability         | Status                                                       |
| ------------------ | ------------------------------------------------------------ |
| `query()`          | yes — one `:rawPredict` POST per turn                         |
| `query_stream()`   | falls back to non-streaming `query()` impl                   |
| `session_resume`   | no — transcripts are client-side                             |
| `token_usage`      | yes — `usage.input_tokens` / `usage.output_tokens`           |
| `mcp_support`      | no                                                            |
| `prompt_caching`   | yes — passes through `cache_read_input_tokens` etc.          |
| `extended_thinking`| no (planned)                                                  |

## Model IDs

Vertex uses `<family>@<release>` IDs:

- `claude-opus-4-7@20251024`
- `claude-sonnet-4@20251015`
- `claude-haiku-4@20251015`

Cost is estimated from token counts × the price table (`pricing::lookup`); unknown models fall back to `$0.00` and a debug log.

## Region

Pick whichever Vertex region has Claude allowlisted on your project. As of writing, `us-east5` and `europe-west1` are common choices.

## License

Apache-2.0
