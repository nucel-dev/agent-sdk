# Changelog

All notable changes to the Nucel Agent SDK crates are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Crate versions covered in this file:

| Crate | Latest |
|---|---|
| `nucel-agent-core` | `0.2.0` |
| `nucel-agent-claude-code` | `0.2.0` |
| `nucel-agent-codex` | `0.2.0` |
| `nucel-agent-opencode` | `0.2.0` |
| `nucel-agent-sdk` (umbrella) | `0.2.0` |

---

## [0.2.0] — 2026-05-24

**Breaking release.** Closes audit gaps G37–G40 (hooks, streaming, prompt
cache control, extended thinking). Bumps all five crates from 0.1.x to
0.2.0 together. Downstream consumers must:

- Wildcard their `match` arms on `AgentError` (now `#[non_exhaustive]`).
- Construct `AgentCost` with `..Default::default()` (two new fields).
- Construct `AgentCapabilities` with `..Default::default()` (four new
  fields) — or fill them explicitly.
- Construct `SpawnConfig` with `..Default::default()` (three new fields).

### Added — `nucel-agent-core` 0.2.0

- **Streaming API** — `SessionImpl::query_stream()` returns `impl Stream<
  Item = Result<MessageEvent>>`. A default implementation collects the
  output of `query()` for back-compat; providers override with native
  event-by-event streaming.
- `MessageEvent` enum (`#[non_exhaustive]`) — `TextChunk`, `ToolUse`,
  `ToolResult`, `ApiRetry`, `RateLimit`, `Thinking`, `ResultDone`,
  `Error`. Tagged on `type` for JSON wire format.
- `EventStream` boxed-stream alias.
- `AgentSession::query_stream()` + `AgentSession::collect_stream()`
  convenience.
- `HookConfig` / `HookHandler` types — `pre_tool_use`, `post_tool_use`,
  `on_stop`, `user_prompt_submit`. Plumbed through `SpawnConfig.hook_config`.
- `CachePoint` type + `SpawnConfig.cache_breakpoints: Vec<CachePoint>`
  for Anthropic-style prompt-cache control.
- `SpawnConfig.thinking_budget: Option<u32>` (extended thinking tokens).
- `AgentCost.cache_read_tokens` + `cache_creation_tokens` fields with
  `#[serde(default)]` for forward-compat reads.
- `AgentCapabilities` gains `streaming`, `hooks`, `prompt_caching`,
  `extended_thinking` booleans + a `Default` impl.
- New `AgentError` variants: `StreamInterrupted(String)`,
  `RateLimited { message }`, `HookFailed { hook, message }`. Enum is now
  `#[non_exhaustive]`.

### Added — `nucel-agent-claude-code` 0.2.0

- Native `query_stream()` — parses `claude --output-format stream-json`
  line-by-line and emits `TextChunk` / `ToolUse` / `ToolResult` /
  `Thinking` / `RateLimit` / `ResultDone` as they arrive (no buffering
  until completion).
- `SpawnConfig.hook_config` → serialized into Claude Code's
  `settings.json` `hooks` schema and passed via `--settings <json>`.
- `SpawnConfig.thinking_budget` → `--thinking-budget-tokens <n>`.
- Cache stats from `usage.cache_read_input_tokens` /
  `cache_creation_input_tokens` populate `AgentCost.cache_read_tokens` /
  `cache_creation_tokens`.
- `capabilities.streaming = true`, `hooks = true`,
  `prompt_caching = true`, `extended_thinking = true`.

### Added — `nucel-agent-codex` 0.2.0

- Native `query_stream()` — drives `codex exec --json` and emits
  `TextChunk` for each `item.completed` agent message, terminating with
  `ResultDone` carrying token usage from `turn.completed`.
- `capabilities.streaming = true`. Hooks, prompt caching, extended
  thinking remain `false` (not supported by Codex CLI).

### Added — `nucel-agent-opencode` 0.2.0

- Native `query_stream()` — opens `GET /event` SSE alongside
  `POST /session/{id}/prompt`, forwards `message.part.updated` text /
  tool / reasoning events as they arrive, terminates with `ResultDone`
  from the prompt response.
- `capabilities.streaming = true`. Other 0.2.0 features remain
  `false` (not supported by OpenCode server today).

### Changed — `nucel-agent-sdk` 0.2.0

- Re-exports the new public types: `MessageEvent`, `EventStream`,
  `HookConfig`, `HookHandler`, `CachePoint`, `SessionImpl`.
- Provider dep pins bumped to 0.2.0 across the board.

## [0.1.4 / 0.1.3] — 2026-05-24

Cross-crate release driven by the SDK audit findings.

### Added — `nucel-agent-core` 0.1.4

- `PermissionMode::DontAsk` — maps to Claude Code's `dontAsk` mode (deny
  without prompting). Previously `RejectAll` was misleadingly mapped to
  `plan`, which still allows reads.
- `PermissionMode::Auto` — let the provider pick its default policy.
- `PermissionMode` is now `#[non_exhaustive]`; downstream matches must
  include a wildcard arm.

### Fixed — `nucel-agent-claude-code` 0.1.4

- `start_resume`, `start_oneshot`, and the default `start` branch no longer
  hard-code `--max-turns 1`; `SpawnConfig.max_turns` is honored and the
  flag is omitted entirely when `None` (CLI default applies).
- Stderr is now drained into a rolling 4 KiB buffer by a background task;
  the last tail is included in `AgentError::Provider` and timeout errors.
- A UUID is pre-minted client-side and passed to `--session-id <uuid>`;
  `AgentSession.session_id` is the same id, so resume round-trips.
- Stream-input shape in `send_query` now matches the documented contract:
  `{"type":"user","message":{"role":"user","content":[{"type":"text","text":"…"}]},"session_id":"…"}`
- `SIGTERM` shutdown is guarded behind `#[cfg(unix)]`; non-unix targets
  fall back to `child.start_kill()`.
- `SpawnConfig.reasoning` is now wired to `--effort <val>` (previously
  ignored).
- `permission_mode_to_cli` adds `DontAsk → "dontAsk"` and `Auto → "default"`;
  `RejectAll → "plan"` kept as a legacy alias.

### Fixed — `nucel-agent-codex` 0.1.3

- `permission_to_codex_args(AcceptEdits)` now uses `--sandbox workspace-write`
  instead of the deprecated `--full-auto` (which prints a warning upstream).
- `resume()` is implemented via `codex exec resume <thread_id> --cd <wd>
  <prompt>` instead of silently spawning a new session.
- `AgentSession.session_id` is now the upstream `thread_id` (captured from
  `thread.started`), so callers can actually resume.
- Stderr is drained by a background task to avoid pipe-buffer deadlock and
  the tail is included in error messages.
- Timeouts now kill the child instead of waiting forever in `child.wait()`.
- `--color never` is passed unconditionally to keep ANSI escapes out of
  stderr captures.
- `turn.completed` token parsing now prefers the canonical `usage` key
  with `token_usage` as a legacy fallback (was previously inverted).
- Removed the misleading `CODEX_API_KEY` env injection; only
  `OPENAI_API_KEY` is set.
- `capabilities.session_resume = true`; `capabilities.structured_output =
  false` (no `--output-schema` wiring yet).

### Fixed — `nucel-agent-opencode` 0.1.3

- The `api_key` parameter is no longer dropped — it is sent as the HTTP
  basic-auth password (default username `opencode`, overridable via
  `OPENCODE_SERVER_USERNAME`/`OPENCODE_SERVER_PASSWORD`).
- `info.tokens.{input,output}` (v2) and top-level `tokens.{input,output}`
  (legacy) are now parsed into `AgentCost.input_tokens` /
  `output_tokens`.
- Model body splits on `/` into `{providerID, modelID}` per the v2 SDK
  contract; falls back to `{modelID}` when no `/` is present.
- `directory` is now sent as `?directory=<path>` query string; the legacy
  `x-opencode-directory` header is preserved for back-compat.
- `reqwest::Client` is hoisted to session scope so HTTP keep-alive
  actually works across queries.
- `resume()` returns the OpenCode session id instead of a fresh UUID.
- `close()` best-effort `POST /session/{id}/abort` so server-side work is
  cancelled.
- `AgentSession.session_id` is the actual server session id (was a fresh
  client-side UUID).

### Changed — `nucel-agent-sdk` 0.1.4

- Re-pinned provider deps to the new versions (core 0.1.4, claude-code
  0.1.4, codex 0.1.3, opencode 0.1.3).

---

## [0.1.3] — 2026-05-22

Affects: `nucel-agent-core`, `nucel-agent-claude-code`.

### Added

- `SpawnConfig.max_turns: Option<u32>` — lets callers control how many
  autonomous turns the agent runs before returning. Maps to the
  `claude --max-turns <n>` flag in the Claude Code provider.
  Defaults to `1` (single-shot) for backward compatibility.

Commit: `29f529e` — *feat: add max_turns to SpawnConfig; bump to 0.1.3*

---

## [0.1.2] — 2026-03-23

Affects: all crates.

### Changed — Claude Code

- Switched from the deprecated `--dangerously-skip-permissions` to the
  official `--permission-mode <mode>` flag.
- `PermissionMode::AcceptEdits` → `--permission-mode acceptEdits`
- `PermissionMode::BypassPermissions` → `--permission-mode bypassPermissions`
- `PermissionMode::RejectAll` → `--permission-mode plan`
- `PermissionMode::Prompt` → `--permission-mode default`

### Added — Claude Code

- `--max-budget-usd <amount>` is now forwarded to the CLI for server-side
  budget enforcement (in addition to the client-side guard).
- `--resume <session_id>` support — `capabilities.session_resume` is now `true`.
- New internal `start_interactive()` mode (subprocess stays alive, prompts
  written to stdin) for multi-turn flows.
- New internal `start_resume()` for the resume path.

### Changed — Codex

- Switched from `--experimental-json` to the official **`--json`** flag.
- Re-implemented the JSONL state machine for the official event sequence:
  `thread.started` → `turn.started` → `item.completed` → `turn.completed`.
- Added `--sandbox {workspace-write|read-only|danger-full-access}` mapping
  from `PermissionMode`.
- Added `--full-auto` for `PermissionMode::AcceptEdits`.
- Added `--skip-git-repo-check` so the provider works in any working dir.
- Added `--cd <working_dir>` for explicit working directory.

### Fixed — Codex

- Use `OPENAI_API_KEY` as the primary env var (also sets `CODEX_API_KEY`
  for `codex exec` compatibility).

### Other

- Reduced `keywords` arrays to ≤ 5 entries to satisfy crates.io constraints.
- Fixed `unified` crate dev-dependencies.

Commits: `0966302`, `5f89be5`.

---

## [0.1.1] — 2026-03-23

### Added

- Per-crate `README.md` files for `core`, `claude-code`, `codex`, `opencode`.
- crates.io metadata across all crates:
  `repository`, `homepage`, `documentation`, `readme`,
  `keywords`, `categories`.

Commit: `fd51642` — *docs: add README to each crate, metadata for crates.io,
bump to 0.1.1*.

---

## [0.1.0] — 2026-03-22

Initial public scaffold.

### Added

- `nucel-agent-core` — `AgentExecutor` trait, `AgentSession`, `SessionImpl`,
  `SpawnConfig`, `ExecutorConfig`, `AgentCapabilities`, `AvailabilityStatus`,
  `AgentResponse`, `AgentCost`, `ToolCall`, `ToolResult`, `ExecutorType`,
  `PermissionMode`, `SessionMetadata`, `AgentError`.
- `nucel-agent-claude-code` — `claude` CLI subprocess wrapper with streaming
  JSONL parsing, budget enforcement, timeout, and graceful shutdown.
- `nucel-agent-codex` — initial `codex exec` subprocess wrapper.
- `nucel-agent-opencode` — initial HTTP client for `opencode serve`.
- `nucel-agent-sdk` (umbrella) — re-exports + `build_executor()` factory and
  `available_providers()`.
- Comprehensive test suites:
  - Core type / session / error tests
  - Claude Code protocol parser tests against real CLI output
  - Codex parser + executor edge cases
  - OpenCode HTTP integration tests via `wiremock` (incl. resume)
  - Unified SDK factory + provider tests
  - End-to-end tests with mock repos and full session lifecycle

Commits: `2b24ac3`, `2d98190`, `0113dd0`, `6a340e0`, `b867bb4`, `b240bfd`,
`c299b2f`, `77b01c9`, `c9413ce`.

---

[0.2.0]: https://github.com/nucel-dev/agent-sdk/releases/tag/v0.2.0
[0.1.3]: https://github.com/nucel-dev/agent-sdk/releases/tag/v0.1.3
[0.1.2]: https://github.com/nucel-dev/agent-sdk/releases/tag/v0.1.2
[0.1.1]: https://github.com/nucel-dev/agent-sdk/releases/tag/v0.1.1
[0.1.0]: https://github.com/nucel-dev/agent-sdk/releases/tag/v0.1.0
