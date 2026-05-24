# Cost and Tokens

What `AgentCost` actually contains, how to track cumulative cost across turns
and sessions, and which providers report what.

For the hard-cap behaviour see [`budget-control.md`](budget-control.md).
This page is purely about *reading* cost data.

---

## `AgentCost` — the shape

```rust
pub struct AgentCost {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,         // 0.2.0
    pub cache_creation_tokens: u64,     // 0.2.0
    pub total_usd: f64,
}
```

| Field | What it counts | Provider notes |
|---|---|---|
| `input_tokens` | Tokens sent up to the model. | All providers. Excludes cache hits where the provider separates them out. |
| `output_tokens` | Tokens streamed back from the model. | All providers. |
| `cache_read_tokens` | Tokens served from prompt cache (cheap). | Claude only today; 0 elsewhere. |
| `cache_creation_tokens` | Tokens written to prompt cache (one-off cost). | Claude only today; 0 elsewhere. |
| `total_usd` | The dollar amount the SDK / provider attributes to this query. | All providers, but accuracy varies — see below. |

`AgentCost` implements `Add`, so it composes cleanly:

```rust
let total = costs.into_iter().fold(AgentCost::default(), |acc, c| acc + c);
```

---

## Where cost shows up

Two surfaces, depending on whether you care about *this turn* or *the whole session*:

| Surface | Type | Reflects |
|---|---|---|
| `AgentResponse::cost` (returned from `query()`) | `AgentCost` | **This single turn**. |
| `AgentSession::total_cost()` | `Result<AgentCost>` | **Cumulative session spend**. |
| `MessageEvent::ResultDone::cost` (streaming) | `AgentCost` | **This single turn**, same as `query()`'s `.cost`. |

Both surfaces are cheap to read — they're just a snapshot of the session's
counters. There's no extra network call.

---

## Tracking cumulative cost across turns

In-session is trivial — that's what `total_cost()` is for:

```rust
let session = executor.spawn(repo, prompt, &cfg).await?;

for prompt in prompts {
    let resp = session.query(prompt).await?;
    println!("turn cost: ${:.4}", resp.cost.total_usd);

    let so_far = session.total_cost().await?;
    println!("session total: ${:.4}", so_far.total_usd);
}
```

Across sessions you roll up manually. Persist `total_cost()` snapshots after
each session ends:

```rust
async fn drain(session: AgentSession, store: &dyn CostStore) -> Result<()> {
    let cost = session.total_cost().await?;
    store.record_session(session.session_id.clone(), cost.clone()).await?;
    session.close().await?;
    Ok(())
}

// Later, when reporting:
let day_total = store.sessions_today().await?
    .into_iter()
    .fold(AgentCost::default(), |acc, c| acc + c);
```

For a working multi-session example, see
[`crates/unified/examples/multi_provider_handoff.rs`](../../crates/unified/examples/multi_provider_handoff.rs)
— it fans the same prompt across all three providers and prints a comparison
table at the end.

---

## Prompt-cache accounting (Claude Code)

Anthropic's prompt-caching surfaces two extra counters:

- `cache_creation_tokens` — input tokens you **wrote** to the cache. This is
  charged at a premium (typically 1.25x the base input rate).
- `cache_read_tokens` — input tokens **served from** the cache. Charged at a
  steep discount (typically 0.1x the base input rate).

If you're using `SpawnConfig::cache_breakpoints`, watch these to confirm
caching is actually firing. A healthy long session looks like:

```
turn 1:  input=4200   output=850   cache_creation=4100  cache_read=0
turn 2:  input=120    output=1200  cache_creation=0     cache_read=4100
turn 3:  input=80     output=900   cache_creation=0     cache_read=4100
```

If `cache_read_tokens` stays at 0 across turns, your breakpoints aren't
hitting the same prefix — most often the system prompt or tools changed.

Codex and OpenCode currently leave both cache fields at zero.

---

## Per-provider accuracy

| Provider | Tokens | `total_usd` | Cache fields |
|---|---|---|---|
| Claude Code | exact (per-model `usage` block) | exact (CLI reports it) | exact |
| Codex | exact when the CLI emits `usage`; older CLI versions omit it | exact when present | n/a |
| OpenCode | exact | best-effort, depends on server pricing config | n/a |

To discover at runtime whether a provider populates token usage at all:

```rust
let caps = executor.capabilities();
if caps.token_usage {
    // safe to assume input/output tokens are real
}
if caps.prompt_caching {
    // cache_read_tokens / cache_creation_tokens are meaningful
}
```

---

## Common pitfalls

- **Don't trust `total_usd` for billing.** It's accurate enough for guardrails
  but not for invoicing — always reconcile against your provider's own usage
  API at the end of the day.
- **Costs are per-handle until you `close()`.** Two concurrent handles to the
  same `session_id` each maintain their own running counters. Persist
  `total_cost()` from each before closing.
- **`total_cost()` after `BudgetExceeded` is still safe.** The session is
  dead for queries, but the counters reflect what *was* spent before the cap
  tripped.
- **Cache reads are not free.** They're cheap — but on heavy traffic the cache
  itself can become the dominant cost line. Watch
  `cache_creation_tokens / cache_read_tokens` ratios; > 1.0 means you're
  paying to populate caches that aren't getting reused.
