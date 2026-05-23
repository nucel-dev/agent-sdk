//! Wiremock integration tests for OpenCode HTTP client.

use nucel_agent_opencode::OpencodeExecutor;
use nucel_agent_core::{AgentExecutor, ExecutorType, SpawnConfig};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use serde_json::json;

// ── Executor basics ────────────────────────────────────────────────────────

#[test]
fn opencode_executor_type() {
    let exec = OpencodeExecutor::new();
    assert_eq!(exec.executor_type(), ExecutorType::OpenCode);
}

#[test]
fn opencode_capabilities() {
    let caps = OpencodeExecutor::new().capabilities();
    assert!(caps.session_resume, "OpenCode supports session resume");
    assert!(caps.autonomous_mode, "OpenCode supports autonomous mode");
    assert!(caps.mcp_support, "OpenCode supports MCP");
    assert!(caps.token_usage, "OpenCode reports token usage");
    assert!(!caps.structured_output, "OpenCode does not support structured output yet");
}

#[test]
fn opencode_availability_mentions_server() {
    let avail = OpencodeExecutor::new().availability();
    let reason = avail.reason.unwrap();
    assert!(reason.contains("opencode"), "reason should mention opencode: {reason}");
    assert!(reason.contains("4096"), "reason should mention default port: {reason}");
}

#[test]
fn opencode_custom_url_does_not_panic() {
    let exec = OpencodeExecutor::with_base_url("http://myhost:9090/");
    assert_eq!(exec.executor_type(), ExecutorType::OpenCode);
}

#[test]
fn opencode_with_api_key_does_not_panic() {
    let exec = OpencodeExecutor::new().with_api_key("sk-test");
    assert_eq!(exec.executor_type(), ExecutorType::OpenCode);
}

// ── Wiremock HTTP tests ────────────────────────────────────────────────────

#[tokio::test]
async fn opencode_spawn_creates_session_and_prompts() {
    let server = MockServer::start().await;

    // Mock: POST /session → returns session with id
    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "sess-123", "status": "active"})),
        )
        .mount(&server)
        .await;

    // Mock: POST /session/sess-123/prompt → returns agent response
    Mock::given(method("POST"))
        .and(path("/session/sess-123/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "parts": [
                    {"type": "text", "text": "Fixed the failing test"}
                ],
                "cost": 0.003
            })),
        )
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());

    let session = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "Fix the failing test",
            &SpawnConfig {
                model: Some("claude-sonnet-4".into()),
                budget_usd: Some(5.0),
                ..Default::default()
            },
        )
        .await
        .expect("spawn should succeed");

    assert_eq!(session.executor_type, ExecutorType::OpenCode);
    assert_eq!(session.model.as_deref(), Some("claude-sonnet-4"));

    let cost = session.total_cost().await.expect("cost should be available");
    assert!((cost.total_usd - 0.003).abs() < f64::EPSILON);
}

#[tokio::test]
async fn opencode_spawn_budget_zero_rejected() {
    let server = MockServer::start().await;
    let exec = OpencodeExecutor::with_base_url(server.uri());

    let result = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "test",
            &SpawnConfig {
                budget_usd: Some(0.0),
                ..Default::default()
            },
        )
        .await;

    assert!(result.is_err(), "budget of 0 should be rejected");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("budget exceeded"),
        "error should mention budget: {err}"
    );
}

#[tokio::test]
async fn opencode_spawn_session_creation_fails() {
    let server = MockServer::start().await;

    // Mock: POST /session → 500 error
    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());

    let result = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "test",
            &SpawnConfig::default(),
        )
        .await;

    assert!(result.is_err(), "500 from session creation should fail");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("opencode"),
        "error should mention opencode provider: {err}"
    );
}

#[tokio::test]
async fn opencode_spawn_prompt_fails() {
    let server = MockServer::start().await;

    // Session creation succeeds
    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "sess-fail"})),
        )
        .mount(&server)
        .await;

    // Prompt fails
    Mock::given(method("POST"))
        .and(path("/session/sess-fail/prompt"))
        .respond_with(ResponseTemplate::new(422))
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());

    let result = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "test",
            &SpawnConfig::default(),
        )
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("422"));
}

#[tokio::test]
async fn opencode_spawn_extracts_multiple_text_parts() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "sess-multi"})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/session/sess-multi/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "parts": [
                    {"type": "text", "text": "First block"},
                    {"type": "tool_call", "name": "bash", "args": {}},
                    {"type": "text", "text": "Second block"}
                ],
                "cost": 0.001
            })),
        )
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let session = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "test",
            &SpawnConfig::default(),
        )
        .await
        .unwrap();

    let cost = session.total_cost().await.unwrap();
    assert!((cost.total_usd - 0.001).abs() < f64::EPSILON);
}

#[tokio::test]
async fn opencode_spawn_budget_exceeded_by_response() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "sess-expensive"})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/session/sess-expensive/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "parts": [{"type": "text", "text": "Expensive response"}],
                "cost": 10.0
            })),
        )
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());

    let result = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "test",
            &SpawnConfig {
                budget_usd: Some(0.01),
                ..Default::default()
            },
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("budget exceeded"), "error: {err}");
}

#[tokio::test]
async fn opencode_spawn_invalid_json_returns_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "sess-bad-json"})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/session/sess-bad-json/prompt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let result = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "test",
            &SpawnConfig::default(),
        )
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn opencode_resume_sends_prompt_to_existing_session() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session/existing-sess/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "parts": [{"type": "text", "text": "Resumed response"}],
                "cost": 0.002
            })),
        )
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let session = exec
        .resume(
            std::path::Path::new("/tmp"),
            "existing-sess",
            "Continue the work",
            &SpawnConfig::default(),
        )
        .await
        .expect("resume should succeed");

    assert_eq!(session.executor_type, ExecutorType::OpenCode);
    let cost = session.total_cost().await.unwrap();
    assert!((cost.total_usd - 0.002).abs() < f64::EPSILON);
}

#[tokio::test]
async fn opencode_spawn_with_model_and_system_prompt() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "sess-model"})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/session/sess-model/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "parts": [{"type": "text", "text": "Response with model"}],
                "cost": 0.005
            })),
        )
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let session = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "test with model",
            &SpawnConfig {
                model: Some("claude-opus-4-6".into()),
                system_prompt: Some("You are a test assistant".into()),
                budget_usd: Some(1.0),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(session.model.as_deref(), Some("claude-opus-4-6"));
}

#[tokio::test]
async fn opencode_spawn_negative_budget_rejected() {
    let exec = OpencodeExecutor::new();
    let result = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "test",
            &SpawnConfig {
                budget_usd: Some(-5.0),
                ..Default::default()
            },
        )
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("budget"));
}

#[tokio::test]
async fn opencode_spawn_fallback_to_text_field() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "sess-text"})),
        )
        .mount(&server)
        .await;

    // Response uses "text" field instead of "parts" array
    Mock::given(method("POST"))
        .and(path("/session/sess-text/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "text": "Fallback text response",
                "cost": 0.001
            })),
        )
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let session = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "test",
            &SpawnConfig::default(),
        )
        .await
        .unwrap();

    let cost = session.total_cost().await.unwrap();
    assert!((cost.total_usd - 0.001).abs() < f64::EPSILON);
}

#[tokio::test]
async fn opencode_spawn_parses_token_usage_v2_shape() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "sess-tok"})),
        )
        .mount(&server)
        .await;

    // v2 shape: info.tokens.{input,output}
    Mock::given(method("POST"))
        .and(path("/session/sess-tok/prompt"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "parts": [{"type": "text", "text": "Done"}],
            "cost": 0.0042,
            "info": {
                "tokens": { "input": 123, "output": 45 }
            }
        })))
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let session = exec
        .spawn(std::path::Path::new("/tmp"), "test", &SpawnConfig::default())
        .await
        .unwrap();

    let cost = session.total_cost().await.unwrap();
    assert_eq!(cost.input_tokens, 123);
    assert_eq!(cost.output_tokens, 45);
    assert!((cost.total_usd - 0.0042).abs() < 1e-9);
}

#[tokio::test]
async fn opencode_spawn_parses_token_usage_legacy_shape() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "sess-leg"})),
        )
        .mount(&server)
        .await;

    // Legacy: top-level tokens.{input,output}
    Mock::given(method("POST"))
        .and(path("/session/sess-leg/prompt"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "parts": [{"type": "text", "text": "Done"}],
            "cost": 0.001,
            "tokens": { "input": 9, "output": 11 }
        })))
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let session = exec
        .spawn(std::path::Path::new("/tmp"), "test", &SpawnConfig::default())
        .await
        .unwrap();

    let cost = session.total_cost().await.unwrap();
    assert_eq!(cost.input_tokens, 9);
    assert_eq!(cost.output_tokens, 11);
}

#[tokio::test]
async fn opencode_session_id_round_trips_from_server() {
    // The session.session_id should be the actual OpenCode server session id,
    // NOT a freshly-minted client-side UUID.
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "real-server-id"})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/session/real-server-id/prompt"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "parts": [{"type": "text", "text": "ok"}],
            "cost": 0.0
        })))
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let session = exec
        .spawn(std::path::Path::new("/tmp"), "test", &SpawnConfig::default())
        .await
        .unwrap();
    assert_eq!(session.session_id, "real-server-id");
}

#[tokio::test]
async fn opencode_resume_returns_original_server_session_id() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session/keep-this/prompt"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "parts": [{"type": "text", "text": "resumed"}],
            "cost": 0.0
        })))
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let session = exec
        .resume(
            std::path::Path::new("/tmp"),
            "keep-this",
            "go",
            &SpawnConfig::default(),
        )
        .await
        .unwrap();
    assert_eq!(session.session_id, "keep-this");
}

#[tokio::test]
async fn opencode_close_calls_abort_endpoint() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let abort_hit = std::sync::Arc::new(AtomicBool::new(false));

    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "sess-abort"})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/session/sess-abort/prompt"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "parts": [{"type": "text", "text": "go"}],
            "cost": 0.0
        })))
        .mount(&server)
        .await;

    let abort_hit_for_mock = abort_hit.clone();
    Mock::given(method("POST"))
        .and(path("/session/sess-abort/abort"))
        .respond_with(move |_req: &wiremock::Request| {
            abort_hit_for_mock.store(true, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_json(json!({"ok": true}))
        })
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let session = exec
        .spawn(std::path::Path::new("/tmp"), "test", &SpawnConfig::default())
        .await
        .unwrap();

    session.close().await.unwrap();
    assert!(
        abort_hit.load(Ordering::SeqCst),
        "close() must POST to /session/<id>/abort"
    );
}

#[tokio::test]
async fn opencode_basic_auth_password_is_sent_when_api_key_set() {
    use wiremock::matchers::header;
    let server = MockServer::start().await;

    // base64("opencode:my-secret") = "b3BlbmNvZGU6bXktc2VjcmV0"
    Mock::given(method("POST"))
        .and(path("/session"))
        .and(header("authorization", "Basic b3BlbmNvZGU6bXktc2VjcmV0"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "auth-sess"})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/session/auth-sess/prompt"))
        .and(header("authorization", "Basic b3BlbmNvZGU6bXktc2VjcmV0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "parts": [{"type": "text", "text": "ok"}],
            "cost": 0.0
        })))
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri()).with_api_key("my-secret");
    let session = exec
        .spawn(std::path::Path::new("/tmp"), "go", &SpawnConfig::default())
        .await
        .expect("spawn with api_key should send basic auth");
    assert_eq!(session.session_id, "auth-sess");
}

#[tokio::test]
async fn opencode_model_body_splits_on_slash_into_provider_and_model() {
    use wiremock::matchers::body_partial_json;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "model-sess"})),
        )
        .mount(&server)
        .await;

    // Body must contain providerID + modelID for "anthropic/claude-sonnet-4".
    Mock::given(method("POST"))
        .and(path("/session/model-sess/prompt"))
        .and(body_partial_json(json!({
            "model": { "providerID": "anthropic", "modelID": "claude-sonnet-4" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "parts": [{"type": "text", "text": "ok"}],
            "cost": 0.0
        })))
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let _session = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "go",
            &SpawnConfig {
                model: Some("anthropic/claude-sonnet-4".into()),
                ..Default::default()
            },
        )
        .await
        .expect("spawn with provider/model should split correctly");
}

#[tokio::test]
async fn opencode_directory_sent_as_query_string() {
    use wiremock::matchers::query_param;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .and(query_param("directory", "/work/dir"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "dir-sess"})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/session/dir-sess/prompt"))
        .and(query_param("directory", "/work/dir"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "parts": [{"type": "text", "text": "ok"}],
            "cost": 0.0
        })))
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let _session = exec
        .spawn(
            std::path::Path::new("/work/dir"),
            "go",
            &SpawnConfig::default(),
        )
        .await
        .expect("spawn should send directory as query string");
}

#[tokio::test]
async fn opencode_spawn_session_missing_id_field() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status": "active"})), // no "id" field
        )
        .mount(&server)
        .await;

    let exec = OpencodeExecutor::with_base_url(server.uri());
    let result = exec
        .spawn(
            std::path::Path::new("/tmp"),
            "test",
            &SpawnConfig::default(),
        )
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("missing id"));
}
