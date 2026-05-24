# Streaming

> **Status: placeholder.** Event-level streaming will land in **0.2.0**.

Today (0.1.x) the SDK exposes a request/response shape:
`session.query(prompt).await -> AgentResponse`. The whole response is buffered
until the provider says it's done.

## What 0.2.0 will add

A new `SessionImpl::query_stream()` returning an
`impl Stream<Item = Result<MessageEvent>>` so callers can:

- Print assistant text as it streams (`MessageEvent::TextChunk`).
- React to tool use as it happens (`MessageEvent::ToolUse` /
  `ToolResult`).
- Surface upstream signals early (`MessageEvent::RateLimit`,
  `MessageEvent::ApiRetry`, `MessageEvent::Thinking`).
- Cancel mid-flight when a UI tab is closed or a budget is exceeded.

A sneak peek of the planned API (subject to change):

```rust
use futures::StreamExt;

let mut stream = session.query_stream("Refactor the cost calc.").await?;
while let Some(event) = stream.next().await {
    match event? {
        MessageEvent::TextChunk { text }       => print!("{text}"),
        MessageEvent::ToolUse { name, .. }     => eprintln!("(running {name})"),
        MessageEvent::ResultDone { cost, .. }  => println!("\n=> ${:.4}", cost.total_usd),
        _ => {}
    }
}
```

## Migration

`query()` won't go away — it'll be a thin wrapper over `query_stream()` that
collects events into an `AgentResponse`. Existing callers stay source
compatible.

## Tracking

Subscribe to the GitHub milestone:
https://github.com/nucel-dev/agent-sdk/milestone/1
