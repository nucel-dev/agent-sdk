# Contributing

Thanks for considering a contribution. This file focuses on the case that
needs the most guidance: **adding a new provider**.

For everything else (bug fixes, doc fixes, perf work, etc.), just open a PR
against `main`. CI runs `cargo test --workspace` and `cargo clippy --workspace`.

---

## Workspace layout

```text
agent-sdk/
├── crates/
│   ├── core/          # nucel-agent-core      — trait + types, zero provider deps
│   ├── claude-code/   # nucel-agent-claude-code
│   ├── codex/         # nucel-agent-codex
│   ├── opencode/      # nucel-agent-opencode
│   └── unified/       # nucel-agent-sdk       — umbrella crate + factory
├── docs/
└── crates/unified/examples/
```

See [`docs/architecture.md`](docs/architecture.md) for the dependency graph
and trait relationships.

---

## Adding a new provider

Suppose you want to add a `cool-agent` provider. Here's the checklist.

### 1. Create the crate

```bash
cd crates
cargo new --lib cool-agent
mv cool-agent ../crates/cool-agent  # if not already there
```

Add it to the workspace `Cargo.toml`:

```toml
[workspace]
members = [
    "crates/core",
    "crates/claude-code",
    "crates/codex",
    "crates/opencode",
    "crates/cool-agent",          # <-- here
    "crates/unified",
]
```

### 2. `Cargo.toml` metadata

Match the shape of the existing provider crates (see `crates/codex/Cargo.toml`
for the smallest reference). Required keys:

```toml
[package]
name = "nucel-agent-cool"
version = "0.1.0"
edition.workspace = true
license.workspace = true
description = "Cool-Agent provider for Nucel agent-sdk — <what it does>"
repository = "https://github.com/nucel-dev/agent-sdk"
homepage = "https://github.com/nucel-dev/agent-sdk"
documentation = "https://docs.rs/nucel-agent-cool"
readme = "README.md"
keywords = ["cool-agent", "coding", "ai", "agent"]    # max 5, kebab-case
categories = ["development-tools", "api-bindings"]

[dependencies]
nucel-agent-core = { path = "../core", version = "0.1" }
async-trait.workspace = true
# ...

[package.metadata.docs.rs]
all-features = true
rustdoc-args = ["--cfg", "docsrs"]
```

### 3. Implement `AgentExecutor`

```rust
use async_trait::async_trait;
use nucel_agent_core::{
    AgentCapabilities, AgentExecutor, AgentSession, AvailabilityStatus,
    ExecutorType, Result, SpawnConfig,
};

pub struct CoolAgentExecutor { /* fields */ }

#[async_trait]
impl AgentExecutor for CoolAgentExecutor {
    fn executor_type(&self) -> ExecutorType {
        // Add a variant to ExecutorType in core — see step 5.
        ExecutorType::CoolAgent
    }

    async fn spawn(
        &self,
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession> { /* ... */ }

    async fn resume(
        &self,
        working_dir: &Path,
        session_id: &str,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession> { /* ... */ }

    fn capabilities(&self) -> AgentCapabilities { /* ... */ }
    fn availability(&self) -> AvailabilityStatus { /* ... */ }
}
```

### 4. Implement `SessionImpl`

Provider-specific state goes here. At minimum:

```rust
#[async_trait]
impl SessionImpl for CoolAgentSession {
    async fn query(&self, prompt: &str) -> Result<AgentResponse> { /* ... */ }
    async fn total_cost(&self) -> Result<AgentCost> { /* ... */ }
    async fn close(&self) -> Result<()> { /* ... */ }
}
```

### 5. Add an `ExecutorType` variant

In `crates/core/src/types.rs`:

```rust
pub enum ExecutorType {
    ClaudeCode,
    Codex,
    OpenCode,
    CoolAgent,         // <-- here
}
```

Update the `Display` impl too. The enum is `#[serde(rename_all = "kebab-case")]`
so this serializes as `"cool-agent"`.

### 6. Wire it into the umbrella crate

In `crates/unified/Cargo.toml`:

```toml
nucel-agent-cool = { path = "../cool-agent", version = "0.1" }
```

In `crates/unified/src/lib.rs`:

```rust
pub use nucel_agent_cool::CoolAgentExecutor;

pub fn build_executor(
    provider: &str,
    api_key_or_url: Option<String>,
) -> Option<Box<dyn AgentExecutor>> {
    match provider {
        // existing arms ...
        "cool-agent" | "cool_agent" | "coolagent" => Some(Box::new(CoolAgentExecutor::new())),
        _ => None,
    }
}

pub fn available_providers() -> &'static [&'static str] {
    &["claude-code", "codex", "opencode", "cool-agent"]   // <-- here
}
```

### 7. Tests

At minimum:

- A `tests/` integration suite that exercises `spawn`, `query`, `resume`,
  and `close` against a mock backend (use `wiremock` for HTTP, a fake CLI
  for subprocesses — see `crates/claude-code/tests/` for the pattern).
- Unit tests in `lib.rs` for the constructor, the capability bitmap, and
  the availability probe.
- A round-trip test in `crates/unified/tests/` that goes through
  `build_executor("cool-agent", ...)`.

### 8. Docs

- Module-level `//!` doc in `lib.rs` with a minimal `spawn → query → close`
  example.
- A row in `docs/tutorials/provider-comparison.md`.
- `[package.metadata.docs.rs]` block in `Cargo.toml`.

### 9. Capability flags

Be honest. Set `AgentCapabilities` fields to `true` only if you actually
implement the capability:

```rust
AgentCapabilities {
    session_resume: true,       // resume() really works cross-process
    token_usage: true,          // we parse upstream usage
    mcp_support: false,         // be honest if you don't
    autonomous_mode: true,
    structured_output: false,
    streaming: false,           // 0.2.0 surface
    hooks: false,
    prompt_caching: false,
    extended_thinking: false,
}
```

Higher-level orchestrators query `capabilities()` to decide which features
to expose to the user. Lying here breaks them.

---

## Code style

- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` must
  pass.
- Public items get rustdoc. Module-level `//!` block at minimum.
- Errors flow through `nucel_agent_core::AgentError`. Don't invent your own
  error type for the provider crate — wrap into `AgentError::Provider`.
- Don't add dependencies to `nucel-agent-core`. It must stay tiny.

---

## Release

Releases are coordinated. Don't bump versions in PRs unless asked. A
maintainer will tag and publish on a cadence.
