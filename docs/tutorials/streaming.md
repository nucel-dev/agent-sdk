# Streaming

> **Status: live in 0.2.0.** Event-level streaming via `query_stream()` is now
> the primary API; `query()` is a thin wrapper that collects events into an
> `AgentResponse`.

The SDK exposes two ways to consume a model's response:

| API | Returns | Use when |
|---|---|---|
| `session.query(prompt)` | `AgentResponse` (buffered) | one-shot scripts, tests, recap CLIs |
| `session.query_stream(prompt)` | `EventStream` (Pin<Box<dyn Stream<Item = Result<MessageEvent>>>>) | terminal UIs, SSE/WebSocket bridges, anything user-facing |

`query()` is implemented in terms of `query_stream()` plus
`AgentSession::collect_stream`, so both are always source-compatible — pick
the one that matches your front-end's appetite.

---

## Quick start

```rust
use futures::StreamExt;
use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, MessageEvent, SpawnConfig};
use std::io::Write;

let executor = ClaudeCodeExecutor::new();
let session = executor.spawn(repo, "Read the README.", &SpawnConfig::default()).await?;

let mut stream = session.query_stream("Summarize it in two lines.").await?;
let mut stdout = std::io::stdout().lock();

while let Some(event) = stream.next().await {
    match event? {
        MessageEvent::TextChunk { text } => {
            stdout.write_all(text.as_bytes())?;
            stdout.flush()?;
        }
        MessageEvent::ToolUse { name, .. }     => eprintln!("[tool: {name}]"),
        MessageEvent::ResultDone { cost, .. }  => {
            println!("\n=> ${:.4}", cost.total_usd);
            break;
        }
        _ => {}
    }
}
```

See the runnable version at
[`crates/unified/examples/streaming_claude.rs`](../../crates/unified/examples/streaming_claude.rs).

---

## The `MessageEvent` enum

`MessageEvent` is `#[non_exhaustive]` — always include a `_ => {}` arm. The
variants you'll see in practice:

| Variant | Meaning | Notes |
|---|---|---|
| `TextChunk { text }` | A piece of assistant text content. | Flush as you receive — they're already chunked by the provider. |
| `ToolUse { id, name, input }` | The model started invoking a tool. | Pair with `ToolResult` via `id` if you care about latency. |
| `ToolResult { tool_use_id, success, output }` | A tool finished. | `success = false` is rendered as an error block to the model. |
| `Thinking { text }` | Extended-thinking content. | Claude only; only emitted when `SpawnConfig::thinking_budget` is set. |
| `ApiRetry { attempt, message }` | Provider retrying an upstream API call. | Useful to surface in a UI ("retry 2 / 3…"). |
| `RateLimit { message }` | Upstream rate limit hit. | `collect_stream` raises `AgentError::RateLimited`. |
| `ResultDone { cost, content, is_error }` | **Terminal**: the query is done. | Always present at end of stream. `content` is the final aggregated text. |
| `Error { message }` | **Terminal**: the query failed. | `collect_stream` raises `AgentError::Provider`. |

Every stream **MUST** end with either `ResultDone` or `Error`. If the stream
drops before either fires, the SDK returns `AgentError::StreamInterrupted`.

---

## Provider behaviour

| Provider | Native streaming? | Notes |
|---|---|---|
| Claude Code | yes | Subprocess emits JSONL events on stdout; the SDK forwards them 1:1. Best fidelity (`Thinking`, `ApiRetry`, cache stats). |
| Codex | yes | Each turn re-invokes `codex exec`; events are surfaced as they arrive. |
| OpenCode | yes | HTTP server-sent events, decoded into `MessageEvent`. `Thinking` not exposed. |

Providers that don't support a given variant simply never emit it — your match
arm just won't fire.

---

## Converting a stream back to `AgentResponse`

If you need both the streaming UX (live tokens) and a structured response at
the end, use `AgentSession::collect_stream`:

```rust
let stream = session.query_stream(prompt).await?;
let response = AgentSession::collect_stream(stream).await?;
println!("collected: {}", response.content);
println!("cost: ${:.4}", response.cost.total_usd);
```

`collect_stream` does the right thing on `RateLimit` (returns
`AgentError::RateLimited`) and `Error` (returns `AgentError::Provider`).

> **Streamed cost is accumulated.** Every provider folds a streamed turn's
> cost into the session total when the stream reaches `ResultDone`, exactly
> like the non-streaming `query()` path. So `session.total_cost()` reflects
> spend whether you drive the session with `query()`, `query_stream()`, or a
> mix — and budget guards on later calls see the running total either way.

---

## Cancellation

`EventStream` is a regular Tokio-friendly `Stream`. Drop the stream to cancel:

```rust
let mut stream = session.query_stream(prompt).await?;
tokio::select! {
    _ = drain(&mut stream)        => {}
    _ = cancel_signal.cancelled() => {
        // Drop `stream` — the provider subprocess / HTTP request is torn down.
        drop(stream);
    }
}
```

The session itself is still usable for further `query()` / `query_stream()`
calls after a cancellation — only the in-flight turn is aborted.

---

## When to prefer streaming

- **Long autonomous turns.** Show users that something is happening.
- **UI front-ends.** Every modern chat UX expects tokens to arrive live.
- **Budget enforcement.** You can drop the stream the moment cumulative cost
  crosses a threshold, instead of waiting for `query()` to settle.
- **Tool-use observability.** `ToolUse` fires before the tool runs, so you can
  surface "agent is about to run `bash -c …`" prompts in a sandbox UI.

For one-shot scripts and tests, plain `query()` is still the friendlier API.
