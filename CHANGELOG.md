# Changelog

All notable changes to the Nucel Agent SDK crates are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Crate versions covered in this file:

| Crate | Latest |
|---|---|
| `nucel-agent-core` | `0.1.3` |
| `nucel-agent-claude-code` | `0.1.3` |
| `nucel-agent-codex` | `0.1.2` |
| `nucel-agent-opencode` | `0.1.2` |
| `nucel-agent-sdk` (umbrella) | `0.1.2` |

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

[0.1.3]: https://github.com/nucel-dev/agent-sdk/releases/tag/v0.1.3
[0.1.2]: https://github.com/nucel-dev/agent-sdk/releases/tag/v0.1.2
[0.1.1]: https://github.com/nucel-dev/agent-sdk/releases/tag/v0.1.1
[0.1.0]: https://github.com/nucel-dev/agent-sdk/releases/tag/v0.1.0
