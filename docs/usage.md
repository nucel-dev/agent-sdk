# Usage Guide

This guide covers the day-to-day patterns for using `nucel-agent-sdk`:
spawning sessions, follow-up queries, budget control, permission modes,
and error handling.

> For high-level architecture and "how to add a provider", see
> [`architecture.md`](architecture.md).

---

## Installing

```toml
[dependencies]
nucel-agent-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
```

You do **not** need to depend on `nucel-agent-core` or the provider crates
directly unless you're implementing a new provider.

---

## Picking a provider

### Statically

```rust
use nucel_agent_sdk::{ClaudeCodeExecutor, CodexExecutor, OpencodeExecutor};

let claude   = ClaudeCodeExecutor::new();
let codex    = CodexExecutor::with_api_key(std::env::var("OPENAI_API_KEY")?);
let opencode = OpencodeExecutor::with_base_url("http://127.0.0.1:4096")
    .with_api_key("optional-token");
```

### Dynamically (from a config string)

```rust
use nucel_agent_sdk::{build_executor, AgentExecutor};

fn pick(name: &str) -> Box<dyn AgentExecutor> {
    build_executor(name, None).expect("unknown provider")
}

let exec = pick("claude-code");
```

`build_executor` accepts these names:

| Input | Provider |
|---|---|
| `"claude-code"`, `"claude_code"`, `"claudecode"` | Claude Code |
| `"codex"` | Codex |
| `"opencode"` | OpenCode (second arg is base URL) |

Anything else returns `None`. Names are case-sensitive.

---

## Spawn / query / close

```rust
use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};
use std::path::Path;

let executor = ClaudeCodeExecutor::new();

// 1. Spawn — runs the first prompt.
let session = executor.spawn(
    Path::new("/my/repo"),
    "Find and fix the failing tests",
    &SpawnConfig {
        model: Some("claude-opus-4-6".into()),
        budget_usd: Some(5.0),
        max_turns: Some(10),
        ..Default::default()
    },
).await?;

// 2. Follow-up queries reuse the same session/subprocess.
let resp = session.query("Now add regression tests").await?;
println!("{}", resp.content);

// 3. Inspect cost
let cost = session.total_cost().await?;
println!("spent ${:.4}", cost.total_usd);

// 4. Cleanup — closes subprocess / HTTP resources.
session.close().await?;
```

`session` is `Send + Sync` (its inner state is `Arc<dyn SessionImpl>`),
so you can pass it across tasks.

---

## Resuming a session

```rust
let resumed = executor.resume(
    Path::new("/my/repo"),
    "session-id-from-before",
    "Continue where we left off",
    &SpawnConfig::default(),
).await?;
```

Resume support varies by provider — check `capabilities().session_resume`:

| Provider | Resume |
|---|---|
| Claude Code | yes — uses `--resume <id>` CLI flag |
| Codex | **no** — calling `resume()` logs a warning and spawns a fresh session |
| OpenCode | yes — reuses the existing server session ID |

---

## `SpawnConfig` fields

```rust
pub struct SpawnConfig {
    pub model: Option<String>,             // "claude-opus-4-6", "gpt-5-codex", …
    pub max_tokens: Option<u32>,
    pub budget_usd: Option<f64>,           // hard USD budget for the session
    pub permission_mode: Option<PermissionMode>,
    pub env: Vec<(String, String)>,        // extra env vars for the subprocess
    pub system_prompt: Option<String>,
    pub reasoning: Option<String>,         // provider-specific reasoning effort
    pub max_turns: Option<u32>,            // autonomous turns (since 0.1.3)
}
```

Not every field is honored by every provider — see the feature matrix in the
root README or each provider's README.

---

## Budget control

`budget_usd` is enforced in **two** places for Claude Code:

1. Client-side: before every `query()` (and after the response, against the
   accumulated total).
2. CLI-side: `--max-budget-usd <amount>` is passed to the `claude` CLI.

For Codex and OpenCode, only the client-side guard applies (the CLI / server
doesn't have an equivalent flag).

Hitting the limit returns:

```rust
Err(AgentError::BudgetExceeded { limit, spent })
```

Setting `budget_usd: Some(0.0)` or any negative value returns `BudgetExceeded`
**before** the subprocess is spawned — useful for hard-stop scenarios.

---

## Permission modes

```rust
use nucel_agent_sdk::PermissionMode;

PermissionMode::Prompt              // default — prompt user for each operation
PermissionMode::AcceptEdits         // auto-approve file edits, still prompt for bash
PermissionMode::BypassPermissions   // sandbox mode — skip all permission checks
PermissionMode::RejectAll           // dry run / plan mode
```

Each provider maps these to its own native flag — see each provider README for
the exact mapping table.

---

## Error handling

`nucel_agent_sdk::AgentError` enumerates everything that can fail:

```rust
pub enum AgentError {
    Provider { provider: String, message: String },
    BudgetExceeded { limit: f64, spent: f64 },
    SessionNotFound { session_id: String },
    CliNotFound { cli_name: String },
    Config(String),
    Timeout { seconds: u64 },
    EscalationRequested,
    Io(std::io::Error),       // #[from]
    Json(serde_json::Error),  // #[from]
}
```

Common patterns:

```rust
match executor.spawn(dir, prompt, &config).await {
    Ok(session) => { /* … */ }
    Err(AgentError::CliNotFound { cli_name }) => {
        eprintln!("Please install {cli_name}");
    }
    Err(AgentError::BudgetExceeded { limit, spent }) => {
        eprintln!("Over budget: ${spent:.2} > ${limit:.2}");
    }
    Err(AgentError::Timeout { seconds }) => {
        eprintln!("Agent stalled after {seconds}s");
    }
    Err(e) => return Err(e.into()),
}
```

`AgentError` implements `std::error::Error`, so `?` propagation into
`Box<dyn Error>` (or `anyhow::Error`) works out of the box.

---

## Checking availability

```rust
let status = executor.availability();
if !status.available {
    eprintln!("provider not ready: {}", status.reason.unwrap_or_default());
    return Ok(());
}
```

- Claude Code & Codex: runs `which <cli>` — returns `available: false` if the
  CLI isn't on PATH, with an install hint.
- OpenCode: currently always returns `available: true` (the executor doesn't
  probe the network) — connection errors surface from `spawn()`.

---

## Inspecting capabilities

```rust
let caps = executor.capabilities();
if caps.session_resume    { /* … */ }
if caps.structured_output { /* … */ }
if caps.mcp_support       { /* … */ }
```

Useful for higher-level code that wants to fall back to a different provider
when a feature isn't available.
