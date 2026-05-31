# Changelog

All notable changes to the Nucel Agent SDK crates are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Crate versions covered in this file:

| Crate | Latest |
|---|---|
| `nucel-agent-core` | `0.2.1` |
| `nucel-agent-claude-code` | `0.2.2` |
| `nucel-agent-codex` | `0.2.2` |
| `nucel-agent-opencode` | `0.2.1` |
| `nucel-agent-bedrock` | `0.1.2` |
| `nucel-agent-vertex` | `0.1.1` |
| `nucel-agent-sdk` (umbrella) | `0.2.4` |

---

## [Unreleased]

### Added — developer-facing examples, cross-provider tests, docs (no public API change)

- **New runnable example: `bedrock_basic`** (`crates/unified/examples/bedrock_basic.rs`,
  `--features bedrock`). Demonstrates the AWS-native path end to end: build the
  executor from the default AWS credential chain, spawn + multi-turn `query`,
  read accumulated cost (incl. cache tokens), handle a post-SDK-retry
  `RateLimited`, and close. Fills the gap where Vertex had a runnable example but
  Bedrock — Nucel's AWS-native moat — did not. Registered with
  `required-features = ["bedrock"]`.
- **New cross-cutting test suite** `crates/unified/tests/cross_provider_tests.rs`
  (12 tests, no network/CLI/creds). Covers behaviour that must stay consistent
  *across* providers rather than any single adapter:
  - *Provider selection* — `build_executor` maps each base provider to the right
    `ExecutorType`; `available_providers()` stays in sync with what actually
    builds; malformed/unknown/cased strings return `None`.
  - *Cost accumulation* — `AgentCost` `Add`/`AddAssign` is associative across a
    multi-provider fold, saturates token counts (never wraps to 0 on a runaway
    session), and sums every dimension including cache tokens.
  - *Retry classification* — the shared `is_transient` / `RetryPolicy` classify a
    given error class identically regardless of which provider raised it (the
    contract that lets `build_executor` swap providers without changing retry
    behaviour), and the default backoff curve is locked deterministic.
- **Docs.rs improvements on the Bedrock path.** Expanded the `nucel-agent-bedrock`
  module docs with a "Why Bedrock" section (VPC-local, IAM/IRSA, no Anthropic
  key — the AWS-native moat) and an explicit "Sessions, cost, and retries"
  section (client-side transcripts → no `resume`; cost is an estimate while token
  counts are authoritative; retries are the AWS SDK's job). Documented EKS Pod
  Identity / IRSA credentials. Corrected the `pricing` module doc that referenced
  a non-existent `with_price_table` constructor.
- **Removed two stale duplicate examples** (`build-executor.rs`, `spawn-claude.rs`)
  that were superseded by their underscore-named, Cargo-registered equivalents
  (`build_executor`, `claude_basic`). Referenced the cloud-provider examples
  (`bedrock_basic`, `vertex_with_retry`) from the umbrella crate docs.
- **Fixed a pre-existing broken intra-doc link** in `nucel-agent-core`
  (`AgentCost`'s `AddAssign` doc linked `[`Add`]` with no path) so
  `cargo doc --all-features` is clean under `RUSTDOCFLAGS="-D warnings"`.

### Fixed — bedrock error classification + cache-token accounting (`nucel-agent-bedrock` 0.1.2)

- **Throttle / quota / overload errors now classify as `RateLimited`, not
  opaque `Provider`.** Every `Converse` failure previously collapsed into a
  single `AgentError::Provider`, so a `ThrottlingException` (throttling / account
  quota) or `ServiceUnavailableException` was indistinguishable from a hard
  validation error. A new `classify_converse_error` maps the typed `ConverseError`
  and transport-level `SdkError` variants into the SDK-wide taxonomy:
  `ThrottlingException` / `ServiceUnavailableException` → `RateLimited`;
  `ModelTimeoutException` and `SdkError::TimeoutError` → `Timeout`;
  `SdkError::DispatchFailure` / `ConstructionFailure` (request never left the
  client) → a transient `Io` error; everything else (validation, access-denied,
  model errors, decode) stays a fatal `Provider`. This makes
  `retry::is_transient` and caller-side back-off logic honest about Bedrock
  throttles, mirroring the Vertex/OpenCode/Codex classification work. Bedrock
  still delegates the request-level *retry loop* to the AWS SDK's own retry
  layer (so `SpawnConfig.retry_policy` is intentionally not re-implemented here);
  this fix only corrects error *type*.
- **Prompt-cache tokens are now captured.** `run_turn` hard-coded
  `cache_read_tokens` / `cache_creation_tokens` to `0`. Bedrock `Converse`
  returns `cacheReadInputTokens` / `cacheWriteInputTokens` in `TokenUsage` when a
  `cachePoint` is present; these are now folded into per-turn and session
  `AgentCost`, and the `prompt_caching` capability flag flips to `true`. Negative
  / absent counters clamp to `0`.
- New integration tests: `throttling_maps_to_rate_limited`,
  `service_unavailable_maps_to_rate_limited`, `cache_tokens_are_captured`
  (all via `aws-smithy-mocks`, no live AWS).

### Added — vertex cache-token regression test (`nucel-agent-vertex` 0.1.1)

- No behavior change. Added `cache_tokens_are_captured_from_usage` to lock in
  that Vertex's pass-through of Anthropic `cache_read_input_tokens` /
  `cache_creation_input_tokens` lands in `AgentCost` (previously only covered
  implicitly).

### Changed — umbrella re-export bump (`nucel-agent-sdk` 0.2.4)

- Pulls in `nucel-agent-bedrock` 0.1.2.

### Fixed — opencode streaming cost accounting (`nucel-agent-opencode` 0.2.1)

- **Streamed-turn cost is no longer dropped.** `query_stream()` ran the prompt
  request and emitted a `ResultDone` carrying that turn's cost, but never folded
  it into the session's running total. A session driven purely through
  `query_stream` therefore reported `total_cost() == 0`, and budget guards on
  subsequent calls were blind to streamed spend. The stream path now accumulates
  the streamed turn's cost (USD + input/output + cache tokens) into the session
  total, matching the non-streaming `query()` path and the claude-code / codex
  adapters. This is the same cost-accounting parity gap fixed for claude-code in
  0.2.2, now closed on OpenCode. New regression test
  `opencode_query_stream_accumulates_cost_into_session_total`.

### Fixed — codex transient-error classification (`nucel-agent-codex` 0.2.2)

- **Throttle errors now classify as `RateLimited`, not opaque `Provider`.**
  A `turn.failed` / `error` event whose message signals throttling ("rate
  limit", "quota", "429", "too many requests", "overloaded") is mapped to
  `AgentError::RateLimited` so callers — and `AgentSession::collect_stream` —
  can recognize it via `retry::is_transient`, mirroring how claude-code surfaces
  a rate-limit event and OpenCode maps a `429`. Codex remains a subprocess
  provider that delegates request-level retry to its CLI; this only fixes
  *classification*, it does not add an in-crate retry loop. Everything else
  still surfaces as a fatal `Provider` error. New unit tests
  `classify_codex_error_rate_limit_is_transient` and
  `classify_codex_error_generic_is_fatal_provider`.

### Changed — umbrella re-export bump (`nucel-agent-sdk` 0.2.3)

- Pulls in `nucel-agent-opencode` 0.2.1 and `nucel-agent-codex` 0.2.2.

### Fixed — claude-code multi-turn + cache-token accounting (`nucel-agent-claude-code` 0.2.2)

- **Multi-turn `query()` actually works now.** `ClaudeCodeExecutor::spawn`
  previously launched the CLI in print mode (`-p`), which exits after the
  first turn and closes stdin. The session's follow-up `query()` /
  `query_stream()` calls — which write the next prompt to stdin via
  `send_query` — therefore had no live process to talk to after turn one.
  `spawn` now uses the interactive path (`--input-format stream-json`, no
  `-p`) and sends the first prompt over stdin, so the subprocess stays alive
  for the whole session. This wires up the previously-unused
  `ClaudeProcess::start_interactive`.
- **Cache tokens are no longer dropped.** `read_response` (the non-streaming
  read used by `spawn` / `resume` / `query`) hard-coded `cache_read_tokens: 0`
  and `cache_creation_tokens: 0` in the returned `AgentCost`. It now
  accumulates cache-read and cache-creation tokens from `usage` events and
  the final `result`, matching what the streaming path already reported.
- **`modelUsage` fallback.** When a `result` message omits top-level
  `total_cost_usd` / `usage` but carries a per-model `modelUsage` breakdown,
  the parser now aggregates the per-model `costUSD` and token counts (summing
  across models) instead of discarding them. Authoritative top-level totals
  still take precedence when present.
- **Diagnostics.** `system/init` tool count, rate-limit `session_id`, and a
  `result` ↔ `init` session-id mismatch are now logged (previously parsed
  but unread).

### Removed — claude-code dead code

- Dropped the vestigial non-streaming code path: `ClaudeProcess::start`
  (print mode), `start_oneshot`, `read_oneshot_response`, and
  `protocol::parse_single_result`. The SDK exclusively uses the streaming
  JSONL path; these were never reached and only widened the surface. No
  public `AgentExecutor` / `SessionImpl` API changed.

### Added

- `claude_multiturn` example — one live session, several sequential `query()`
  calls, exercising the fixed multi-turn path and printing cumulative cost
  (including cache tokens).
- `nucel-agent-sdk` (umbrella) bumped to `0.2.2` to pull in claude-code
  `0.2.2`.

### Changed — audit pass: retry parity, overflow-safety, test hardening

- **Vertex retry parity.** `VertexExecutor::spawn` now honors a non-default
  `SpawnConfig.retry_policy`, overriding the executor-level policy for that
  spawn — matching the behavior OpenCode already had. Previously Vertex only
  respected the builder `with_retry_policy(...)` and silently ignored the
  per-call config field. Purely additive (the default `RetryPolicy` is
  unchanged, so existing callers are unaffected).
- **`AgentCost` overflow safety.** `AgentCost + AgentCost` now uses
  `saturating_add` for all token counts, so a very long-running cost
  accumulator can never panic on overflow in a debug build (it pins at
  `u64::MAX`). Added an `AddAssign` impl mirroring `Add` — the in-place
  accumulation idiom every provider session uses.
- **Docs.** `SpawnConfig.retry_policy` doc now states that Vertex and OpenCode
  honor a non-default per-call policy and that Bedrock relies on the AWS SDK's
  own retry layer.
- **Tests + fixes.**
  - Fixed a stale e2e test that asserted a `503` during OpenCode session
    creation surfaces as a `Provider` error — `503` is now (correctly)
    classified as transient. Split into two deterministic cases: a fatal `500`
    that surfaces immediately as a provider error, and a transient `503` that
    classifies as `RateLimited` under `RetryPolicy::none()`.
  - Fixed a no-op assertion in the Bedrock zero-budget test (`matches!(...)`
    without `assert!`).
  - New Vertex integration test: `SpawnConfig.retry_policy` overrides the
    executor default (asserts the endpoint is hit exactly once with
    `RetryPolicy::none()`).
  - New core unit tests: saturating-add overflow, `AddAssign` accumulation,
    `with_max_retries` keeps the default backoff curve, `none()` backoff is
    zero, and extra transient/fatal `io::ErrorKind` classification cases.
  - Removed dead `EventStream`/`MessageEvent` import (claude-code) and a
    vestigial `saw_terminal` flag (codex) — quieter build.

### Added — transient-retry policy (robustness)

- `nucel-agent-core::retry` module: `RetryPolicy` (max retries + exponential,
  capped backoff) and the `is_transient(&AgentError)` classifier. Both are
  re-exported from the umbrella crate (`RetryPolicy`, `is_transient`, and the
  `retry` module). Purely additive — no existing item changed.
- **Side-effect-safe by design.** Only failures in the *request-dispatch*
  window are retried: connection errors, timeouts, and `429`/`502`/`503`/`504`
  *before any response body is consumed*. The moment a `2xx` body starts
  streaming (tokens generated, cost incurred), errors are fatal and never
  replayed — so a retry can never double-charge or duplicate a completed turn.
  A generic `Provider` error is always treated as fatal for the same reason.
- `nucel-agent-vertex`: wired the policy into `VertexExecutor` (default 3
  retries / 250 ms base / 8 s cap). New builder `with_retry_policy(...)`;
  pass `RetryPolicy::none()` to opt out. `query_stream()` is now implemented
  (`capabilities().streaming == true`): it surfaces `MessageEvent::ApiRetry`
  events live while a transient request is retried, then terminates with
  `TextChunk` + `ResultDone`.
- `nucel-agent-opencode`: wired the policy through `OpencodeExecutor` /
  `OpencodeClient`. New builder `OpencodeExecutor::with_retry_policy(...)` and
  `OpencodeClient::with_retry(...)`. Both `create_session` and `prompt` retry
  the pre-side-effect window only (connect/timeout/`429`/`502`/`503`/`504`
  before any body is consumed); the request is rebuilt each attempt and the
  user-turn transcript push stays outside the loop, so nothing is duplicated.
  Retries fired during `query_stream()` are surfaced as `MessageEvent::ApiRetry`
  on the SSE stream.
- `nucel-agent-core`: `SpawnConfig` and `ExecutorConfig` gained an additive
  `retry_policy: RetryPolicy` field (defaults to `RetryPolicy::default()`, so
  existing `..Default::default()` construction is unchanged). For OpenCode a
  non-default `SpawnConfig.retry_policy` overrides the executor-level policy.
- New examples: `retry_policy` (provider-agnostic, no creds) and
  `vertex_with_retry` (feature `vertex`).
- Docs: `AvailabilityStatus` public fields documented (`#![deny(missing_docs)]`
  readiness).

> AWS Bedrock already retries transient errors inside `aws-sdk-bedrockruntime`.
> The subprocess providers (Claude Code, Codex) deliberately stay fatal: they
> delegate retry to their CLI, and replaying a spawn is high-side-effect-risk.

### Added — Bedrock + Vertex providers

Closes audit gap **G47** — first-party providers for Claude on AWS Bedrock
and GCP Vertex AI. Both ship as separate crates so consumers that don't
need cloud bindings pay zero dependency cost. Wired into the umbrella
crate behind optional features (`bedrock`, `vertex`, or `all-providers`).
Core / claude-code / codex / opencode are untouched — no version bumps.

### Added — `nucel-agent-bedrock` 0.1.0 (new crate)

- `BedrockExecutor` implements `AgentExecutor` against
  `aws-sdk-bedrockruntime::Client::converse`.
- Credentials via the default AWS provider chain
  (`aws_config::defaults(BehaviorVersion::latest())`) — env vars,
  `~/.aws/credentials`, IMDS, ECS task role, SSO.
- `BedrockExecutor::from_client(...)` for callers that want full SDK
  config control (retries, region, identity_cache, etc.) — also the
  hook used in tests.
- Token usage parsed from Bedrock invocation metadata
  (`output.usage().input_tokens` / `output_tokens`).
- Best-effort USD cost via an internal `pricing::lookup` table; covers
  Claude Opus 4.7 / 4 / Sonnet 4 / 3.5 / Haiku 4 / 3.5 / 3 and the
  cross-region inference profile prefixes. Unknown models fall back to
  `$0.00`.
- Multi-turn transcript kept client-side in `Arc<Mutex<Vec<Message>>>`.
- Budget enforced before every turn (pre-flight check returns
  `AgentError::BudgetExceeded` without touching the network).
- `resume()` returns an explanatory `AgentError::Provider` — Bedrock has
  no server-side session store.
- Tests: 12 unit + 5 integration (via `aws-smithy-mocks`) + doc test.

### Added — `nucel-agent-vertex` 0.1.0 (new crate)

- `VertexExecutor` issues HTTP POSTs against the regional
  `.../publishers/anthropic/models/<model>:rawPredict` Anthropic
  endpoint on Vertex AI.
- Pluggable auth via the `TokenProvider` trait:
  - `AdcToken::discover()` — Google Application Default Credentials
    minted into a `cloud-platform`-scoped bearer (`gcp_auth = "0.12"`).
  - `StaticToken::new(...)` — pre-minted tokens (tests, sidecar flows).
- `VertexExecutor::with_adc(project, region)`, `with_static_token(...)`,
  and `with_api_root(...)` constructors.
- Sends `anthropic_version: "vertex-2023-10-16"` and a standard
  Anthropic messages payload.
- Token usage + Anthropic cache token passthrough
  (`cache_read_input_tokens`, `cache_creation_input_tokens`).
- USD cost via per-model `pricing::lookup`.
- Maps HTTP 429 → `AgentError::RateLimited`, other non-2xx → `Provider`
  error, malformed JSON → `Provider` error.
- `resume()` returns explanatory `Provider` error (no server-side
  session store on Vertex).
- Tests: 14 unit + 6 integration (via `wiremock`) + doc test.

### Added — `nucel-agent-sdk` (umbrella)

- New optional features: `bedrock`, `vertex`, `all-providers`.
- `BedrockExecutor` and `VertexExecutor` re-exported under their
  respective feature flags.
- `build_executor` learns two new arms:
  - `"bedrock"` — defers async credential lookup to a short-lived
    nested `tokio` runtime. Async callers should construct directly.
  - `"vertex"` — `api_key_or_url` is parsed as `"<project>:<region>"`;
    returns `None` if malformed or if either field is empty.
- `available_providers()` is now feature-gated; only includes
  `bedrock` / `vertex` strings when the feature is enabled.

### Tutorial

- New `docs/tutorials/bedrock-vertex.md` covering provider selection,
  credentials, multi-turn usage, cost tracking, and wire-mocked tests.

### Constraints honored

- No changes to `core` / `claude-code` / `codex` / `opencode` crates.
- No bump of `nucel-agent-core` to 0.3.0. The new providers use the
  existing 0.2.x trait surface and reuse `ExecutorType::ClaudeCode`
  (since both Bedrock and Vertex serve Anthropic models) — adding new
  enum variants will land in a future minor of core.
- AWS / GCP credentials are optional at runtime; missing creds surface
  via `availability()` and through real SDK errors at request time.

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
