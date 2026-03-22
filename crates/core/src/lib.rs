//! Nucel Agent SDK — Core
//!
//! Provider-agnostic traits and types for AI coding agents.
//! Implement this trait to add support for any coding agent backend.

pub mod error;
pub mod executor;
pub mod session;
pub mod types;

pub use error::{AgentError, Result};
pub use executor::{
    AgentCapabilities, AgentExecutor, AvailabilityStatus, ExecutorConfig, SpawnConfig,
};
pub use session::{AgentSession, SessionImpl, SessionMetadata};
pub use types::{
    AgentCost, AgentResponse, ExecutorType, PermissionMode, ToolCall, ToolResult,
};
