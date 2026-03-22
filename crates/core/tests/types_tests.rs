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

// ── AgentCost edge cases ──────────────────────────────────────────────────

#[test]
fn cost_add_identity() {
    let a = AgentCost {
        input_tokens: 50,
        output_tokens: 25,
        total_usd: 0.03,
    };
    let zero = AgentCost::default();
    let sum = a.clone() + zero;
    assert_eq!(sum.input_tokens, 50);
    assert_eq!(sum.output_tokens, 25);
    assert!((sum.total_usd - 0.03).abs() < f64::EPSILON);
}

#[test]
fn cost_large_token_counts() {
    let a = AgentCost {
        input_tokens: u64::MAX / 2,
        output_tokens: u64::MAX / 2,
        total_usd: 999_999.99,
    };
    let b = AgentCost {
        input_tokens: 1,
        output_tokens: 1,
        total_usd: 0.01,
    };
    let sum = a + b;
    assert_eq!(sum.input_tokens, u64::MAX / 2 + 1);
    assert_eq!(sum.output_tokens, u64::MAX / 2 + 1);
}

// ── ExecutorType hash / collections ───────────────────────────────────────

#[test]
fn executor_type_hashable() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(ExecutorType::ClaudeCode);
    set.insert(ExecutorType::Codex);
    set.insert(ExecutorType::OpenCode);
    set.insert(ExecutorType::ClaudeCode); // duplicate
    assert_eq!(set.len(), 3);
}

#[test]
fn executor_type_deserialize_all_variants() {
    let variants = ["\"claude-code\"", "\"codex\"", "\"open-code\""];
    let expected = [ExecutorType::ClaudeCode, ExecutorType::Codex, ExecutorType::OpenCode];
    for (json, exp) in variants.iter().zip(expected.iter()) {
        let parsed: ExecutorType = serde_json::from_str(json).unwrap();
        assert_eq!(&parsed, exp);
    }
}

#[test]
fn executor_type_invalid_deserialize() {
    let result = serde_json::from_str::<ExecutorType>("\"unknown-agent\"");
    assert!(result.is_err());
}

// ── PermissionMode edge cases ─────────────────────────────────────────────

#[test]
fn permission_mode_snake_case_serialization() {
    let json = serde_json::to_string(&PermissionMode::BypassPermissions).unwrap();
    assert_eq!(json, "\"bypass_permissions\"");
    let json = serde_json::to_string(&PermissionMode::AcceptEdits).unwrap();
    assert_eq!(json, "\"accept_edits\"");
    let json = serde_json::to_string(&PermissionMode::RejectAll).unwrap();
    assert_eq!(json, "\"reject_all\"");
}

// ── AgentError variants ──────────────────────────────────────────────────

#[test]
fn error_session_not_found_display() {
    let err = AgentError::SessionNotFound {
        session_id: "sess-xyz".into(),
    };
    let msg = err.to_string();
    assert!(msg.contains("sess-xyz"));
    assert!(msg.contains("session not found"));
}

#[test]
fn error_timeout_display() {
    let err = AgentError::Timeout { seconds: 300 };
    let msg = err.to_string();
    assert!(msg.contains("300"));
    assert!(msg.contains("timeout"));
}

#[test]
fn error_config_display() {
    let err = AgentError::Config("missing API key".into());
    assert!(err.to_string().contains("missing API key"));
}

#[test]
fn error_escalation_display() {
    let err = AgentError::EscalationRequested;
    assert!(err.to_string().contains("escalation"));
}

#[test]
fn error_json_conversion() {
    let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
    let agent_err: AgentError = json_err.into();
    assert!(matches!(agent_err, AgentError::Json(_)));
}

// ── AgentResponse with escalation ─────────────────────────────────────────

#[test]
fn response_with_escalation() {
    let resp = AgentResponse {
        content: "I need help".into(),
        requests_escalation: true,
        ..Default::default()
    };
    assert!(resp.requests_escalation);
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("true"));
    let back: AgentResponse = serde_json::from_str(&json).unwrap();
    assert!(back.requests_escalation);
}

#[test]
fn response_multiple_tool_calls() {
    let resp = AgentResponse {
        content: "Done".into(),
        tool_calls: vec![
            ToolCall {
                name: "read".into(),
                args: serde_json::json!({"path": "a.rs"}),
                result: Some(ToolResult { success: true, output: "code".into() }),
            },
            ToolCall {
                name: "write".into(),
                args: serde_json::json!({"path": "b.rs", "content": "new"}),
                result: Some(ToolResult { success: true, output: "ok".into() }),
            },
            ToolCall {
                name: "bash".into(),
                args: serde_json::json!({"cmd": "cargo test"}),
                result: Some(ToolResult { success: false, output: "FAILED".into() }),
            },
        ],
        ..Default::default()
    };
    assert_eq!(resp.tool_calls.len(), 3);
    assert!(!resp.tool_calls[2].result.as_ref().unwrap().success);

    let json = serde_json::to_string(&resp).unwrap();
    let back: AgentResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tool_calls.len(), 3);
    assert_eq!(back.tool_calls[0].name, "read");
    assert_eq!(back.tool_calls[2].result.as_ref().unwrap().output, "FAILED");
}

// ── SpawnConfig clone ─────────────────────────────────────────────────────

#[test]
fn spawn_config_clone() {
    let config = SpawnConfig {
        model: Some("claude-opus-4-6".into()),
        budget_usd: Some(10.0),
        env: vec![("FOO".into(), "bar".into())],
        ..Default::default()
    };
    let cloned = config.clone();
    assert_eq!(cloned.model, config.model);
    assert_eq!(cloned.budget_usd, config.budget_usd);
    assert_eq!(cloned.env.len(), 1);
}

// ── AgentSession + SessionImpl mock ───────────────────────────────────────

#[tokio::test]
async fn session_query_and_cost_via_mock() {
    use std::sync::Arc;
    use async_trait::async_trait;

    struct MockSession;

    #[async_trait]
    impl nucel_agent_core::SessionImpl for MockSession {
        async fn query(&self, prompt: &str) -> nucel_agent_core::Result<AgentResponse> {
            Ok(AgentResponse {
                content: format!("echo: {prompt}"),
                cost: AgentCost {
                    input_tokens: 10,
                    output_tokens: 5,
                    total_usd: 0.001,
                },
                ..Default::default()
            })
        }
        async fn total_cost(&self) -> nucel_agent_core::Result<AgentCost> {
            Ok(AgentCost {
                input_tokens: 10,
                output_tokens: 5,
                total_usd: 0.001,
            })
        }
        async fn close(&self) -> nucel_agent_core::Result<()> {
            Ok(())
        }
    }

    let session = AgentSession::new(
        "test-session-1",
        ExecutorType::ClaudeCode,
        "/tmp/test",
        Some("test-model".into()),
        Arc::new(MockSession),
    );

    assert_eq!(session.session_id, "test-session-1");
    assert_eq!(session.executor_type, ExecutorType::ClaudeCode);
    assert_eq!(session.model.as_deref(), Some("test-model"));

    let resp = session.query("hello").await.unwrap();
    assert_eq!(resp.content, "echo: hello");
    assert_eq!(resp.cost.input_tokens, 10);

    let cost = session.total_cost().await.unwrap();
    assert!((cost.total_usd - 0.001).abs() < f64::EPSILON);

    let meta = session.metadata();
    assert_eq!(meta.session_id, "test-session-1");
    assert_eq!(meta.executor_type, ExecutorType::ClaudeCode);
    assert_eq!(meta.model.as_deref(), Some("test-model"));

    session.close().await.unwrap();
}

#[tokio::test]
async fn session_metadata_preserves_working_dir() {
    use std::sync::Arc;
    use async_trait::async_trait;

    struct NoopSession;
    #[async_trait]
    impl nucel_agent_core::SessionImpl for NoopSession {
        async fn query(&self, _: &str) -> nucel_agent_core::Result<AgentResponse> {
            Ok(AgentResponse::default())
        }
        async fn total_cost(&self) -> nucel_agent_core::Result<AgentCost> {
            Ok(AgentCost::default())
        }
        async fn close(&self) -> nucel_agent_core::Result<()> {
            Ok(())
        }
    }

    let session = AgentSession::new(
        "s1",
        ExecutorType::OpenCode,
        "/home/user/project",
        None,
        Arc::new(NoopSession),
    );

    let meta = session.metadata();
    assert_eq!(meta.working_dir.to_str().unwrap(), "/home/user/project");
    assert!(meta.model.is_none());
    assert_eq!(meta.executor_type, ExecutorType::OpenCode);
}

#[test]
fn session_debug_format() {
    use std::sync::Arc;
    use async_trait::async_trait;

    struct NoopSession;
    #[async_trait]
    impl nucel_agent_core::SessionImpl for NoopSession {
        async fn query(&self, _: &str) -> nucel_agent_core::Result<AgentResponse> {
            Ok(AgentResponse::default())
        }
        async fn total_cost(&self) -> nucel_agent_core::Result<AgentCost> {
            Ok(AgentCost::default())
        }
        async fn close(&self) -> nucel_agent_core::Result<()> {
            Ok(())
        }
    }

    let session = AgentSession::new(
        "debug-test",
        ExecutorType::Codex,
        "/tmp",
        Some("gpt-5".into()),
        Arc::new(NoopSession),
    );

    let debug = format!("{:?}", session);
    assert!(debug.contains("debug-test"));
    assert!(debug.contains("Codex"));
    assert!(debug.contains("gpt-5"));
}
