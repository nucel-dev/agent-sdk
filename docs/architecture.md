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
└── crates/
    ├── core/                        # nucel-agent-core
    ├── claude-code/                 # nucel-agent-claude-code
    ├── codex/                       # nucel-agent-codex
    ├── opencode/                    # nucel-agent-opencode
    ├── bedrock/                     # nucel-agent-bedrock   (feature-gated)
    ├── vertex/                      # nucel-agent-vertex    (feature-gated)
    └── unified/                     # nucel-agent-sdk (umbrella)
        └── examples/                # runnable examples (cargo-registered)
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
├── nucel-agent-opencode     ──►  nucel-agent-core
├── nucel-agent-bedrock      ──►  nucel-agent-core   (optional: --features bedrock)
└── nucel-agent-vertex       ──►  nucel-agent-core   (optional: --features vertex)
```

`nucel-agent-core` has **zero** provider dependencies, so it stays small and
can be depended on by anything (including non-Anthropic / non-OpenAI providers
in the future). CI enforces this with a `check-core` job that fails the build
if `crates/core` ever gains a provider, transport, or cloud-SDK dependency.

### Provider crates at a glance

| Crate | Backend | Enabled by | Published? |
|---|---|---|---|
| `nucel-agent-claude-code` | `claude` CLI subprocess | default | yes |
| `nucel-agent-codex` | `codex` CLI subprocess | default | yes |
| `nucel-agent-opencode` | HTTP to `opencode serve` | default | yes |
| `nucel-agent-bedrock` | AWS Bedrock Runtime `Converse` | `--features bedrock` | **no** — `publish = false` |
| `nucel-agent-vertex` | Anthropic-on-Vertex `rawPredict` | `--features vertex` | **no** — `publish = false` |

`--features all-providers` turns on both cloud crates at once.

> **The two cloud crates are deliberately unpublished.** Their `budget_usd`
> guard does not work on current model ids: `pricing::lookup` matches by
> substring against a table that only knows the Claude 3.x/4.x families, so a
> current model resolves to `None`, cost accrues as `$0.00`, and the guard never
> trips. See [`known-issues/cloud-provider-pricing.md`](known-issues/cloud-provider-pricing.md).
> They remain fully usable from a path or git dependency.

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

    /// Streaming variant of `query()`. Has a default implementation, so
    /// providers only override it when they can stream for real.
    async fn query_stream(&self, prompt: &str) -> Result<EventStream> { /* … */ }

    async fn total_cost(&self) -> Result<AgentCost>;
    async fn close(&self) -> Result<()>;
}
```

`AgentSession` just forwards `query()` / `query_stream()` / `total_cost()` /
`close()` to its inner `Arc<dyn SessionImpl>` and snapshots metadata via
`metadata()`.

### `query_stream()` and the streaming contract

`query_stream()` (added in 0.2.0) returns an `EventStream` — a stream of
`MessageEvent`s delivered as they arrive rather than one response at the end.
The contract every implementation must honour:

- The stream **must** terminate with either `MessageEvent::ResultDone` or
  `MessageEvent::Error`. Consumers such as `AgentSession::collect_stream` rely
  on this to know when a turn is over.
- The turn's cost **must** be folded into the session's running total, exactly
  as `query()` does. A session driven purely through `query_stream()` has to
  report the same `total_cost()` as the equivalent `query()` sequence,
  otherwise budget guards go blind to streamed spend.

It ships with a **default implementation** so adding it was not a breaking
change for providers: the default calls `query()` and replays the result as a
single `TextChunk` followed by `ResultDone`. A provider that cannot stream
natively inherits correct-but-unstreamed behaviour and reports
`capabilities().streaming` honestly.

---

## Transport per provider

| Provider | Transport | Subprocess kept alive? |
|---|---|---|
| Claude Code | Subprocess running `claude --output-format stream-json --input-format stream-json --verbose` (interactive; prompts written to stdin) | yes (one subprocess per session for multi-turn) |
| Codex | Subprocess running `codex exec --json …` | **no** — each `query()` spawns a fresh `codex exec` |
| OpenCode | HTTP REST to `opencode serve` | n/a — stateless client |
| Bedrock | AWS SDK (`aws-sdk-bedrockruntime`) `Converse` / `ConverseStream` | n/a — no subprocess; transcript held client-side |
| Vertex | HTTPS `rawPredict` against the Anthropic-on-Vertex endpoint | n/a — no subprocess; transcript held client-side |

### Claude Code subprocess

`crates/claude-code/src/process.rs` manages a `tokio::process::Child` with:

- `stdin_writer: Option<tokio::process::ChildStdin>` — for interactive mode
- `stdout_reader: BufReader<ChildStdout>` — line-buffered JSONL
- `stderr_reader: Option<BufReader<ChildStderr>>` — for debug capture

Two entry points:

- `start_interactive()` — `claude --output-format stream-json --input-format
  stream-json --verbose [--max-turns <n>]`. No `-p`, so the CLI keeps stdin
  open and accepts prompts across turns. `spawn()` uses this, then writes the
  first prompt (and every follow-up) to stdin via `send_query`. This is the
  only spawn path for live sessions — print mode (`-p`) was removed because it
  exits after one turn and closes stdin, breaking multi-turn `query()`.
- `start_resume()` — adds `--resume <session_id>` for cross-process resume.

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

`availability()` resolves `host:port` from the base URL and does a 750 ms TCP
connect — OpenCode is a server, so reachability is the only meaningful
availability signal (there is no CLI to look for). The `reason` on failure names
the endpoint that was dialled, because callers surface it verbatim to end users.

### Bedrock and Vertex

Both are direct-API providers rather than local runtimes, which changes three
things relative to the CLI/server providers:

- **No `resume`.** There is no server-side session to reattach to; the
  conversation transcript lives in the session object on the client, so
  `capabilities().session_resume` is `false` and `resume()` starts fresh.
- **Cost is an estimate; token counts are authoritative.** Both crates report
  `AgentCost.total_usd` from a local price table, while `input_tokens` /
  `output_tokens` / cache tokens come straight off the wire. Callers needing
  exact, region-specific spend should recompute from the token counts. See the
  [pricing known-issue](known-issues/cloud-provider-pricing.md) — the current
  tables do not price current models at all.
- **Retries belong to the transport.** Bedrock delegates request-level retry to
  the AWS SDK's own retry layer, so `SpawnConfig.retry_policy` is intentionally
  not re-implemented there. Vertex, being a plain HTTP client, honours the
  policy itself like OpenCode does.

Credentials resolve from the ambient environment — the AWS default credential
chain (including EKS Pod Identity / IRSA) for Bedrock, and Application Default
Credentials for Vertex. When nothing resolves, `availability()` reports
unavailable with a reason rather than failing at first query.

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
   `nucel-agent-core = { path = "../core", version = "0.2" }` plus your transport
   crate (`reqwest`, `tonic`, etc.). Inherit the workspace MSRV with
   `rust-version.workspace = true`, or declare a higher one if your transport
   needs it.
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
   - `availability()` — actually probe (`which <cli>` for a subprocess
     provider, a short-timeout connect for a server, a credential-chain check
     for a cloud API) and return a clear `reason` when unavailable. Never
     hardcode `available: true`: `reason` is surfaced verbatim to end users by
     downstream consumers, and a provider that does not probe forces every
     caller to reimplement the probe.
   - `query_stream()` on your `SessionImpl` — override it only if you can
     stream natively, and make sure the streamed turn's cost lands in the
     session total (see the streaming contract above).
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
with provider crates. The umbrella crate `nucel-agent-sdk` pins patch versions
of the provider crates via `version = "0.2.x"` in its Cargo.toml, so a
`cargo update -p nucel-agent-sdk` brings in the right provider set.

Bumping `core` is a breaking change for **all** providers; bumping a provider
crate is local to that provider.

The MSRV is declared once in `[workspace.package]` and inherited via
`rust-version.workspace = true`, so it shows up on crates.io and docs.rs.
Raising it is a semver-visible change — re-verify with
`cargo +<new-msrv> check --all-targets` before editing the number.

---

## CI / publish

`.github/workflows/ci.yml` runs on every push to `main` and every PR:

- Format check: `cargo fmt --all -- --check`.
- Lints must be clean: `cargo clippy --workspace --all-targets -- -D warnings`.
- Tests must pass with `cargo test --workspace` on Linux and macOS.
- `check-core` — fails if `crates/core` gains a banned provider/transport
  dependency.
- `publish-check` — `cargo publish -p nucel-agent-core --dry-run` (a full
  registry-verified dry-run, meaningful because `core` has no path deps) plus
  `cargo package --workspace --no-verify` across every crate.

`.github/workflows/publish.yml` runs on a `release-*` / `v*` tag and pushes to
crates.io in dependency order:

```text
nucel-agent-core
  → nucel-agent-claude-code, nucel-agent-codex, nucel-agent-opencode
    → nucel-agent-sdk
```

Each stage waits for the previous crate's version to become visible on the
sparse index before continuing, because a dependent crate's `{ path, version }`
dependency resolves against crates.io once the path is stripped at publish
time, and the index is eventually consistent. Already-published versions are
skipped, so a re-run after a partial failure is safe.

The cloud crates are not in that list — both carry `publish = false`.

> Publishing is irreversible: a version can be yanked but never deleted or
> overwritten. Push a release tag only when the versions in the manifests are
> the ones you intend to ship.
