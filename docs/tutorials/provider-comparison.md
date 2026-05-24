# Provider Comparison

When to reach for which provider. None of this is permanent — the SDK is
designed so you can swap with a single config change.

---

## TL;DR

| Need | Recommendation |
|---|---|
| Best-in-class coding agent today | **Claude Code** |
| OpenAI shop / GPT-5 codex | **Codex** |
| Self-hosted / air-gapped / multi-LLM | **OpenCode** |

---

## Feature matrix

| Capability | Claude Code | Codex | OpenCode |
|---|---|---|---|
| Transport | subprocess (`claude` CLI) | subprocess (`codex exec`) | HTTP (`opencode serve`) |
| Multi-turn within one process | yes — single subprocess | yes — auto-resume per turn | yes — stateless client |
| Session resume across processes | yes (`--resume`) | yes (`exec resume`) | yes (native) |
| Tool / file operations | yes | yes | yes |
| MCP support | yes | yes | yes |
| Token usage reporting | yes | yes | partial |
| Prompt caching | yes (0.2.0 surfaces it) | n/a | n/a |
| Extended thinking budget | yes | n/a | n/a |
| Self-hostable backend | no | no | **yes** |
| Pricing knobs | model + thinking budget | model + sandbox | depends on backend |

---

## Claude Code

**Use when:** you want the best raw coding-agent behavior available today and
you're OK with Anthropic pricing.

**Strengths:**

- Extremely strong on multi-step refactors and code review.
- Mature tool-use, including parallel tool calls.
- `--permission-mode` gives you fine-grained control over what the agent can
  do without asking.
- Prompt caching dramatically cheapens long sessions (0.2.0 surfaces
  `cache_read_tokens` / `cache_creation_tokens` so you can see it work).

**Watch out for:**

- A single subprocess per session means a stuck session = a stuck process.
  Always pair `budget_usd` with a sensible `max_turns` and `close()` on
  failure paths.
- The `claude` CLI must be on `$PATH` at every working dir — bake it into
  your container image.

---

## Codex (OpenAI)

**Use when:** your org standardizes on OpenAI / your billing already lives at
OpenAI, or you specifically want GPT-5-codex.

**Strengths:**

- Cheap on simple short tasks (no subprocess kept alive between turns).
- Good sandbox model (`read-only` / `workspace-write` /
  `danger-full-access`) and an `--ask-for-approval` policy.
- Resume works through the official `codex exec resume <thread_id>`
  subcommand.

**Watch out for:**

- Each turn spawns a fresh `codex exec` — has higher latency than Claude's
  long-lived subprocess on chatty sessions.
- Older versions of the CLI emit `token_usage` instead of `usage`; the SDK
  reads both, but very old binaries may emit neither.
- `--full-auto` is deprecated upstream; we map `PermissionMode::AcceptEdits`
  to `--sandbox workspace-write`.

---

## OpenCode

**Use when:** you can't ship customer data to Anthropic / OpenAI, or you
want to A/B between multiple LLM backends behind one interface.

**Strengths:**

- Runs anywhere as `opencode serve` — self-host on a GPU box, a VPS, a
  Kubernetes cluster, whatever.
- Sessions are first-class server-side resources — `resume()` is genuinely
  native.
- HTTP transport means zero subprocess overhead per turn.

**Watch out for:**

- You're now operating a server. Token usage and cost reporting depend on
  the underlying model and the server's price config.
- Network latency replaces subprocess latency — colocate the SDK client and
  the server for chat-style use cases.
- Auth is HTTP basic today; do not expose `opencode serve` to the public
  internet without a reverse proxy.

---

## Decision tree

```
Need air-gap / multi-LLM ────────────────▶ OpenCode
        │
        no
        ▼
Already paying OpenAI for codex ─────────▶ Codex
        │
        no
        ▼
                                         ▶ Claude Code (default)
```

When in doubt: start with Claude Code, profile the cost, and only switch
once you've measured the wall-clock + dollar cost on a representative task.
