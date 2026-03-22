//! OpenCode SSE protocol types.
//!
//! These types are defined for future SSE streaming support.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// SSE event from OpenCode server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseEvent {
    pub event: String,
    pub data: serde_json::Value,
}

/// Session status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Idle,
    Completed,
    Failed,
}

/// Message from the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub id: String,
    pub role: String,
    pub parts: Vec<MessagePart>,
}

/// Part of a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MessagePart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_call")]
    ToolCall {
        name: String,
        args: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult { output: String, success: bool },
}
