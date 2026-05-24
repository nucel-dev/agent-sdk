# Budget Control

Coding agents can spend real money quickly: a single autonomous session that
hits a search-the-internet tool plus a re-read-everything loop can burn
several dollars in minutes. This guide covers the two primary brakes the SDK
gives you — `budget_usd` and `total_cost()` — and when each matters.

---

## `budget_usd` — the hard ceiling

`SpawnConfig::budget_usd` is the **dollar cap for a single session**. The SDK
checks it before every `query()` and at the end of each response; when the
accumulated `AgentCost::total_usd` for the session passes the cap, the next
operation returns:

```rust
AgentError::BudgetExceeded { limit, spent }
```

That's a terminal error for the session — subsequent `query()` calls will
keep returning it. Call `close()` and move on.

```rust
use nucel_agent_sdk::{AgentExecutor, AgentError, ClaudeCodeExecutor, SpawnConfig};

let executor = ClaudeCodeExecutor::new();
let session = executor.spawn(
    Path::new("."),
    "Refactor the cost module.",
    &SpawnConfig {
        budget_usd: Some(2.50),  // $2.50 hard cap
        max_turns: Some(15),
        ..Default::default()
    },
).await?;

match session.query("Did the tests pass?").await {
    Ok(resp)                                  => println!("{}", resp.content),
    Err(AgentError::BudgetExceeded { spent, limit }) => {
        eprintln!("session stopped: spent ${spent:.2} of ${limit:.2}");
    }
    Err(e) => return Err(e.into()),
}
```

### Edge cases

- `budget_usd: None` (default) → no SDK-enforced cap. The provider's own
  defaults still apply.
- `budget_usd: Some(0.0)` → `BudgetExceeded` returned from `spawn()` itself,
  before any tokens are spent. Useful as a "dry run".
- Cost is checked **between** turns, not in the middle of a single turn. A
  single very long autonomous turn could overshoot. Pair `budget_usd` with a
  conservative `max_turns` if you're paranoid.

---

## `total_cost()` — live cost tracking

Every session maintains a running `AgentCost`:

```rust
pub struct AgentCost {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_usd: f64,
    // ... cache fields land in 0.2.0
}
```

Sample it as often as you like — it's cheap:

```rust
let cost = session.total_cost().await?;
metrics::counter!("agent.tokens.input").increment(cost.input_tokens);
metrics::gauge!("agent.session.spend_usd").set(cost.total_usd);
```

Tips:

- `total_cost()` reflects what the **upstream provider reported** in its
  `usage` blocks. If the CLI omits usage, the counters stay at zero (Codex's
  older CLI versions did this).
- Use `AgentCost::add` to roll up across multiple sessions:
  ```rust
  let day_total = sessions.iter().fold(AgentCost::default(), |acc, c| acc + c.clone());
  ```

---

## When to cap

| Scenario | Suggested cap |
|---|---|
| Unit-test stub / CI integration | `Some(0.10)` |
| Quick "summarize this file" task | `Some(0.50)` |
| Multi-turn refactor in a small repo | `Some(2.00)` |
| Long autonomous run with broad tools | `Some(5.00..=10.00)` plus alerting |
| You're paying with your own credit card | Always cap. Always. |

For multi-tenant systems, also enforce a higher-level **daily / monthly
budget** outside the SDK — `budget_usd` is per-session and won't protect you
from a user who keeps spawning fresh sessions.

---

## Capability matrix

| Provider | Reports `total_usd` | Reports tokens | Notes |
|---|---|---|---|
| Claude Code | yes | yes | `modelUsage` block is per-model and most accurate. |
| Codex | yes | yes | Older `codex` versions omit usage; counters stay at zero. |
| OpenCode | partial | yes | `total_usd` is best-effort from server-side pricing. |

See `AgentExecutor::capabilities()` and the `token_usage` flag for runtime
discovery.
