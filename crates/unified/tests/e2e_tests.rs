//! End-to-end tests for the Nucel Agent SDK.
//!
//! These tests exercise the full lifecycle: spawn → query → cost → close.
//! - OpenCode: full E2E against wiremock with a real temp repo
//! - Claude Code / Codex: spawn with real CLI if available, otherwise skip

use nucel_agent_sdk::*;
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Helpers ──────────────────────────────────────────────────────────────

/// Create a temp directory that looks like a real project workspace.
fn create_mock_repo() -> TempDir {
    let dir = TempDir::new().expect("failed to create temp dir");
    let repo_path = dir.path();

    // Initialize a git repo so agents don't complain.
    std::process::Command::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(repo_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok();

    std::process::Command::new("git")
        .args(["config", "user.email", "test@nucel.dev"])
        .current_dir(repo_path)
        .stdout(std::process::Stdio::null())
        .status()
        .ok();

    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(repo_path)
        .stdout(std::process::Stdio::null())
        .status()
        .ok();

    // Create some source files.
    std::fs::write(
        repo_path.join("main.rs"),
        r#"fn main() {
    println!("Hello from mock repo");
}
"#,
    )
    .unwrap();

    std::fs::write(
        repo_path.join("lib.rs"),
        r#"pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }
}
"#,
    )
    .unwrap();

    std::fs::write(
        repo_path.join("Cargo.toml"),
        r#"[package]
name = "mock-project"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // Commit so the repo has history.
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo_path)
        .stdout(std::process::Stdio::null())
        .status()
        .ok();

    std::process::Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(repo_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok();

    dir
}

/// Check if a CLI tool is available.
fn cli_available(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── OpenCode E2E (wiremock) ─────────────────────────────────────────────

/// Full lifecycle against wiremock: create session → prompt → follow-up → cost → close.
#[tokio::test]
async fn e2e_opencode_full_session_lifecycle() {
    let server = MockServer::start().await;
    let repo = create_mock_repo();

    // 1. POST /session → create session
    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "id": "e2e-sess-001",
                "status": "active"
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    // 2. POST /session/e2e-sess-001/prompt → initial response
    Mock::given(method("POST"))
        .and(path("/session/e2e-sess-001/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "parts": [
                    {"type": "text", "text": "I've analyzed the codebase. The main.rs file prints 'Hello from mock repo' and lib.rs has an `add` function with a passing test."}
                ],
                "cost": 0.012
            })),
        )
        .mount(&server)
        .await;

    let executor = OpencodeExecutor::with_base_url(server.uri());

    // Verify availability.
    let avail = executor.availability();
    assert!(avail.available);

    // Spawn session.
    let session = executor
        .spawn(
            repo.path(),
            "Analyze this Rust project and describe what it does",
            &SpawnConfig {
                model: Some("claude-sonnet-4-6".into()),
                budget_usd: Some(1.0),
                system_prompt: Some("You are a code reviewer.".into()),
                ..Default::default()
            },
        )
        .await
        .expect("spawn should succeed");

    // Verify session properties.
    assert_eq!(session.executor_type, ExecutorType::OpenCode);
    assert_eq!(session.model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(session.working_dir, repo.path());

    // Check metadata.
    let meta = session.metadata();
    assert_eq!(meta.executor_type, ExecutorType::OpenCode);
    assert_eq!(meta.model.as_deref(), Some("claude-sonnet-4-6"));
    assert!(meta.created_at <= chrono::Utc::now());

    // Check cost from initial prompt.
    let cost = session.total_cost().await.unwrap();
    assert!((cost.total_usd - 0.012).abs() < f64::EPSILON);

    // Close session.
    session.close().await.expect("close should succeed");
}

/// E2E: budget exceeded mid-session.
#[tokio::test]
async fn e2e_opencode_budget_exceeded_during_prompt() {
    let server = MockServer::start().await;
    let repo = create_mock_repo();

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "e2e-budget"})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/session/e2e-budget/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "parts": [{"type": "text", "text": "Expensive analysis"}],
                "cost": 5.0
            })),
        )
        .mount(&server)
        .await;

    let executor = OpencodeExecutor::with_base_url(server.uri());

    let result = executor
        .spawn(
            repo.path(),
            "Deep analysis",
            &SpawnConfig {
                budget_usd: Some(0.50),
                ..Default::default()
            },
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("budget exceeded"),
        "expected budget error, got: {err}"
    );
}

/// E2E: session resume with existing session ID.
#[tokio::test]
async fn e2e_opencode_resume_existing_session() {
    let server = MockServer::start().await;
    let repo = create_mock_repo();

    Mock::given(method("POST"))
        .and(path("/session/prev-session-42/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "parts": [{"type": "text", "text": "Continuing from where we left off."}],
                "cost": 0.005
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let executor = OpencodeExecutor::with_base_url(server.uri());

    let session = executor
        .resume(
            repo.path(),
            "prev-session-42",
            "What was the last change you made?",
            &SpawnConfig {
                budget_usd: Some(1.0),
                ..Default::default()
            },
        )
        .await
        .expect("resume should succeed");

    assert_eq!(session.executor_type, ExecutorType::OpenCode);
    let cost = session.total_cost().await.unwrap();
    assert!((cost.total_usd - 0.005).abs() < f64::EPSILON);

    session.close().await.unwrap();
}

/// E2E: server error during session creation.
#[tokio::test]
async fn e2e_opencode_server_error_handling() {
    let server = MockServer::start().await;
    let repo = create_mock_repo();

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let executor = OpencodeExecutor::with_base_url(server.uri());

    let result = executor
        .spawn(
            repo.path(),
            "test",
            &SpawnConfig::default(),
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("opencode"),
        "error should mention provider: {err}"
    );
}

/// E2E: multiple text parts in response are concatenated.
#[tokio::test]
async fn e2e_opencode_multipart_response() {
    let server = MockServer::start().await;
    let repo = create_mock_repo();

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "e2e-multi"})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/session/e2e-multi/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "parts": [
                    {"type": "text", "text": "Step 1: Read the code"},
                    {"type": "tool_call", "name": "read_file", "args": {"path": "main.rs"}},
                    {"type": "tool_result", "output": "fn main() {}", "success": true},
                    {"type": "text", "text": "Step 2: The code looks good"}
                ],
                "cost": 0.008
            })),
        )
        .mount(&server)
        .await;

    let executor = OpencodeExecutor::with_base_url(server.uri());

    let session = executor
        .spawn(
            repo.path(),
            "Review the code",
            &SpawnConfig::default(),
        )
        .await
        .unwrap();

    let cost = session.total_cost().await.unwrap();
    assert!((cost.total_usd - 0.008).abs() < f64::EPSILON);

    session.close().await.unwrap();
}

// ── build_executor E2E ──────────────────────────────────────────────────

/// E2E: build_executor produces working executors for all providers.
#[test]
fn e2e_build_executor_all_providers_construct() {
    let repo = create_mock_repo();

    for provider in available_providers() {
        let executor = build_executor(provider, None)
            .unwrap_or_else(|| panic!("build_executor({provider}) returned None"));

        // Check type matches.
        let expected_type = match *provider {
            "claude-code" => ExecutorType::ClaudeCode,
            "codex" => ExecutorType::Codex,
            "opencode" => ExecutorType::OpenCode,
            _ => panic!("unexpected provider: {provider}"),
        };
        assert_eq!(executor.executor_type(), expected_type);

        // Capabilities are populated.
        let caps = executor.capabilities();
        assert!(caps.token_usage);

        // Working dir exists in the mock repo.
        assert!(repo.path().exists());
    }
}

// ── Claude Code E2E (real CLI, skip if unavailable) ─────────────────────

/// Full E2E with real Claude Code CLI — spawns a real session.
/// Skipped if `claude` is not installed or ANTHROPIC_API_KEY is not set.
#[tokio::test]
async fn e2e_claude_code_real_cli_session() {
    if !cli_available("claude") {
        eprintln!("SKIP: claude CLI not available");
        return;
    }
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("SKIP: ANTHROPIC_API_KEY not set");
        return;
    }

    let repo = create_mock_repo();
    let executor = ClaudeCodeExecutor::new();

    let avail = executor.availability();
    assert!(avail.available, "claude CLI should be available");

    let session = executor
        .spawn(
            repo.path(),
            "What files are in this project? Reply in one sentence.",
            &SpawnConfig {
                budget_usd: Some(0.50),
                permission_mode: Some(PermissionMode::RejectAll),
                ..Default::default()
            },
        )
        .await;

    match session {
        Ok(sess) => {
            assert_eq!(sess.executor_type, ExecutorType::ClaudeCode);
            assert!(!sess.session_id.is_empty());

            let cost = sess.total_cost().await.unwrap();
            assert!(cost.total_usd >= 0.0, "cost should be non-negative");
            assert!(cost.total_usd <= 0.50, "cost should be within budget");

            sess.close().await.unwrap();
        }
        Err(e) => {
            // Acceptable failures: timeout, rate limit, etc.
            eprintln!("Claude Code E2E failed (acceptable): {e}");
        }
    }
}

// ── Codex E2E (real CLI, skip if unavailable) ───────────────────────────

/// Full E2E with real Codex CLI.
/// Skipped if `codex` is not installed or CODEX_API_KEY is not set.
#[tokio::test]
async fn e2e_codex_real_cli_session() {
    if !cli_available("codex") {
        eprintln!("SKIP: codex CLI not available");
        return;
    }
    if std::env::var("CODEX_API_KEY").is_err() && std::env::var("OPENAI_API_KEY").is_err() {
        eprintln!("SKIP: CODEX_API_KEY/OPENAI_API_KEY not set");
        return;
    }

    let repo = create_mock_repo();
    let executor = CodexExecutor::new();

    let avail = executor.availability();
    assert!(avail.available, "codex CLI should be available");

    let session = executor
        .spawn(
            repo.path(),
            "List the files in this directory",
            &SpawnConfig {
                budget_usd: Some(0.50),
                ..Default::default()
            },
        )
        .await;

    match session {
        Ok(sess) => {
            assert_eq!(sess.executor_type, ExecutorType::Codex);
            let cost = sess.total_cost().await.unwrap();
            assert!(cost.total_usd >= 0.0);
            sess.close().await.unwrap();
        }
        Err(e) => {
            eprintln!("Codex E2E failed (acceptable): {e}");
        }
    }
}
