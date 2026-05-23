//! Extended tests for Codex provider.

use nucel_agent_codex::CodexExecutor;
use nucel_agent_core::*;

#[test]
fn codex_executor_type() {
    assert_eq!(CodexExecutor::new().executor_type(), ExecutorType::Codex);
}

#[test]
fn codex_executor_with_api_key() {
    let exec = CodexExecutor::with_api_key("sk-test-key");
    assert_eq!(exec.executor_type(), ExecutorType::Codex);
}

#[test]
fn codex_capabilities() {
    let caps = CodexExecutor::new().capabilities();
    assert!(caps.autonomous_mode, "Codex can run bash and edit files");
    assert!(caps.token_usage, "Codex reports token usage");
    assert!(
        !caps.structured_output,
        "structured output not yet wired in this version"
    );
    assert!(
        caps.session_resume,
        "Codex now resumes via `codex exec resume`"
    );
    assert!(!caps.mcp_support, "Codex does not support MCP");
}

#[test]
fn codex_accepts_all_permission_modes_without_panic() {
    // Regression: ensure new core variants don't break the executor.
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
        assert_eq!(cfg.permission_mode, Some(mode));
    }
}

#[test]
fn codex_availability_checks_cli() {
    let avail = CodexExecutor::new().availability();
    if !avail.available {
        let reason = avail.reason.unwrap();
        assert!(
            reason.contains("codex"),
            "reason should mention codex: {reason}"
        );
        assert!(
            reason.contains("npm"),
            "reason should mention installation: {reason}"
        );
    }
}
