# Multi-turn Sessions

Three different patterns for "keep talking to the same agent":

1. **In-process follow-ups** — `session.query()` (same process, cheapest).
2. **Bounded autonomous loops** — `max_turns` (let the agent iterate without you).
3. **Cross-process resume** — `executor.resume(session_id, ...)` (talk to a session
   created earlier, possibly by a different process).

> Quick reminder: see [`getting-started.md`](getting-started.md) for the
> basics of `spawn()`.

---

## 1. In-process follow-ups

The cheapest, most common multi-turn pattern: keep calling `query()` on the
same `AgentSession`.

```rust
let session = executor.spawn(repo, "Read the README.", &cfg).await?;

let a = session.query("What's the project's primary language?").await?;
let b = session.query("List the top-level directories.").await?;
let c = session.query("Anything you'd flag as suspicious?").await?;
```

What's happening per provider:

| Provider | Mechanism |
|---|---|
| Claude Code | Single long-lived subprocess; each `query()` writes a new JSONL line to its stdin. |
| Codex | Each `query()` re-invokes `codex exec resume <thread_id>` — multi-turn is implemented as auto-resume on the wire. |
| OpenCode | Each `query()` is an HTTP POST against the same server-side session. |

In all three cases the agent keeps the conversation context, so you don't
need to re-supply it on every turn.

---

## 2. Letting the agent iterate: `max_turns`

`SpawnConfig::max_turns` caps how many autonomous tool-use turns the model
takes before returning to you.

```rust
let cfg = SpawnConfig {
    max_turns: Some(8),       // upper bound; the agent may stop earlier
    budget_usd: Some(2.0),    // hard ceiling regardless of turns
    ..Default::default()
};
```

Rules of thumb:

- `max_turns: None` → provider default (usually quite generous).
- `max_turns: Some(1)` → effectively single-shot.
- `max_turns: Some(5..=20)` → typical for "write a small change" tasks.

`max_turns` and `budget_usd` are independent — whichever trips first wins.

---

## 3. Cross-process resume

Sometimes the process that created a session isn't the one that needs to
follow up — e.g. a background worker that hands off to another step, or a
session resumed hours later from a saved id.

```rust
// Worker A: create the session, persist the id.
let session = executor.spawn(repo, "Investigate the failure.", &cfg).await?;
db.save(&session.session_id).await?;
session.close().await?;

// Worker B (later, fresh process): resume by id.
let session_id = db.load().await?;
let resumed = executor.resume(repo, &session_id, "Now apply your suggested fix.", &cfg).await?;
let resp = resumed.query("Did the test pass?").await?;
```

### Provider quirks

| Provider | Resume support | Notes |
|---|---|---|
| Claude Code | `--resume <session_id>` | The resumed handle keeps the **same** session id, so you can keep resuming forever. |
| Codex | `codex exec resume <thread_id>` | Same — original `thread_id` is preserved. |
| OpenCode | Native — sessions live on the server. | Just keep prompting the same server-side session id. |

The runnable example
[`crates/unified/examples/codex_resume.rs`](../../crates/unified/examples/codex_resume.rs)
walks through the full save → close → resume cycle against Codex.

---

## 4. When to start fresh vs. resume

Don't reflexively resume — context accumulates and so does cost.

Start fresh when:

- The task is unrelated to the previous one.
- Context grew past ~50% of the model's window (your prompt cache will thrash).
- You want a *second opinion* on the same problem.

Resume when:

- You want the model to remember tool calls it just made.
- You're following up on a fix the model proposed.
- The cost of re-priming context outweighs the cost of carrying it.
