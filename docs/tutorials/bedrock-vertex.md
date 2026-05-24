# Tutorial — Claude on AWS Bedrock & GCP Vertex AI

The Nucel Agent SDK ships first-class providers for hosted Claude on the two big-cloud gateways:

- `nucel-agent-bedrock` — `aws-sdk-bedrockruntime` `Converse` API
- `nucel-agent-vertex`  — Vertex AI `:rawPredict` Anthropic endpoint

Both implement the same `AgentExecutor` trait that Claude Code / Codex / OpenCode implement, so you can drop them into existing code with zero refactors.

This tutorial walks through:

1. When to pick Bedrock vs Vertex vs Claude Code CLI.
2. Adding the dependency.
3. Wiring credentials.
4. Spawning a session and running multi-turn conversations.
5. Reading cost & token usage.
6. Wire-mocked tests (no real cloud calls).

---

## 1. Picking a provider

| Criterion                | Claude Code CLI            | Bedrock                          | Vertex                              |
| ------------------------ | -------------------------- | -------------------------------- | ----------------------------------- |
| Where data lives         | Anthropic                  | AWS account / region             | GCP project / region                |
| Auth                     | `ANTHROPIC_API_KEY` / OAuth | AWS IAM (default provider chain) | GCP ADC / SA tokens                 |
| Compliance lever         | Anthropic ZDR              | AWS BAA / GovCloud / etc.        | GCP BAA / VPC-SC / data residency   |
| MCP / tools              | yes                        | not yet                          | not yet                             |
| Session resume           | yes                        | no (client-side transcript)      | no (client-side transcript)         |
| Cost via SDK             | yes                        | yes (token-count estimate)       | yes (token-count estimate)          |
| Streaming                | yes                        | not yet                          | not yet                             |

Rule of thumb: if your data must stay in AWS or GCP, use the cloud provider. Otherwise prefer the Claude Code CLI for richer tooling support.

---

## 2. Add the dependency

Per-crate:

```toml
[dependencies]
nucel-agent-bedrock = "0.1.0"
nucel-agent-vertex  = "0.1.0"
nucel-agent-core    = "0.2.0"
tokio = { version = "1", features = ["full"] }
```

Or via the umbrella crate with feature flags:

```toml
[dependencies]
nucel-agent-sdk = { version = "0.2.0", features = ["bedrock", "vertex"] }
```

With the umbrella crate, the providers can be selected at runtime:

```rust,no_run
use nucel_agent_sdk::build_executor;

// AWS Bedrock — credentials resolved via the default AWS chain.
let bedrock = build_executor("bedrock", None);

// Vertex — pass "<project>:<region>" as the second argument.
let vertex = build_executor("vertex", Some("my-gcp-project:us-east5".into()));
```

---

## 3. Credentials

### Bedrock

Uses the default AWS provider chain (`aws_config::defaults`):

```bash
# Pick *one* of:
export AWS_ACCESS_KEY_ID=AKIA...
export AWS_SECRET_ACCESS_KEY=...
export AWS_SESSION_TOKEN=...     # if using STS
# ...or use ~/.aws/credentials, SSO, IMDS, ECS task role, etc.
export AWS_REGION=us-east-1      # Bedrock requires an explicit region
```

If no credentials resolve, `executor.availability()` returns `available = false` with a reason. `spawn()` still attempts the call so the SDK error reaches your code.

### Vertex

Uses Google Application Default Credentials via `gcp_auth`:

```bash
# Service account file (recommended for CI):
export GOOGLE_APPLICATION_CREDENTIALS=/etc/secrets/sa.json

# Or user creds for local dev:
gcloud auth application-default login

# In GCE/GKE workloads, the metadata server is used automatically.
```

`VertexExecutor::with_adc(...)` returns `AgentError::Config` when credentials cannot be resolved — that's a hard error, not a deferred one, because Vertex needs a token before issuing the very first request.

---

## 4. Spawn + multi-turn

```rust,no_run
use nucel_agent_bedrock::BedrockExecutor;
use nucel_agent_core::{AgentExecutor, SpawnConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> nucel_agent_core::Result<()> {
    let executor = BedrockExecutor::new().await;

    let session = executor.spawn(
        Path::new("/my/repo"),
        "Summarize this codebase in one paragraph.",
        &SpawnConfig {
            model: Some("anthropic.claude-opus-4-7-20251024-v2:0".into()),
            budget_usd: Some(2.0),
            max_tokens: Some(2048),
            ..Default::default()
        },
    ).await?;

    let follow_up = session.query("Now list the three biggest TODOs.").await?;
    println!("{}", follow_up.content);

    println!("total: ${:.4}", session.total_cost().await?.total_usd);
    session.close().await?;
    Ok(())
}
```

The Vertex variant is identical — just swap `BedrockExecutor::new().await` for `VertexExecutor::with_adc("my-proj", "us-east5").await?` and the model id to a Vertex format like `claude-opus-4-7@20251024`.

### Budget enforcement

`budget_usd` is checked **before** every turn. If the cumulative cost meets or exceeds the cap, the next call returns `AgentError::BudgetExceeded` without touching the network. This is the same behaviour as the OpenCode and Claude Code providers.

### `resume()` is not supported

Neither Bedrock nor Vertex expose server-side session storage — the transcript is purely client-side. Persist the conversation history yourself and call `spawn()` again with the saved messages prepended.

---

## 5. Cost & tokens

Token counts come straight off the provider response:

- Bedrock — `output.usage().input_tokens` / `output_tokens`
- Vertex  — `usage.input_tokens` / `usage.output_tokens` (plus `cache_read_input_tokens` / `cache_creation_input_tokens` when present)

USD cost is estimated against an internal `pricing::lookup` table per model id. Unknown models fall back to `$0.00` so the rest of the response still flows through.

> Treat the USD estimate as a budget guardrail, not an invoice. The cloud provider's billing system is the source of truth.

---

## 6. Testing without real cloud calls

### Bedrock — `aws-smithy-mocks`

```rust,ignore
use aws_smithy_mocks::{mock, mock_client, RuleMode};
use aws_sdk_bedrockruntime::operation::converse::ConverseOutput as ConverseOpOutput;

let rule = mock!(aws_sdk_bedrockruntime::Client::converse)
    .then_output(|| build_op_output("hello", 42, 17));
let client = mock_client!(aws_sdk_bedrockruntime, RuleMode::Sequential, &[&rule]);
let executor = nucel_agent_bedrock::BedrockExecutor::from_client(client);
```

See `crates/bedrock/tests/bedrock_integration.rs` for full examples.

### Vertex — `wiremock`

```rust,ignore
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

let server = MockServer::start().await;
Mock::given(method("POST"))
    .and(path("/v1/projects/p/locations/us-east5/publishers/anthropic/models/claude-opus-4-7@20251024:rawPredict"))
    .respond_with(ResponseTemplate::new(200).set_body_json(...))
    .mount(&server).await;

let executor = nucel_agent_vertex::VertexExecutor::with_static_token("p", "us-east5", "tok")
    .with_api_root(server.uri());
```

See `crates/vertex/tests/vertex_integration.rs` for full examples.

---

## Limitations & roadmap

Both providers are intentionally narrow in 0.1.0:

- **No streaming** — `query_stream()` falls back to the default `query()`-then-replay path. Native `ConverseStream` (Bedrock) and SSE (Vertex) are planned for 0.2.
- **No MCP / tool bridging** — Anthropic tool blocks aren't surfaced yet; add a wire-format translation layer if you need this today.
- **No prompt caching control** — Vertex *reports* cache hits/writes when upstream provides them, but neither provider lets you author cache breakpoints yet.
- **No extended thinking** — `SpawnConfig::thinking_budget` is currently ignored.

Watch the workspace [CHANGELOG](../../CHANGELOG.md) for upgrades.
