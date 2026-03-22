//! Extended protocol tests for Claude Code.

use nucel_agent_claude_code::ClaudeCodeExecutor;
use nucel_agent_core::*;

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
    assert!(!caps.session_resume, "CLI resume not yet supported");
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
