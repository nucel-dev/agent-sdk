# Retries & transient failures

> **Status: live.** The network providers (Vertex, OpenCode HTTP) retry
> *transient, pre-side-effect* request failures automatically. Subprocess
> providers (Claude Code, Codex) delegate retry to their own CLI.

Long-running coding-agent calls occasionally hit a transient blip — a dropped
connection while the upstream scales, a `429` rate-limit, a `503`/`502`/`504`
from a gateway that was never given the request. Retrying those is safe and
makes your integration more robust.

The SDK does this for you, but under a strict rule:

> **Retry only the request-dispatch window. Fatal after any side effect.**

Once a request has been accepted and the model has started producing output
(tokens streamed, cost incurred, a tool invoked, a file written), replaying the
whole turn would double the side effect and double-charge you. So the moment the
SDK starts reading a `2xx` response body, every error from there on is *fatal*
and is never retried.

---

## Which errors are retried

Classification lives in
[`nucel_agent_core::retry`](../../crates/core/src/retry.rs)
(`is_transient` + `RetryPolicy`). A failure is **transient** (retryable) only
when it happened *before* any response body was consumed:

| Situation | `AgentError` variant | Retried? |
|---|---|:---:|
| Connection reset / refused / aborted / broken pipe / DNS | `Io(...)` (transient `ErrorKind`) | yes |
| Request timed out with no response | `Timeout` | yes |
| `429 Too Many Requests` (before body) | `RateLimited` | yes |
| `502 Bad Gateway` / `503 Service Unavailable` / `504 Gateway Timeout` (before body) | `RateLimited` | yes |
| Any other `4xx` / hard `5xx` | `Provider` | **no — fatal** |
| Failure *after* a `2xx` body started (decode error, mid-stream) | `Provider` | **no — fatal** |
| `BudgetExceeded`, `Config`, JSON-decode | (respective) | **no — fatal** |

A `Provider` error is treated as fatal on purpose: it may already reflect
partially-applied work. Config/budget/JSON errors fail identically on every
attempt, so retrying them just wastes time.

`Io` errors are filtered by `ErrorKind`: only genuine connection-level failures
(`ConnectionReset`, `ConnectionRefused`, `ConnectionAborted`, `NotConnected`,
`BrokenPipe`, `TimedOut`, `Interrupted`, `WouldBlock`, `UnexpectedEof`) are
transient. A `NotFound` won't fix itself, so it's fatal.

---

## Configuring `RetryPolicy`

```rust
pub struct RetryPolicy {
    pub max_retries: u32,        // attempts AFTER the first; 0 disables retrying
    pub base_backoff: Duration,  // wait before the first retry
    pub max_backoff: Duration,   // cap on a single backoff interval
}
```

Backoff is exponential and deterministic (no jitter): the wait before retry `n`
is `base_backoff * 2^n`, clamped to `max_backoff`.

```rust
use nucel_agent_sdk::RetryPolicy;
use std::time::Duration;

// The default: 3 retries, 250 ms base, capped at 8 s.
let policy = RetryPolicy::default();
assert_eq!(policy.backoff_for(0), Duration::from_millis(250));
assert_eq!(policy.backoff_for(1), Duration::from_millis(500));
assert_eq!(policy.backoff_for(2), Duration::from_millis(1000));

// A custom curve.
let aggressive = RetryPolicy {
    max_retries: 5,
    base_backoff: Duration::from_millis(500),
    max_backoff: Duration::from_secs(10),
};

// Keep the default curve, change only the count.
let two = RetryPolicy::with_max_retries(2);

// Opt out entirely.
let off = RetryPolicy::none();
```

The default is deliberately conservative — coding-agent calls are long and
expensive, so the goal is to ride out a blip, not hammer a struggling endpoint.

---

## Wiring a policy into a provider

### Vertex

```rust
use nucel_agent_sdk::RetryPolicy;
use nucel_agent_vertex::VertexExecutor;
use std::time::Duration;

let executor = VertexExecutor::with_adc("my-gcp-project", "us-east5")
    .await?
    .with_retry_policy(RetryPolicy {
        max_retries: 5,
        base_backoff: Duration::from_millis(500),
        max_backoff: Duration::from_secs(10),
    });
```

### OpenCode HTTP

```rust
use nucel_agent_sdk::RetryPolicy;
use nucel_agent_opencode::OpencodeExecutor;

let executor = OpencodeExecutor::with_base_url("http://localhost:4096")
    .with_retry_policy(RetryPolicy::with_max_retries(2));
```

### Per-session override via `SpawnConfig`

Both the executor-level builder (`with_retry_policy`) and the per-session
`SpawnConfig::retry_policy` field are honored. When `SpawnConfig::retry_policy`
differs from `RetryPolicy::default()` it wins for that session; otherwise the
executor-level policy applies. The field is additive — existing
`..Default::default()` construction is unaffected.

```rust
use nucel_agent_sdk::{RetryPolicy, SpawnConfig};

let config = SpawnConfig {
    retry_policy: RetryPolicy::with_max_retries(1),
    ..Default::default()
};
```

`ExecutorConfig` carries the same `retry_policy` field for providers built from
config.

### Subprocess providers

Claude Code and Codex run as subprocesses and **delegate retrying to their own
CLI** — the `retry_policy` field is ignored for them. Configure retries through
the CLI itself if needed.

---

## Observing retries via `ApiRetry`

When a transient failure is retried, the provider emits a
`MessageEvent::ApiRetry { attempt, message }` on the streaming channel
(`query_stream()`), so you can surface it in a UI or log it. `attempt` is
1-based (the first retry is `attempt = 1`). Each retry is also logged via
`tracing::warn!` with `attempt` and `backoff_ms` fields.

```rust
use futures::StreamExt;
use nucel_agent_sdk::MessageEvent;

let mut stream = session.query_stream("Refactor this module.").await?;
while let Some(event) = stream.next().await {
    match event? {
        MessageEvent::ApiRetry { attempt, message } => {
            eprintln!("transient failure, retry #{attempt}: {message}");
        }
        MessageEvent::TextChunk { text } => print!("{text}"),
        MessageEvent::ResultDone { cost, .. } => {
            println!("\ndone — ${:.4}", cost.total_usd);
        }
        _ => {}
    }
}
```

---

## Examples

- [`retry_policy.rs`](../../crates/unified/examples/retry_policy.rs) —
  provider-agnostic: print the default backoff curve and inspect which errors
  the policy classifies as transient. No CLI, network, or credentials needed.

  ```bash
  cargo run -p nucel-agent-sdk --example retry_policy
  ```

- [`vertex_with_retry.rs`](../../crates/unified/examples/vertex_with_retry.rs) —
  build a Vertex executor with a custom `RetryPolicy`, run a turn, handle the
  result. Requires the `vertex` feature and GCP ADC.

  ```bash
  cargo run -p nucel-agent-sdk --features vertex --example vertex_with_retry -- my-gcp-project us-east5
  ```
