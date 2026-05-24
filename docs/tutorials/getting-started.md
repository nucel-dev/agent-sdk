# Getting Started

A 10-minute tour: install the SDK, spawn your first session, and clean up.

> See also: [`usage.md`](../usage.md) for the full surface area,
> [`architecture.md`](../architecture.md) for how it's organized.

---

## 1. Install a provider CLI

The SDK is a thin wrapper around the upstream coding-agent CLI. You need to
install **at least one** provider before anything works:

| Provider | Install |
|---|---|
| Claude Code | `npm install -g @anthropic-ai/claude-code` |
| Codex (OpenAI) | Follow https://developers.openai.com/codex/cli/ |
| OpenCode | `brew install sst/tap/opencode` (or see https://opencode.ai) |

Verify:

```bash
claude --version
codex --version
opencode --version
```

If you only want to *try* the SDK without making any API calls, the
`build_executor` example below still works without any CLI installed.

---

## 2. Add the crate

```toml
# Cargo.toml
[dependencies]
nucel-agent-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
```

You almost certainly want the umbrella crate `nucel-agent-sdk`, not the
individual provider crates.

---

## 3. Spawn your first session

```rust
use std::path::Path;
use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executor = ClaudeCodeExecutor::new();

    // Always probe — saves you from a confusing error later.
    let avail = executor.availability();
    if !avail.available {
        eprintln!("claude CLI not available: {}", avail.reason.unwrap_or_default());
        return Ok(());
    }

    let session = executor.spawn(
        Path::new("."),                                    // working dir
        "Summarize this repo in one paragraph.",           // first prompt
        &SpawnConfig {
            // Hard cap so a runaway autonomous loop can't surprise your wallet.
            budget_usd: Some(1.0),
            // Bound autonomous turns.
            max_turns: Some(5),
            ..Default::default()
        },
    ).await?;

    println!("session_id = {}", session.session_id);

    // Follow-up query reuses the same session (cheaper, faster, more context).
    let resp = session.query("List the three most important modules.").await?;
    println!("{}", resp.content);

    let cost = session.total_cost().await?;
    println!("spent: ${:.4}", cost.total_usd);

    session.close().await?;
    Ok(())
}
```

The full runnable version is at
[`crates/unified/examples/claude_basic.rs`](../../crates/unified/examples/claude_basic.rs).

Run it with:

```bash
cargo run -p nucel-agent-sdk --example claude_basic
```

---

## 4. What just happened

1. `ClaudeCodeExecutor::new()` builds an empty executor handle. No subprocess
   yet — just the configuration.
2. `availability()` checks if the `claude` binary is on `$PATH`.
3. `spawn(...)` launches `claude -p ... --output-format stream-json --verbose`
   under the given working directory, sends the first prompt over stdin, and
   reads the first JSONL response. It returns an `AgentSession` with the
   subprocess still running.
4. `session.query(...)` writes a follow-up prompt over the same stdin and
   reads the next response — no new process.
5. `total_cost()` reflects accumulated `usage` events.
6. `close()` shuts down the subprocess gracefully.

Other providers (Codex, OpenCode) have the same `AgentExecutor` trait —
just swap the executor type.

---

## 5. Next steps

- [`multi-turn.md`](multi-turn.md) — how long sessions, `max_turns`, and
  `resume()` work.
- [`budget-control.md`](budget-control.md) — `budget_usd` and cost tracking.
- [`provider-comparison.md`](provider-comparison.md) — which provider to pick
  for which job.
- [`streaming.md`](streaming.md) — placeholder; planned for 0.2.0.
