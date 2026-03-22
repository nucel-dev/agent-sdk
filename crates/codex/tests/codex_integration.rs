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
    assert!(caps.structured_output, "Codex supports JSON schema output");
    assert!(!caps.session_resume, "CLI resume not yet implemented");
    assert!(!caps.mcp_support, "Codex does not support MCP");
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
