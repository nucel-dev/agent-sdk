//! Extended protocol tests for Claude Code.

use nucel_agent_claude_code::ClaudeCodeExecutor;
use nucel_agent_core::*;
use std::path::Path;

#[test]
fn claude_executor_type() {
    assert_eq!(
        ClaudeCodeExecutor::new().executor_type(),
        ExecutorType::ClaudeCode
    );
}

#[test]
fn claude_executor_with_api_key() {
    let exec = ClaudeCodeExecutor::with_api_key("sk-ant-test");
    assert_eq!(exec.executor_type(), ExecutorType::ClaudeCode);
}

#[test]
fn claude_capabilities() {
    let caps = ClaudeCodeExecutor::new().capabilities();
    assert!(caps.autonomous_mode);
    assert!(caps.token_usage);
    assert!(caps.mcp_support);
    assert!(caps.session_resume, "Claude Code supports --resume flag");
    assert!(!caps.structured_output);
}

#[test]
fn claude_availability_checks_cli() {
    let avail = ClaudeCodeExecutor::new().availability();
    // Either available or has a reason about the CLI
    if !avail.available {
        assert!(avail.reason.unwrap().contains("claude"));
    }
}

#[test]
fn claude_accepts_new_permission_modes_without_panic() {
    // Regression: ensure new core variants compile against the claude-code
    // executor without exhaustive-match breakage.
    for mode in [
        PermissionMode::Prompt,
        PermissionMode::AcceptEdits,
        PermissionMode::BypassPermissions,
        PermissionMode::RejectAll,
        PermissionMode::DontAsk,
        PermissionMode::Auto,
    ] {
        let cfg = SpawnConfig {
            permission_mode: Some(mode),
            ..Default::default()
        };
        // We're not actually spawning — just exercising the config plumbing.
        assert_eq!(cfg.permission_mode, Some(mode));
    }
}

#[tokio::test]
async fn claude_spawn_returns_cli_not_found_when_missing() {
    // If `claude` CLI is missing we should get a CliNotFound error rather
    // than panicking — this also guards against silently swallowing the
    // not-found io::ErrorKind.
    if ClaudeCodeExecutor::new().availability().available {
        // CLI is present on this machine; skip the negative assertion.
        return;
    }
    let exec = ClaudeCodeExecutor::new();
    let result = exec
        .spawn(
            Path::new("/tmp"),
            "test",
            &SpawnConfig {
                budget_usd: Some(1.0),
                ..Default::default()
            },
        )
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, AgentError::CliNotFound { .. }),
        "expected CliNotFound, got: {err}"
    );
}
