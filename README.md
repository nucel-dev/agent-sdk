# Nucel Agent SDK

[![License](https://img.shields.io/crates/l/nucel-agent-sdk.svg)](LICENSE)
[![nucel-agent-sdk on crates.io](https://img.shields.io/crates/v/nucel-agent-sdk.svg?label=nucel-agent-sdk)](https://crates.io/crates/nucel-agent-sdk)
[![nucel-agent-core on crates.io](https://img.shields.io/crates/v/nucel-agent-core.svg?label=nucel-agent-core)](https://crates.io/crates/nucel-agent-core)
[![docs.rs](https://img.shields.io/docsrs/nucel-agent-sdk)](https://docs.rs/nucel-agent-sdk)

Provider-agnostic Rust SDK for AI coding agents. **One trait, multiple backends.**

`nucel-agent-sdk` is a vendor-neutral abstraction for spawning coding-agent CLIs
(Claude Code, Codex, OpenCode) as subprocesses or HTTP clients. Swap providers
via a config string; the rest of your code never changes.

Part of the [Nucel](https://github.com/nucel-dev) ecosystem.

---

## Crates

| Crate | crates.io | docs.rs | Description |
|-------|-----------|---------|-------------|
| [`nucel-agent-sdk`](crates/unified) | [![crates.io](https://img.shields.io/crates/v/nucel-agent-sdk.svg)](https://crates.io/crates/nucel-agent-sdk) | [![docs.rs](https://img.shields.io/docsrs/nucel-agent-sdk)](https://docs.rs/nucel-agent-sdk) | Umbrella crate — re-exports core + all providers + `build_executor()` factory |
| [`nucel-agent-core`](crates/core) | [![crates.io](https://img.shields.io/crates/v/nucel-agent-core.svg)](https://crates.io/crates/nucel-agent-core) | [![docs.rs](https://img.shields.io/docsrs/nucel-agent-core)](https://docs.rs/nucel-agent-core) | `AgentExecutor` trait + shared types (`SpawnConfig`, `AgentSession`, `AgentResponse`, `AgentError`) |
| [`nucel-agent-claude-code`](crates/claude-code) | [![crates.io](https://img.shields.io/crates/v/nucel-agent-claude-code.svg)](https://crates.io/crates/nucel-agent-claude-code) | [![docs.rs](https://img.shields.io/docsrs/nucel-agent-claude-code)](https://docs.rs/nucel-agent-claude-code) | Subprocess wrapper for the `claude` CLI |
| [`nucel-agent-codex`](crates/codex) | [![crates.io](https://img.shields.io/crates/v/nucel-agent-codex.svg)](https://crates.io/crates/nucel-agent-codex) | [![docs.rs](https://img.shields.io/docsrs/nucel-agent-codex)](https://docs.rs/nucel-agent-codex) | Subprocess wrapper for the OpenAI `codex` CLI |
| [`nucel-agent-opencode`](crates/opencode) | [![crates.io](https://img.shields.io/crates/v/nucel-agent-opencode.svg)](https://crates.io/crates/nucel-agent-opencode) | [![docs.rs](https://img.shields.io/docsrs/nucel-agent-opencode)](https://docs.rs/nucel-agent-opencode) | HTTP client for the OpenCode server |

> Most users want the umbrella crate `nucel-agent-sdk`. Pull in individual
> provider crates only if you want to avoid compiling the providers you don't use.

---

## Quick Start

```toml
[dependencies]
nucel-agent-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executor = ClaudeCodeExecutor::new();

    // Check availability — does the `claude` CLI exist on PATH?
    let avail = executor.availability();
    if !avail.available {
        eprintln!("Not available: {:?}", avail.reason);
        return Ok(());
    }

    // Spawn a session with the first prompt.
    let session = executor.spawn(
        Path::new("/my/repo"),
        "Fix the failing tests in src/lib.rs",
        &SpawnConfig {
            model: Some("claude-opus-5".into()),
            budget_usd: Some(5.0),
            max_turns: Some(10),
            ..Default::default()
        },
    ).await?;

    // Follow-up query reuses the same session.
    let resp = session.query("Did CI pass?").await?;
    println!("Response: {}", resp.content);

    let cost = session.total_cost().await?;
    println!("Total cost: ${:.4}", cost.total_usd);

    session.close().await?;
    Ok(())
}
```

## Provider Selection at Runtime

```rust
use nucel_agent_sdk::{build_executor, available_providers};

// From a config string (e.g. `providers.agent = "claude-code"` in TOML).
let executor = build_executor("claude-code", None).unwrap();
let executor = build_executor("codex", None).unwrap();
let executor = build_executor("opencode", Some("http://localhost:4096".into())).unwrap();

// Discover what's compiled in.
for name in available_providers() {
    println!("provider: {name}");
}
```

---

## Feature Matrix

| Capability                   | Claude Code | Codex | OpenCode |
|------------------------------|:-----------:|:-----:|:--------:|
| `session_resume`             | yes         | no    | yes      |
| `token_usage`                | yes         | yes   | yes      |
| `mcp_support`                | yes         | no    | yes      |
| `autonomous_mode`            | yes         | yes   | yes      |
| `structured_output`          | no          | yes   | no       |
| Transport                    | subprocess (JSONL) | subprocess (JSONL) | HTTP REST |
| Required runtime             | `claude` CLI | `codex` CLI | `opencode serve` |
| Budget enforcement (CLI-side)| `--max-budget-usd` | client-side | client-side |
| Default permission mode      | `default` (prompt) | `workspace-write` sandbox | server-configured |

Values come directly from each provider's `capabilities()` implementation —
see `crates/*/src/lib.rs`.

### `SpawnConfig` field support

| `SpawnConfig` field | Claude Code | Codex | OpenCode |
|---------------------|:-----------:|:-----:|:--------:|
| `model`             | `--model`   | `--model` | `model.modelID` in body |
| `max_tokens`        | (no direct CLI flag) | (no direct CLI flag) | (n/a) |
| `budget_usd`        | `--max-budget-usd` + client guard | client guard | client guard |
| `permission_mode`   | `--permission-mode <mode>` | sandbox + approval flags | (server-side) |
| `env`               | yes (subprocess env) | yes (subprocess env) | (n/a) |
| `system_prompt`     | `--system-prompt` | (n/a yet) | `system` field in body |
| `reasoning`         | (provider-specific) | (provider-specific) | (n/a) |
| `max_turns`         | `--max-turns <n>` | (single-turn `codex exec`) | (server-controlled) |

---

## Architecture

```text
nucel-agent-sdk            (umbrella — re-exports + build_executor)
└── nucel-agent-core       (AgentExecutor trait, AgentSession, types, errors)
    ├── nucel-agent-claude-code  (claude CLI subprocess)
    ├── nucel-agent-codex        (codex CLI subprocess)
    └── nucel-agent-opencode     (opencode HTTP client)
```

### Core trait

```rust
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    fn executor_type(&self) -> ExecutorType;

    async fn spawn(
        &self,
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession>;

    async fn resume(
        &self,
        working_dir: &Path,
        session_id: &str,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession>;

    fn capabilities(&self) -> AgentCapabilities;
    fn availability(&self) -> AvailabilityStatus;
}
```

### Session API

```rust
let session = executor.spawn(working_dir, "fix bug", &config).await?;

// Follow-up queries on the same session
let resp = session.query("now add tests").await?;

// Accumulated cost
let cost = session.total_cost().await?;

// Clean up subprocess / HTTP resources
session.close().await?;
```

### `SpawnConfig`

```rust
pub struct SpawnConfig {
    pub model: Option<String>,             // e.g. "claude-opus-5", "gpt-5-codex"
    pub max_tokens: Option<u32>,           // upper bound on response tokens
    pub budget_usd: Option<f64>,           // session-wide USD budget
    pub permission_mode: Option<PermissionMode>,
    pub env: Vec<(String, String)>,        // extra env vars for the subprocess
    pub system_prompt: Option<String>,
    pub reasoning: Option<String>,         // provider-specific reasoning effort
    pub max_turns: Option<u32>,            // autonomous turns before returning (added in 0.1.3)
    pub retry_policy: RetryPolicy,         // transient pre-side-effect retry (network providers)
    // ... (hook_config, cache_breakpoints, thinking_budget — Claude Code only)
}
```

### `PermissionMode`

| Variant | Meaning |
|---|---|
| `Prompt` | Ask the user for each operation (default). |
| `AcceptEdits` | Auto-approve file edits, still prompt for bash. |
| `BypassPermissions` | Skip all permission checks (sandbox mode). |
| `RejectAll` | Reject all operations (dry run / plan mode). |

Each provider maps these to its own native flag — see [`docs/architecture.md`](docs/architecture.md).

---

## Reliability: automatic retries

The network providers (**Vertex** and the **OpenCode HTTP** client) automatically
retry *transient, pre-side-effect* request failures with exponential backoff.
Subprocess providers (Claude Code, Codex) delegate retrying to their own CLI.

The rule: **retry only the request-dispatch window; fatal after any side
effect.** Once the model starts streaming a `2xx` body (tokens, cost, tool
calls), errors from there on are never retried — replaying a turn that already
produced output would double-charge and duplicate side effects.

**Retried (transient):** connection errors (reset/refused/aborted/broken
pipe/DNS), request timeouts, and `429` / `502` / `503` / `504` *before any
response body is consumed*.
**Fatal (never retried):** any other `4xx`/hard `5xx`, decode/`Provider` errors
after a `2xx` body starts, and `BudgetExceeded` / `Config` / JSON errors.

```rust
use nucel_agent_sdk::{RetryPolicy, SpawnConfig};
use nucel_agent_vertex::VertexExecutor;
use std::time::Duration;

// Default policy: 3 retries, 250 ms base, exponential, capped at 8 s.
let executor = VertexExecutor::with_adc("my-gcp-project", "us-east5")
    .await?
    .with_retry_policy(RetryPolicy {
        max_retries: 5,
        base_backoff: Duration::from_millis(500),
        max_backoff: Duration::from_secs(10),
    });

// Or override per session; RetryPolicy::none() opts out.
let config = SpawnConfig {
    retry_policy: RetryPolicy::with_max_retries(2),
    ..Default::default()
};
```

Each retry is observable on the stream as a `MessageEvent::ApiRetry { attempt,
message }` event (and logged via `tracing::warn!`). See
[`docs/tutorials/retries.md`](docs/tutorials/retries.md) for the full
classification table, per-provider support, `SpawnConfig`/`ExecutorConfig`
precedence, and the
[`retry_policy`](crates/unified/examples/retry_policy.rs) +
[`vertex_with_retry`](crates/unified/examples/vertex_with_retry.rs) examples.

---

## Adding a New Provider

1. Create `crates/my-provider/` with a `Cargo.toml` that depends on `nucel-agent-core`.
2. Implement the `AgentExecutor` trait and a `SessionImpl` for your transport.
3. Add the crate to the workspace `members` list in the root `Cargo.toml`.
4. Re-export `MyProviderExecutor` from `crates/unified/src/lib.rs`.
5. Add a match arm in `build_executor()` and an entry in `available_providers()`.
6. Add unit tests (executor type, capabilities, availability) + integration tests
   (mock CLI / mock HTTP server).

See [`docs/architecture.md`](docs/architecture.md) for a deeper walkthrough.

---

## Integration with `agent-operator`

```toml
# agent-operator/Cargo.toml
[dependencies]
nucel-agent-sdk = "0.1"
```

```rust
use nucel_agent_sdk::{AgentExecutor, build_executor};

let executor = build_executor(&config.providers.agent, None)
    .ok_or("unknown agent provider")?;
let session = executor.spawn(working_dir, prompt, &spawn_config).await?;
```

---

## Examples

Runnable examples live in the umbrella crate
([`crates/unified/examples/`](crates/unified/examples)):

- [`claude_basic.rs`](crates/unified/examples/claude_basic.rs) — minimal Claude Code session
- [`claude_multiturn.rs`](crates/unified/examples/claude_multiturn.rs) — one live session, several sequential `query()` calls, cumulative cost (incl. cache tokens)
- [`codex_resume.rs`](crates/unified/examples/codex_resume.rs) — spawn, save `session_id`, resume in a new handle
- [`opencode_http.rs`](crates/unified/examples/opencode_http.rs) — point at a local `opencode serve`
- [`build_executor.rs`](crates/unified/examples/build_executor.rs) — pick a provider by name and inspect its capabilities
- [`streaming_claude.rs`](crates/unified/examples/streaming_claude.rs) — 0.2.0 `query_stream()`: print tokens as they arrive
- [`multi_provider_handoff.rs`](crates/unified/examples/multi_provider_handoff.rs) — run the same prompt across all 3 providers, compare cost
- [`with_hooks.rs`](crates/unified/examples/with_hooks.rs) — pre/post tool-use hooks (Claude Code only)
- [`budget_control.rs`](crates/unified/examples/budget_control.rs) — hit the `budget_usd` cap and handle `BudgetExceeded`
- [`resume_session.rs`](crates/unified/examples/resume_session.rs) — full spawn → save id → close → resume → continue flow
- [`retry_policy.rs`](crates/unified/examples/retry_policy.rs) — inspect the default backoff curve and transient-error classification (no network/credentials)
- [`vertex_with_retry.rs`](crates/unified/examples/vertex_with_retry.rs) — build a Vertex executor with a custom `RetryPolicy` and run a turn (needs `vertex` feature + GCP ADC)

```bash
cargo run -p nucel-agent-sdk --example claude_basic -- /path/to/repo
cargo run -p nucel-agent-sdk --example claude_multiturn -- /path/to/repo
cargo run -p nucel-agent-sdk --example streaming_claude -- /path/to/repo
cargo run -p nucel-agent-sdk --example build_executor -- claude-code
cargo run -p nucel-agent-sdk --example retry_policy
cargo run -p nucel-agent-sdk --features vertex --example vertex_with_retry -- my-gcp-project us-east5
```

---

## Docs

- [`docs/usage.md`](docs/usage.md) — Usage patterns, error handling, budget control
- [`docs/architecture.md`](docs/architecture.md) — Internals, transport details, adding providers
- [`docs/tutorials/`](docs/tutorials/) — Getting started, multi-turn, streaming, retries, hooks, cost & tokens, budget control, provider comparison
- [`CHANGELOG.md`](CHANGELOG.md) — Release notes

## License

[Apache-2.0](LICENSE)
