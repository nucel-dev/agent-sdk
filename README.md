# Nucel Agent SDK

[![License](https://img.shields.io/crates/l/nucel-agent-sdk.svg)](LICENSE)
[![nucel-agent-sdk on crates.io](https://img.shields.io/crates/v/nucel-agent-sdk.svg?label=nucel-agent-sdk)](https://crates.io/crates/nucel-agent-sdk)
[![nucel-agent-core on crates.io](https://img.shields.io/crates/v/nucel-agent-core.svg?label=nucel-agent-core)](https://crates.io/crates/nucel-agent-core)
[![docs.rs](https://img.shields.io/docsrs/nucel-agent-sdk)](https://docs.rs/nucel-agent-sdk)

A Rust workspace that puts one trait — `AgentExecutor` — in front of several
AI backends, so the calling code can pick a backend from a config string
instead of a `#[cfg]`.

Part of the [Nucel](https://github.com/nucel-dev) platform, but it has no
dependency on the rest of Nucel and works standalone.

---

## What is actually behind the trait

Two quite different kinds of backend share the trait, and the difference
matters more than the shared interface does:

**Coding agents.** Claude Code, Codex, and OpenCode are real agents. The SDK
starts one (as a subprocess, or over HTTP against a running server), it reads
and writes files under `working_dir`, runs commands, and can iterate over
several turns on its own.

**Plain model APIs.** AWS Bedrock and GCP Vertex AI are not agents. Each
`query()` is one request and one response. There is no tool loop, no file
access, and `AgentResponse::tool_calls` is always empty — both providers
report `capabilities().autonomous_mode == false`. They exist so you can reach
Claude inside your own AWS or GCP boundary, with IAM or ADC instead of an
Anthropic API key. If you want something to edit a repository, use one of the
three agent providers.

What the SDK adds on top of either kind: a session handle you can send
follow-up prompts to, optional event streaming, cumulative token and cost
accounting, a client-side USD budget cap, and retry handling for transient
network failures.

---

## Release status

The workspace is ahead of what has been published. Read this before picking a
dependency line.

| Crate | On crates.io | In this repo | Notes |
|---|---|---|---|
| `nucel-agent-sdk` (umbrella) | `0.2.0` | `0.2.4` | published build has **no** cargo features |
| `nucel-agent-core` | `0.2.0` | `0.2.1` | |
| `nucel-agent-claude-code` | `0.2.0` | `0.2.2` | |
| `nucel-agent-codex` | `0.2.0` | `0.2.2` | |
| `nucel-agent-opencode` | `0.2.0` | `0.2.1` | |
| `nucel-agent-bedrock` | **not published** | `0.1.2` | alpha; git dependency only |
| `nucel-agent-vertex` | **not published** | `0.1.1` | alpha; git dependency only |

Two consequences:

- The Bedrock and Vertex providers cannot be pulled from crates.io at all.
  `nucel-agent-sdk = { version = "0.2", features = ["bedrock"] }` fails —
  the published `0.2.0` declares no features. Use a git dependency.
- Work landed since the `0.2.0` publish is in this repo but not on crates.io:
  the whole `RetryPolicy` / `ApiRetry` mechanism, Bedrock throttle
  classification, prompt-cache token capture on Claude Code and Bedrock,
  interactive multi-turn for Claude Code, and a dependency refresh. See
  [`CHANGELOG.md`](CHANGELOG.md) for the detail.

---

## Crates

| Crate | Directory | Description |
|---|---|---|
| [`nucel-agent-sdk`](crates/unified) | `crates/unified/` | Umbrella — re-exports core + providers, plus the `build_executor()` factory |
| [`nucel-agent-core`](crates/core) | `crates/core/` | `AgentExecutor` / `SessionImpl` traits and the shared types (`SpawnConfig`, `AgentSession`, `AgentCost`, `AgentError`, `RetryPolicy`) |
| [`nucel-agent-claude-code`](crates/claude-code) | `crates/claude-code/` | Subprocess wrapper for the `claude` CLI |
| [`nucel-agent-codex`](crates/codex) | `crates/codex/` | Subprocess wrapper for the OpenAI `codex` CLI |
| [`nucel-agent-opencode`](crates/opencode) | `crates/opencode/` | HTTP client for an OpenCode server |
| [`nucel-agent-bedrock`](crates/bedrock) | `crates/bedrock/` | AWS Bedrock Runtime `Converse` (alpha, unpublished) |
| [`nucel-agent-vertex`](crates/vertex) | `crates/vertex/` | Claude on GCP Vertex AI `rawPredict` (alpha, unpublished) |

Most callers want the umbrella. Depend on a single provider crate if you would
rather not compile the ones you don't use, and on `nucel-agent-core` alone if
you are writing a new provider.

```text
nucel-agent-sdk (umbrella)
├── nucel-agent-core            ← zero provider dependencies
├── nucel-agent-claude-code  ──► nucel-agent-core
├── nucel-agent-codex        ──► nucel-agent-core
├── nucel-agent-opencode     ──► nucel-agent-core
├── nucel-agent-bedrock      ──► nucel-agent-core   (feature "bedrock")
└── nucel-agent-vertex       ──► nucel-agent-core   (feature "vertex")
```

---

## Install

The three agent providers, from crates.io:

```toml
[dependencies]
nucel-agent-sdk = "0.2"
tokio = { version = "1", features = ["full"] }
```

With Bedrock or Vertex — git only, until those crates are published:

```toml
[dependencies]
nucel-agent-sdk = { git = "https://github.com/nucel-dev/agent-sdk", features = ["bedrock", "vertex"] }
tokio = { version = "1", features = ["full"] }
```

Cargo features on the umbrella: `bedrock`, `vertex`, and `all-providers`
(both). All are off by default. The three agent providers are always compiled
in and are not feature-gated.

Each agent provider needs its runtime installed separately — see the matrix
below. Nothing is bundled.

---

## Quick start

```rust
use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executor = ClaudeCodeExecutor::new();

    // Is the `claude` CLI on PATH? `reason` carries the install command
    // when it isn't.
    let avail = executor.availability();
    if !avail.available {
        eprintln!("not available: {:?}", avail.reason);
        return Ok(());
    }

    // Open a session with the first prompt.
    let session = executor.spawn(
        Path::new("/my/repo"),
        "Fix the failing tests in src/lib.rs",
        &SpawnConfig {
            model: Some("claude-opus-4-6".into()),
            budget_usd: Some(5.0),
            max_turns: Some(10),
            ..Default::default()
        },
    ).await?;

    // Follow-ups reuse the same live session.
    let resp = session.query("Did the tests pass?").await?;
    println!("{}", resp.content);

    let cost = session.total_cost().await?;
    println!("spent ${:.4} over {} input / {} output tokens",
        cost.total_usd, cost.input_tokens, cost.output_tokens);

    session.close().await?;
    Ok(())
}
```

`spawn()` sends the first prompt as part of opening the session, so the first
response is already accounted for by the time you get the handle back.

---

## Choosing a provider at runtime

```rust
use nucel_agent_sdk::{available_providers, build_executor};

let claude   = build_executor("claude-code", None).unwrap();
let codex    = build_executor("codex", None).unwrap();
let opencode = build_executor("opencode", Some("http://localhost:4096".into())).unwrap();

for name in available_providers() {
    println!("{name}");
}
```

`build_executor` returns `Option<Box<dyn AgentExecutor>>` — `None` for an
unrecognised name. The second argument is overloaded per provider:

| Name | Second argument | Behaviour without it |
|---|---|---|
| `claude-code` (also `claude_code`, `claudecode`) | ignored | fine |
| `codex` | ignored | fine |
| `opencode` | base URL | defaults to `http://127.0.0.1:4096` |
| `bedrock` (feature `bedrock`) | ignored | resolves the default AWS credential chain |
| `vertex` (feature `vertex`) | `"<project>:<region>"`, e.g. `"my-proj:us-east5"` | returns `None` |

Matching is case-sensitive: `"Claude-Code"` and `"CODEX"` return `None`.
`available_providers()` reflects the features you compiled with, so it is the
right thing to show a user when their config string does not resolve.

`build_executor("bedrock", …)` and `build_executor("vertex", …)` do async
credential discovery behind a synchronous signature by driving a short-lived
current-thread runtime. That is safe to call from inside a Tokio runtime, but
it does block the calling thread, so prefer `BedrockExecutor::new().await` /
`VertexExecutor::with_adc(project, region).await` on a hot path.

---

## Provider matrix

Every value below is read straight out of each provider's `capabilities()` in
`crates/*/src/lib.rs`. Nothing here is aspirational.

| `AgentCapabilities` field | Claude Code | Codex | OpenCode | Bedrock | Vertex |
|---|:--:|:--:|:--:|:--:|:--:|
| `session_resume` | yes | yes | yes | no | no |
| `token_usage` | yes | yes | yes | yes | yes |
| `mcp_support` | yes | no | yes | no | no |
| `autonomous_mode` | yes | yes | yes | **no** | **no** |
| `structured_output` | no | no | no | no | no |
| `streaming` | yes | yes | yes | no | yes |
| `hooks` | yes | no | no | no | no |
| `prompt_caching` | yes | no | no | yes | yes |
| `extended_thinking` | yes | no | no | no | no |

| | Claude Code | Codex | OpenCode | Bedrock | Vertex |
|---|---|---|---|---|---|
| Transport | subprocess, stream-json over stdio | subprocess, `codex exec --json` | HTTP REST | AWS SDK `Converse` | HTTPS `rawPredict` |
| Runtime you must supply | `claude` CLI | `codex` CLI | a running `opencode serve` | AWS credentials | GCP ADC or a token |
| Default endpoint / model | CLI default | CLI default | `http://127.0.0.1:4096` | `anthropic.claude-opus-4-7-20251024-v2:0` | `claude-opus-4-7@20251024` |
| `budget_usd` enforcement | `--max-budget-usd` **and** a client-side guard | client-side guard | client-side guard | client-side guard | client-side guard |

Things worth knowing that a matrix cell can't carry:

- **`structured_output` is `false` everywhere.** Codex has an
  `--output-schema` flag upstream, but this SDK does not wire it yet.
- **`ExecutorType` has only three variants** (`ClaudeCode`, `Codex`,
  `OpenCode`). Bedrock and Vertex both report `ExecutorType::ClaudeCode`,
  because the enum was closed for the 0.2.x line. Do not use `executor_type()`
  to tell a Bedrock session apart from a Claude Code one.
- **`resume()` on Bedrock and Vertex returns `AgentError::Provider`.** Both
  keep their transcript client-side, so there is no session id to look up.
  Persist the transcript yourself and spawn again.
- **Hooks are Claude Code only.** Other providers accept a `HookConfig` and
  log that they ignored it.
- **Bedrock does not stream.** `query_stream()` still works there, but it
  falls back to the default implementation in `SessionImpl`, which replays the
  finished response as a single `TextChunk` followed by `ResultDone`.
- **Bedrock and Vertex report an estimated `total_usd`**, computed from
  hardcoded price tables (`crates/bedrock/src/pricing.rs`,
  `crates/vertex/src/pricing.rs`). Their token counts are exact; the dollar
  figure is not authoritative for billing — reconcile against your AWS or GCP
  invoice. Claude Code, by contrast, reports the CLI's own `total_cost_usd`.

---

## The API surface

### `AgentExecutor`

```rust
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    fn executor_type(&self) -> ExecutorType;

    async fn spawn(&self, working_dir: &Path, prompt: &str, config: &SpawnConfig)
        -> Result<AgentSession>;

    async fn resume(&self, working_dir: &Path, session_id: &str, prompt: &str, config: &SpawnConfig)
        -> Result<AgentSession>;

    fn capabilities(&self) -> AgentCapabilities;
    fn availability(&self) -> AvailabilityStatus;
}
```

`availability()` is a cheap synchronous probe — is the CLI on `PATH`, are the
credentials resolvable — and its `reason` is written to be shown to a human.
Providers do not refuse to run when it reports `false`; they let the real
error through instead.

### `AgentSession`

```rust
let session = executor.spawn(working_dir, "fix the bug", &config).await?;

session.session_id;                          // resumable id, where supported
let resp   = session.query("now add tests").await?;
let stream = session.query_stream("explain").await?;   // Stream<Item = Result<MessageEvent>>
let cost   = session.total_cost().await?;    // cumulative for the session
let meta   = session.metadata();             // cloneable, persistable snapshot
session.close().await?;                      // consumes self
```

`AgentSession::collect_stream(stream)` folds an `EventStream` back into an
`AgentResponse` if you want streaming for progress display but a single value
at the end.

### `SpawnConfig`

```rust
pub struct SpawnConfig {
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub budget_usd: Option<f64>,
    pub permission_mode: Option<PermissionMode>,
    pub env: Vec<(String, String)>,          // extra subprocess env
    pub system_prompt: Option<String>,
    pub reasoning: Option<String>,           // Claude Code: --effort
    pub max_turns: Option<u32>,
    pub hook_config: Option<HookConfig>,     // Claude Code only
    pub cache_breakpoints: Vec<CachePoint>,  // prompt caching
    pub thinking_budget: Option<u32>,        // Claude Code only
    pub retry_policy: RetryPolicy,
}
```

It derives `Default`, and every field is additive, so `..Default::default()`
keeps compiling as fields are added.

How the fields reach each backend:

| Field | Claude Code | Codex | OpenCode | Bedrock / Vertex |
|---|---|---|---|---|
| `model` | `--model` | `--model` | `{providerID, modelID}` in the body, split on `/` | body |
| `budget_usd` | `--max-budget-usd` **and** a client guard | client guard | client guard | client guard |
| `system_prompt` | `--system-prompt` | not sent | body | body |
| `max_tokens` | no CLI flag | no CLI flag | not sent | body (Vertex defaults to 4096) |
| `permission_mode` | `--permission-mode` | `--sandbox` / bypass flag | not sent | not sent |
| `max_turns` | `--max-turns` | not sent | not sent | n/a — one request per `query()` |
| `reasoning` | `--effort` | not sent | not sent | not sent |
| `thinking_budget` | `--thinking-budget-tokens` | not sent | not sent | not sent |
| `hook_config` | `--settings <json>` | ignored | ignored | ignored |
| `env` | subprocess env | subprocess env | n/a | n/a |
| `retry_policy` | delegated to the CLI | delegated to the CLI | honoured | Vertex honours it; Bedrock defers to the AWS SDK |

### `PermissionMode`

Six variants. Each provider maps them onto its own native flag:

| Variant | Claude Code | Codex |
|---|---|---|
| `Prompt` (default) | `default` | `--sandbox workspace-write` |
| `AcceptEdits` | `acceptEdits` | `--sandbox workspace-write` |
| `BypassPermissions` | `bypassPermissions` | `--dangerously-bypass-approvals-and-sandbox` |
| `RejectAll` | `plan` | `--sandbox read-only` |
| `DontAsk` | `dontAsk` | `--sandbox read-only` |
| `Auto` | `default` | `--sandbox workspace-write` |

`RejectAll` is historically misnamed: on Claude Code it maps to `plan` mode,
which still reads files. If you want deny-without-prompting, use `DontAsk`.

### Errors

`AgentError` is `#[non_exhaustive]`, so match with a `_` arm. The variants
worth handling explicitly are `BudgetExceeded { limit, spent }`,
`CliNotFound { cli_name }`, `RateLimited { message }`, `Timeout { seconds }`,
and `SessionNotFound { session_id }`. Everything provider-specific is wrapped
into `Provider { provider, message }` — provider crates do not define their
own error types.

### `MessageEvent`

What `query_stream()` yields: `TextChunk`, `ToolUse`, `ToolResult`,
`Thinking`, `ApiRetry`, `RateLimit`, and the two terminal events `ResultDone`
(carries the final `AgentCost`) and `Error`. A conforming stream always ends
with one of the two terminal events.

---

## Retries

The two network providers — Vertex and the OpenCode HTTP client — retry
transient request failures with exponential backoff. The subprocess providers
delegate retrying to their own CLI, and Bedrock delegates to the AWS SDK's
retry layer.

The rule is: **retry the request-dispatch window only, fatal after any side
effect.** Once a `2xx` body starts arriving — tokens, cost, tool calls — every
later error is fatal, because replaying a turn that already produced output
would double-charge and duplicate its side effects.

Retried: connection errors (reset, refused, aborted, broken pipe, DNS),
request timeouts, and `429` / `502` / `503` / `504` *before any response body
is consumed*. Fatal: everything else, including `Provider` and decode errors
after a body has started, plus `BudgetExceeded`, `Config`, and JSON errors.

```rust
use nucel_agent_sdk::{RetryPolicy, SpawnConfig};
use nucel_agent_vertex::VertexExecutor;
use std::time::Duration;

// Default: 3 retries, 250 ms base, doubling, capped at 8 s.
let executor = VertexExecutor::with_adc("my-gcp-project", "us-east5")
    .await?
    .with_retry_policy(RetryPolicy {
        max_retries: 5,
        base_backoff: Duration::from_millis(500),
        max_backoff: Duration::from_secs(10),
    });

// Or per session. A non-default value here wins over the executor's policy;
// RetryPolicy::none() opts out entirely.
let config = SpawnConfig {
    retry_policy: RetryPolicy::with_max_retries(2),
    ..Default::default()
};
```

Each retry surfaces on the stream as `MessageEvent::ApiRetry { attempt,
message }` and is logged at `warn`. Full classification table and the
`SpawnConfig` / `ExecutorConfig` precedence rules are in
[`docs/tutorials/retries.md`](docs/tutorials/retries.md).

---

## Examples

Runnable examples live in [`crates/unified/examples/`](crates/unified/examples).

Run without any credentials or CLI installed:

```bash
cargo run -p nucel-agent-sdk --example retry_policy
cargo run -p nucel-agent-sdk --example build_executor -- claude-code
```

| Example | What it shows | Needs |
|---|---|---|
| [`retry_policy`](crates/unified/examples/retry_policy.rs) | default backoff curve and transient-error classification | nothing |
| [`build_executor`](crates/unified/examples/build_executor.rs) | pick a provider by name, print its capabilities | nothing |
| [`claude_basic`](crates/unified/examples/claude_basic.rs) | minimal spawn → query → close | `claude` CLI |
| [`claude_multiturn`](crates/unified/examples/claude_multiturn.rs) | several `query()` calls on one live session, cumulative cost including cache tokens | `claude` CLI |
| [`streaming_claude`](crates/unified/examples/streaming_claude.rs) | `query_stream()`, printing tokens as they arrive | `claude` CLI |
| [`with_hooks`](crates/unified/examples/with_hooks.rs) | pre/post tool-use hooks | `claude` CLI |
| [`budget_control`](crates/unified/examples/budget_control.rs) | hitting the `budget_usd` cap and handling `BudgetExceeded` | `claude` CLI |
| [`resume_session`](crates/unified/examples/resume_session.rs) | spawn → save id → close → resume → continue | `claude` CLI |
| [`codex_resume`](crates/unified/examples/codex_resume.rs) | the same flow on Codex threads | `codex` CLI |
| [`opencode_http`](crates/unified/examples/opencode_http.rs) | pointing at a local `opencode serve` | `opencode serve` |
| [`multi_provider_handoff`](crates/unified/examples/multi_provider_handoff.rs) | one prompt across all three agent providers, comparing cost | all three CLIs |
| [`bedrock_basic`](crates/unified/examples/bedrock_basic.rs) | AWS-native path end to end | `--features bedrock`, AWS credentials |
| [`vertex_with_retry`](crates/unified/examples/vertex_with_retry.rs) | a Vertex executor with a custom `RetryPolicy` | `--features vertex`, GCP ADC |

```bash
cargo run -p nucel-agent-sdk --features bedrock --example bedrock_basic
cargo run -p nucel-agent-sdk --features vertex  --example vertex_with_retry -- my-gcp-project us-east5
```

---

## Build, test, lint

```bash
cargo build --workspace
cargo test  --workspace
cargo test  --workspace --all-features   # adds the bedrock/vertex build_executor arms
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The Bedrock and Vertex crates are workspace members, so `cargo test
--workspace` already covers them; `--all-features` only turns on the
umbrella's feature-gated re-exports and factory arms.

The suite needs no network, no cloud credentials, and no agent CLI: HTTP
providers are tested against `wiremock`, Bedrock against `aws-smithy-mocks`,
and the subprocess providers by asserting on constructed argument vectors and
parsed protocol fixtures rather than by launching a real CLI.

CI is GitHub Actions ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)):
fmt, clippy with `-D warnings`, build and test on `ubuntu-latest` and
`macos-latest`, plus a `publish-check` job that runs a registry-verified
`cargo publish --dry-run` for `nucel-agent-core` and
`cargo package --workspace --no-verify` for everything else. The dry run
cannot cover the downstream crates, because they depend on siblings by
`{ path, version }` and a just-bumped sibling version is not on crates.io yet.

The manifests do not declare a `rust-version`; `edition = "2024"` implies Rust
1.85 or newer, and CI builds on `stable`. Verified locally on 1.97.1.
`Cargo.lock` is gitignored, as is normal for a library workspace.

---

## Contributing

Bug fixes, docs, and perf work: open a PR against `main` and make sure `cargo
fmt --check` and `cargo clippy --all-targets -- -D warnings` pass.

Adding a provider is the case with the most moving parts, and
[`CONTRIBUTING.md`](CONTRIBUTING.md) walks through it. In outline:

1. New crate under `crates/`, depending on `nucel-agent-core`.
2. Implement `AgentExecutor` for the executor and `SessionImpl` for the
   session.
3. Add it to `members` in the root `Cargo.toml`.
4. Re-export the executor from `crates/unified/src/lib.rs`, add a
   `build_executor()` arm and an `available_providers()` entry. Gate it behind
   a cargo feature if it drags in a heavy dependency tree — that is what
   `bedrock` and `vertex` do.
5. Tests: unit tests for the constructor, capability bitmap and availability
   probe; an integration suite against a mock backend; a round-trip through
   `build_executor()` in `crates/unified/tests/`.

One rule matters more than the rest: **be honest in `capabilities()`.** Set a
flag to `true` only if the capability really works. Callers branch on those
flags to decide what to expose to their users, so a wrong flag breaks them
silently rather than loudly.

Do not add dependencies to `nucel-agent-core`; it stays small on purpose. Do
not bump versions in a PR — releases are done separately.

---

## Where this sits in Nucel

Two Nucel components consume these crates today, and they consume them
differently:

- **[`agent-operator`](https://github.com/nucel-dev/agent-operator)** — the
  Kubernetes operator that runs `AgentTask` / `PrReviewTask`. Depends on the
  umbrella (`nucel-agent-sdk = "0.1"`, locked at `0.1.4`, so one release line
  behind this repo) behind its own `claude-code` / `codex` / `opencode` cargo
  features, and wraps each executor in its own `CodingAgent` port.
- **`nucel-server`** in the [`nucel`](https://github.com/nucel-dev/nucel)
  repo — depends on the individual crates (`nucel-agent-core`,
  `-claude-code`, `-codex`, `-opencode`, all at `0.2.0`) rather than the
  umbrella. Its `adapters/local.rs` constructs the concrete executors and
  keeps everything around them on `nucel_agent_core::AgentExecutor`, and it
  surfaces `availability().reason` verbatim when a `local` agent session fails
  because the server image ships no `claude` CLI.

Worth noting for anyone maintaining the factory: neither consumer actually
calls `build_executor()`. Both import the concrete executor types and select
between them with their own cargo features. The factory is still the right
entry point for a caller whose provider name arrives at runtime, but it is not
currently load-bearing inside Nucel.

Nothing in this workspace depends on Nucel, so it is usable on its own.

---

## Docs

- [`docs/usage.md`](docs/usage.md) — usage patterns, error handling, budget control
- [`docs/architecture.md`](docs/architecture.md) — internals, per-provider transport details, adding a provider
- [`docs/tutorials/`](docs/tutorials/) — getting started, multi-turn, streaming, retries, hooks, cost and tokens, budget control, provider comparison, and Bedrock/Vertex
- [`CHANGELOG.md`](CHANGELOG.md) — release notes
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — adding a provider

Known-stale, not yet corrected — treat this README and the source as
authoritative until they catch up:

- `docs/architecture.md` and `docs/tutorials/provider-comparison.md` predate
  the Bedrock and Vertex crates and still describe a three-provider workspace.
- `crates/codex/README.md` and `crates/unified/README.md` still report
  `session_resume: false` and `structured_output: true` for Codex. Both are
  the wrong way round against `crates/codex/src/lib.rs`.
- Several crate READMEs still show `nucel-agent-sdk = "0.1"` in their install
  snippet.

## License

[Apache-2.0](LICENSE)
