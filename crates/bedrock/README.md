# nucel-agent-bedrock

AWS Bedrock provider for the [Nucel Agent SDK](https://github.com/nucel-dev/agent-sdk).

Implements `AgentExecutor` on top of the [`aws-sdk-bedrockruntime`](https://docs.rs/aws-sdk-bedrockruntime) `Converse` API. Lets you talk to Claude (and any other Bedrock-served model that supports the Converse interface) through the same trait surface as Claude Code, Codex, and OpenCode.

## Quick start

```toml
[dependencies]
nucel-agent-bedrock = "0.1.0"
nucel-agent-core   = "0.2.0"
tokio = { version = "1", features = ["full"] }
```

```rust,no_run
use nucel_agent_bedrock::BedrockExecutor;
use nucel_agent_core::{AgentExecutor, SpawnConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> nucel_agent_core::Result<()> {
    let executor = BedrockExecutor::new().await;

    let session = executor.spawn(
        Path::new("/my/repo"),
        "Summarize this codebase.",
        &SpawnConfig {
            model: Some("anthropic.claude-opus-4-7-20251024-v2:0".into()),
            budget_usd: Some(2.0),
            ..Default::default()
        },
    ).await?;

    println!("{}", session.query("Any TODOs?").await?.content);
    session.close().await?;
    Ok(())
}
```

## Credentials

Uses the default AWS provider chain (`aws_config::from_env`):

- `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` (+ optional `AWS_SESSION_TOKEN`)
- `~/.aws/credentials` profile
- IMDS / ECS task role / SSO
- `AWS_REGION` (Bedrock requires an explicit region)

If credentials cannot be resolved, `availability()` returns `available = false` with a descriptive reason. `spawn()` will still attempt the call so the SDK error reaches the caller verbatim.

## Supported features

| Capability         | Status                                       |
| ------------------ | -------------------------------------------- |
| `query()`          | yes — single `Converse` request per turn      |
| `query_stream()`   | falls back to non-streaming `query()` impl   |
| `session_resume`   | no — transcripts are client-side             |
| `token_usage`      | yes — pulled from Bedrock invocation metadata|
| `mcp_support`      | no                                           |
| `prompt_caching`   | no (planned)                                 |
| `extended_thinking`| no (planned)                                 |

## Cost tracking

Token counts come from Bedrock invocation metadata. USD cost is estimated against an internal price table (`pricing::lookup`). Treat reported cost as an approximation and reconcile against your AWS invoice; the price table is best-effort and ships per-region us-east-1 on-demand rates.

## Model IDs

Common Bedrock Claude IDs (and inference-profile prefixes) supported by the price table:

- `anthropic.claude-opus-4-7-20251024-v2:0`
- `anthropic.claude-sonnet-4-20251015-v1:0`
- `anthropic.claude-haiku-4-20251015-v1:0`
- `us.anthropic.claude-...` (cross-region inference profile)

Unknown models still work — only the cost estimate falls back to `$0.00`.

## License

Apache-2.0
