# Hooks

Hooks let you intercept an agent's lifecycle and react — for audit logging,
sandbox enforcement, metrics, or integration glue. They're shell commands the
provider runs at specific lifecycle points, with the event payload piped to
stdin as JSON.

> **Provider support:** Hooks are honored by **Claude Code** today. Codex and
> OpenCode accept a `HookConfig` but treat each handler as a no-op (with a
> `debug`-level log line). Check `executor.capabilities().hooks` at runtime if
> you need to gate features on real hook support.

For a runnable end-to-end example see
[`crates/unified/examples/with_hooks.rs`](../../crates/unified/examples/with_hooks.rs).

---

## The shape

```rust
pub struct HookConfig {
    pub pre_tool_use:        Option<HookHandler>,
    pub post_tool_use:       Option<HookHandler>,
    pub on_stop:             Option<HookHandler>,
    pub user_prompt_submit:  Option<HookHandler>,
}

pub struct HookHandler {
    pub command: String,            // shell command — receives JSON on stdin
    pub matcher: Option<String>,    // provider-specific tool-name regex
    pub timeout_seconds: Option<u32>,
}
```

You attach a `HookConfig` to `SpawnConfig::hook_config`. Claude Code
serializes it into a temp `--settings <json>` file the CLI consumes.

`HookHandler` has a small builder:

```rust
HookHandler::new("/usr/local/bin/audit-tool-use.sh")
    .with_matcher("Bash|Edit|Write")
    .with_timeout(5)
```

---

## Lifecycle points

| Hook | When it fires | Common use |
|---|---|---|
| `pre_tool_use` | Just before the model invokes a tool. | **Sandboxing / access control.** Exit non-zero to deny. |
| `post_tool_use` | Just after a tool returns. | **Audit logs, metrics, side-effect mirroring.** |
| `user_prompt_submit` | When the caller submits a new prompt (any `query()` call). | **Per-prompt logging, PII scrubbing, prompt-injection scans.** |
| `on_stop` | When the session ends (normal close or error). | **Cleanup, finalize traces, ship metrics.** |

### PreToolUse vs PostToolUse — when to use which

Use **PreToolUse** when:

- You need to **block** a tool call before it runs (`rm -rf`, network access,
  edits outside the workspace).
- You want to **rewrite or annotate** the tool input on the wire.
- You're enforcing a policy that's cheaper to short-circuit than to undo.
- You need a **synchronous** decision: the model waits for your command's exit
  code before proceeding.

Use **PostToolUse** when:

- You only need to **observe** what happened — audit logs, structured events,
  notifications.
- You want to **mirror side effects** elsewhere (push the diff to a queue,
  update a dashboard).
- The hook does meaningful work and you don't want to slow the model down on
  the hot path.

Both can be set at once. The typical "secure pipeline" pattern uses a
permissive `pre_tool_use` for policy and a heavier `post_tool_use` for
observability:

```rust
HookConfig {
    pre_tool_use: Some(
        HookHandler::new("/opt/agent/policy.sh")
            .with_matcher("Bash")
            .with_timeout(2),                  // fast — runs synchronously
    ),
    post_tool_use: Some(
        HookHandler::new("/opt/agent/audit.sh") // slower work OK
            .with_timeout(30),
    ),
    ..Default::default()
}
```

---

## Examples

### 1. Read-only audit log

Every tool invocation gets appended to a log line. Nothing is blocked.

```rust
let hook_config = HookConfig {
    post_tool_use: Some(HookHandler::new(
        "sh -c 'echo \"[$(date -u +%FT%TZ)] $(cat)\" >> /var/log/agent.jsonl'"
    )),
    ..Default::default()
};
```

### 2. Block writes outside the workspace

Refuses any `Edit` / `Write` whose path escapes the working dir. The script
exits non-zero, which Claude Code surfaces back to the model as a tool error.

```rust
let hook_config = HookConfig {
    pre_tool_use: Some(
        HookHandler::new("/opt/agent/deny-escapes.sh")
            .with_matcher("Edit|Write")
            .with_timeout(2),
    ),
    ..Default::default()
};
```

Inside `deny-escapes.sh`:

```sh
#!/usr/bin/env bash
set -euo pipefail
payload="$(cat)"
path="$(jq -r '.tool_input.file_path // .tool_input.path // empty' <<< "$payload")"
case "$path" in
  /workspace/*) exit 0 ;;
  *)            echo "denied: $path outside workspace" 1>&2; exit 1 ;;
esac
```

### 3. Prompt-injection scrub on the user input

Run a quick classifier on every `query()` payload. If it smells suspicious,
fail the prompt.

```rust
HookConfig {
    user_prompt_submit: Some(
        HookHandler::new("/opt/agent/promptguard")
            .with_timeout(3),
    ),
    ..Default::default()
}
```

### 4. Ship metrics on session end

```rust
HookConfig {
    on_stop: Some(HookHandler::new(
        "sh -c 'curl -fsS -X POST https://metrics/agent -d \"$(cat)\"'",
    )),
    ..Default::default()
}
```

---

## Hook payload

The JSON piped to stdin depends on the hook point. Treat field shapes as
provider-defined and use `jq` (or a small Rust binary) to extract what you
need. At time of writing Claude Code emits at least:

```json
{
  "hook":        "pre_tool_use" | "post_tool_use" | ...,
  "session_id":  "abc123",
  "tool_name":   "Bash",
  "tool_input":  { ... },
  "tool_result": { ... },   // post_tool_use only
  "prompt":      "...",     // user_prompt_submit only
  "model":       "claude-opus-5"
}
```

Don't pattern-match on exact keys — Anthropic adds fields over time. Use
`jq -r '.field // empty'` and tolerate absence.

---

## Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| Hook exits non-zero, model errors out | That's by design for `pre_tool_use`. | Either intend it (sandboxing), or fix the script. |
| `AgentError::HookFailed { hook, message }` | The handler crashed or timed out. | Check `timeout_seconds`; consider longer for slow post hooks. |
| Hook never fires | Provider doesn't support hooks (Codex/OpenCode), or matcher excludes the tool. | Check `executor.capabilities().hooks` and the matcher regex. |
| Stdin payload is empty | You forgot to `cat` it in your shell wrapper. | Always read stdin in the handler — the payload is the contract. |

---

## When NOT to use hooks

- **For business logic.** Hooks are a sidecar mechanism; building real logic
  on top of them is brittle. Prefer wrapping `query()` / `query_stream()` in
  Rust where you have type safety and async.
- **For things that need to block the response.** Hooks fire around tool use,
  not around the model's text generation. If you need to scrub the
  *response*, do it after `query()` returns (or in your stream consumer).
- **For multi-provider portability.** Codex/OpenCode no-op hooks today. If
  your enforcement needs to cover all three, put it in a wrapper layer above
  the SDK instead.
