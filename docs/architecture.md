# Architecture

How `nucel-agent-sdk` is organized internally, and how to extend it.

> For day-to-day usage, see [`usage.md`](usage.md).

---

## Workspace layout

```text
agent-sdk/
├── Cargo.toml                       # workspace (resolver = "2")
├── README.md                        # top-level (used as readme for the umbrella crate)
├── CHANGELOG.md
├── LICENSE                          # Apache-2.0
├── docs/                            # this directory
├── examples/                        # runnable examples
└── crates/
    ├── core/                        # nucel-agent-core
    ├── claude-code/                 # nucel-agent-claude-code
    ├── codex/                       # nucel-agent-codex
    ├── opencode/                    # nucel-agent-opencode
    └── unified/                     # nucel-agent-sdk (umbrella)
```

The Cargo package name `nucel-agent-sdk` lives in `crates/unified/`.
The directory was named `unified` to disambiguate from the workspace root.

---

## Dependency graph

```text
nucel-agent-sdk (umbrella)
├── nucel-agent-core
├── nucel-agent-claude-code  ──►  nucel-agent-core
├── nucel-agent-codex        ──►  nucel-agent-core
└── nucel-agent-opencode     ──►  nucel-agent-core
```

`nucel-agent-core` has **zero** provider dependencies, so it stays small and
can be depended on by anything (including non-Anthropic / non-OpenAI providers
in the future).

---

## Core trait

Every provider implements:

```rust
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    fn executor_type(&self) -> ExecutorType;

    async fn spawn(
        &self,
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession>;

    async fn resume(
        &self,
        working_dir: &Path,
        session_id: &str,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession>;

    fn capabilities(&self) -> AgentCapabilities;
    fn availability(&self) -> AvailabilityStatus;
}
```

The trait is intentionally narrow: **spawn**, **resume**, **capabilities**,
**availability**. Provider-specific functionality lives in the provider crate
and isn't exposed through the abstraction. That keeps `nucel-agent-core` stable
and forces us to think hard before generalizing a feature.

## `AgentSession` and `SessionImpl`

`AgentSession` is the public, concrete handle returned from `spawn` / `resume`.
It owns:

- `session_id: String`
- `executor_type: ExecutorType`
- `working_dir: PathBuf`
- `created_at: DateTime<Utc>`
- `model: Option<String>`
- `inner: Arc<dyn SessionImpl>` — the provider-specific behavior

`SessionImpl` is the trait providers actually implement to drive a single
session:

```rust
#[async_trait]
pub trait SessionImpl: Send + Sync {
    async fn query(&self, prompt: &str) -> Result<AgentResponse>;
    async fn total_cost(&self) -> Result<AgentCost>;
    async fn close(&self) -> Result<()>;
}
```

`AgentSession` just forwards `query()` / `total_cost()` / `close()` to its
inner `Arc<dyn SessionImpl>` and snapshots metadata via `metadata()`.

---

## Transport per provider

| Provider | Transport | Subprocess kept alive? |
|---|---|---|
| Claude Code | Subprocess running `claude -p … --output-format stream-json --verbose` | yes (one subprocess per session for multi-turn) |
| Codex | Subprocess running `codex exec --json …` | **no** — each `query()` spawns a fresh `codex exec` |
| OpenCode | HTTP REST to `opencode serve` | n/a — stateless client |

### Claude Code subprocess

`crates/claude-code/src/process.rs` manages a `tokio::process::Child` with:

- `stdin_writer: Option<tokio::process::ChildStdin>` — for interactive mode
- `stdout_reader: BufReader<ChildStdout>` — line-buffered JSONL
- `stderr_reader: Option<BufReader<ChildStderr>>` — for debug capture

Three entry points:

- `start()` — print mode `claude -p <prompt> --output-format stream-json --verbose --max-turns <n>`
- `start_interactive()` — keeps stdin open, prompts written line-by-line
- `start_resume()` — adds `--resume <session_id>` to the above

Shutdown:

1. Drop stdin to signal EOF.
2. Send `SIGTERM` via `libc::kill`.
3. Wait up to 5 seconds; otherwise `child.kill().await`.

### Codex subprocess

`crates/codex/src/lib.rs` re-spawns `codex exec` for every `query()`.
The JSONL state machine handles:

```text
thread.started  →  turn.started  →  item.completed (agent_message)
                                  →  item.completed (reasoning | command_execution | file_change | mcp_tool_call)
                                  →  turn.completed (token_usage)
                                  →  turn.failed | error
```

Sandbox / approval mapping:

| `PermissionMode` | Codex flags |
|---|---|
| `Prompt` *(default)* | `--sandbox workspace-write` |
| `AcceptEdits` | `--full-auto` |
| `BypassPermissions` | `--dangerously-bypass-approvals-and-sandbox` |
| `RejectAll` | `--sandbox read-only` |

### OpenCode HTTP client

`crates/opencode/src/client.rs` uses `reqwest::Client` with an
`x-opencode-directory` default header. Two endpoints are used:

- `POST {base_url}/session` — create a session, returns `{ id, … }`
- `POST {base_url}/session/{id}/prompt` — body `{ parts: [{type:"text", text}], model?, system? }`

Response parsing concatenates `parts[].text` entries where `type == "text"`,
falling back to a top-level `text` field if `parts` is missing.

---

## Cost tracking

Every provider keeps an `Arc<Mutex<AgentCost>>` per session. After each query
the per-turn cost is **added** to the running total, so `session.total_cost()`
returns the cumulative spend.

`AgentCost::Add` is implemented for ergonomic accumulation.

Budget enforcement happens at two points:

1. **Pre-query**: if `total_usd >= budget` already, return
   `BudgetExceeded` without spawning anything.
2. **Post-response**: if `response.cost.total_usd > budget` (or the new total
   would overflow), return `BudgetExceeded`.

For Claude Code, `--max-budget-usd` is **also** forwarded to the CLI as a
defense-in-depth measure.

---

## Error model

All providers funnel into a single `AgentError`:

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

`Io` and `Json` get `From` impls via `thiserror`, so `?` propagation just works.

---

## Adding a new provider

1. **New crate**: `crates/my-provider/Cargo.toml`. Depend on
   `nucel-agent-core = { path = "../core", version = "0.1" }` plus your transport
   crate (`reqwest`, `tonic`, etc.).
2. **Implement `SessionImpl`** for your per-session state — `query`,
   `total_cost`, `close`. Hold any subprocess / HTTP-client state behind
   `Arc<Mutex<_>>` so the session is `Send + Sync`.
3. **Implement `AgentExecutor`** for your executor struct:
   - `executor_type()` — pick or extend `ExecutorType` (note: adding a new
     variant is a breaking change for `core`).
   - `spawn()` — create the session, run the first prompt, wrap in
     `AgentSession::new(...)`.
   - `resume()` — if your backend supports it, otherwise delegate to `spawn`
     and set `capabilities.session_resume = false`.
   - `capabilities()` — be honest about feature support.
   - `availability()` — probe quickly (e.g. `which <cli>`) and return a clear
     `reason` if unavailable.
4. **Register**:
   - Add the path to `members = [...]` in the workspace `Cargo.toml`.
   - Add it as a dep in `crates/unified/Cargo.toml` and re-export the executor
     from `crates/unified/src/lib.rs`.
   - Add a `match` arm in `build_executor()` and a string in
     `available_providers()`.
5. **Test**:
   - Unit tests: executor type, capabilities, availability, error mapping.
   - Integration tests: a mock CLI (a Rust binary written to a tempfile) or
     `wiremock` for HTTP backends.
   - E2E tests in `crates/unified/tests/` exercising the full lifecycle.
6. **Document**:
   - README with badges, install snippet, usage example, CLI mapping table,
     capabilities block, requirements.
   - Update the workspace README feature matrix.
   - Add an entry to `CHANGELOG.md`.

---

## Versioning

Each crate is versioned independently — `core` doesn't have to move in lockstep
with provider crates. The umbrella crate `nucel-agent-sdk` pins exact patch
versions of the provider crates via `version = "0.1.x"` in its Cargo.toml,
so a `cargo update -p nucel-agent-sdk` brings in the right provider set.

Bumping `core` is a breaking change for **all** providers; bumping a provider
crate is local to that provider.

---

## CI / publish

- Tests must pass with `cargo test --workspace`.
- Lints must be clean: `cargo clippy --workspace --all-targets -- -D warnings`.
- Format check: `cargo fmt --all -- --check`.
- Publish order matters: `core` → providers → `unified`.
