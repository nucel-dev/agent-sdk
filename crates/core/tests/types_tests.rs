//! Integration tests for nucel-agent-core types.

use nucel_agent_core::*;

// ── AgentCost ──────────────────────────────────────────────────────────────

#[test]
fn cost_default_is_zero() {
    let cost = AgentCost::default();
    assert_eq!(cost.input_tokens, 0);
    assert_eq!(cost.output_tokens, 0);
    assert_eq!(cost.total_usd, 0.0);
}

#[test]
fn cost_add_accumulates() {
    let a = AgentCost {
        input_tokens: 100,
        output_tokens: 50,
        total_usd: 0.05,
    };
    let b = AgentCost {
        input_tokens: 200,
        output_tokens: 75,
        total_usd: 0.10,
    };
    let sum = a + b;
    assert_eq!(sum.input_tokens, 300);
    assert_eq!(sum.output_tokens, 125);
    assert!((sum.total_usd - 0.15).abs() < f64::EPSILON);
}

#[test]
fn cost_serialization_roundtrip() {
    let cost = AgentCost {
        input_tokens: 42,
        output_tokens: 17,
        total_usd: 1.23,
    };
    let json = serde_json::to_string(&cost).unwrap();
    let back: AgentCost = serde_json::from_str(&json).unwrap();
    assert_eq!(back.input_tokens, 42);
    assert_eq!(back.output_tokens, 17);
    assert!((back.total_usd - 1.23).abs() < f64::EPSILON);
}

// ── AgentResponse ──────────────────────────────────────────────────────────

#[test]
fn response_default_is_empty() {
    let resp = AgentResponse::default();
    assert!(resp.content.is_empty());
    assert!(!resp.requests_escalation);
    assert!(resp.tool_calls.is_empty());
    assert_eq!(resp.confidence, None);
}

#[test]
fn response_serialization_roundtrip() {
    let resp = AgentResponse {
        content: "Fixed the bug".into(),
        cost: AgentCost {
            input_tokens: 100,
            output_tokens: 50,
            total_usd: 0.01,
        },
        confidence: Some(0.85),
        requests_escalation: false,
        tool_calls: vec![ToolCall {
            name: "bash".into(),
            args: serde_json::json!({"cmd": "ls"}),
            result: Some(ToolResult {
                success: true,
                output: "file.rs\n".into(),
            }),
        }],
    };
    let json = serde_json::to_string(&resp).unwrap();
    let back: AgentResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(back.content, "Fixed the bug");
    assert_eq!(back.confidence, Some(0.85));
    assert_eq!(back.tool_calls.len(), 1);
    assert_eq!(back.tool_calls[0].name, "bash");
}

// ── ExecutorType ───────────────────────────────────────────────────────────

#[test]
fn executor_type_display() {
    assert_eq!(ExecutorType::ClaudeCode.to_string(), "claude-code");
    assert_eq!(ExecutorType::Codex.to_string(), "codex");
    assert_eq!(ExecutorType::OpenCode.to_string(), "opencode");
}

#[test]
fn executor_type_equality() {
    assert_eq!(ExecutorType::ClaudeCode, ExecutorType::ClaudeCode);
    assert_ne!(ExecutorType::ClaudeCode, ExecutorType::Codex);
}

#[test]
fn executor_type_serialization_roundtrip() {
    let t = ExecutorType::ClaudeCode;
    let json = serde_json::to_string(&t).unwrap();
    assert_eq!(json, "\"claude-code\"");
    let back: ExecutorType = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ExecutorType::ClaudeCode);
}

// ── PermissionMode ─────────────────────────────────────────────────────────

#[test]
fn permission_mode_default_is_prompt() {
    assert_eq!(PermissionMode::default(), PermissionMode::Prompt);
}

#[test]
fn permission_mode_serialization() {
    let modes = vec![
        PermissionMode::Prompt,
        PermissionMode::AcceptEdits,
        PermissionMode::BypassPermissions,
        PermissionMode::RejectAll,
    ];
    for mode in modes {
        let json = serde_json::to_string(&mode).unwrap();
        let back: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, back);
    }
}

// ── SpawnConfig ────────────────────────────────────────────────────────────

#[test]
fn spawn_config_default() {
    let config = SpawnConfig::default();
    assert!(config.model.is_none());
    assert!(config.max_tokens.is_none());
    assert!(config.budget_usd.is_none());
    assert!(config.permission_mode.is_none());
    assert!(config.env.is_empty());
    assert!(config.system_prompt.is_none());
}

#[test]
fn spawn_config_with_values() {
    let config = SpawnConfig {
        model: Some("claude-opus-4-6".into()),
        max_tokens: Some(8192),
        budget_usd: Some(5.0),
        permission_mode: Some(PermissionMode::AcceptEdits),
        env: vec![("KEY".into(), "value".into())],
        system_prompt: Some("You are a coding assistant.".into()),
        reasoning: Some("high".into()),
    };
    assert_eq!(config.model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(config.max_tokens, Some(8192));
    assert_eq!(config.budget_usd, Some(5.0));
}

// ── AgentError ─────────────────────────────────────────────────────────────

#[test]
fn error_budget_exceeded_display() {
    let err = AgentError::BudgetExceeded {
        limit: 5.0,
        spent: 5.01,
    };
    let msg = err.to_string();
    assert!(msg.contains("5.00"));
    assert!(msg.contains("5.01"));
}

#[test]
fn error_provider_display() {
    let err = AgentError::Provider {
        provider: "claude-code".into(),
        message: "CLI not found".into(),
    };
    let msg = err.to_string();
    assert!(msg.contains("claude-code"));
    assert!(msg.contains("CLI not found"));
}

#[test]
fn error_cli_not_found_display() {
    let err = AgentError::CliNotFound {
        cli_name: "codex".into(),
    };
    assert!(err.to_string().contains("codex"));
}

#[test]
fn error_io_conversion() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let agent_err: AgentError = io_err.into();
    assert!(matches!(agent_err, AgentError::Io(_)));
}

// ── ToolCall / ToolResult ──────────────────────────────────────────────────

#[test]
fn tool_call_with_result() {
    let tc = ToolCall {
        name: "read_file".into(),
        args: serde_json::json!({"path": "src/main.rs"}),
        result: Some(ToolResult {
            success: true,
            output: "fn main() {}".into(),
        }),
    };
    assert_eq!(tc.name, "read_file");
    assert!(tc.result.as_ref().unwrap().success);
}

#[test]
fn tool_call_without_result() {
    let tc = ToolCall {
        name: "bash".into(),
        args: serde_json::json!({"cmd": "ls"}),
        result: None,
    };
    assert!(tc.result.is_none());
}
