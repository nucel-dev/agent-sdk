//! Nucel Agent SDK — Core
//!
//! Provider-agnostic traits and types for AI coding agents. Implement
//! [`AgentExecutor`] and [`SessionImpl`] to add support for any coding-agent
//! backend (CLI subprocess, HTTP server, in-process library, ...).
//!
//! Most application code should depend on the umbrella crate
//! [`nucel-agent-sdk`](https://docs.rs/nucel-agent-sdk) rather than this
//! crate directly. Depend on `nucel-agent-core` only if you are:
//!
//! - **Writing a new provider** and want the bare trait surface, or
//! - Embedding the abstraction into something that should stay agnostic
//!   to which providers are compiled in.
//!
//! # Minimal example
//!
//! With a real provider plugged in (see
//! `crates/unified/examples/claude_basic.rs` in the workspace),
//! the shape of a session is:
//!
//! ```rust,no_run
//! use nucel_agent_core::{AgentExecutor, SpawnConfig};
//! use std::path::Path;
//! # async fn run(executor: impl AgentExecutor) -> nucel_agent_core::Result<()> {
//! let session = executor.spawn(
//!     Path::new("/my/repo"),
//!     "What does this codebase do?",
//!     &SpawnConfig { budget_usd: Some(1.0), ..Default::default() },
//! ).await?;
//!
//! let resp = session.query("Now write me a one-line summary.").await?;
//! println!("{}", resp.content);
//! session.close().await?;
//! # Ok(()) }
//! ```
//!
//! # See also
//!
//! - [Workspace README](https://github.com/nucel-dev/agent-sdk#readme)
//! - [`docs/architecture.md`](https://github.com/nucel-dev/agent-sdk/blob/main/docs/architecture.md)
//! - [`CONTRIBUTING.md`](https://github.com/nucel-dev/agent-sdk/blob/main/CONTRIBUTING.md) — how to add a new provider.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod error;
pub mod executor;
pub mod retry;
pub mod session;
pub mod types;

pub use error::{AgentError, Result};
pub use executor::{
    AgentCapabilities, AgentExecutor, AvailabilityStatus, ExecutorConfig, SpawnConfig,
};
pub use retry::{is_transient, RetryPolicy};
pub use session::{AgentSession, EventStream, SessionImpl, SessionMetadata};
pub use types::{
    AgentCost, AgentResponse, CachePoint, ExecutorType, HookConfig, HookHandler, MessageEvent,
    PermissionMode, ToolCall, ToolResult,
};
