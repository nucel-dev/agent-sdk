//! Wiremock integration tests for `nucel-agent-vertex`.
//!
//! Each test boots a local `MockServer`, points the executor at it via
//! `with_api_root`, and asserts request/response handling end-to-end —
//! without touching real GCP.

use std::path::Path;

use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use nucel_agent_core::{AgentError, AgentExecutor, ExecutorType, SpawnConfig};
use nucel_agent_vertex::VertexExecutor;

fn ok_body(text: &str, input: u64, output: u64) -> serde_json::Value {
    json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-4-7@20251024",
        "content": [
            { "type": "text", "text": text }
        ],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": input,
            "output_tokens": output
        }
    })
}

#[tokio::test]
async fn spawn_first_turn_hits_endpoint_and_records_cost() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path(
            "/v1/projects/my-proj/locations/us-east5/publishers/anthropic/models/claude-opus-4-7@20251024:rawPredict",
        ))
        .and(header("authorization", "Bearer fake-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("hi there", 42, 17)))
        .expect(1)
        .mount(&server)
        .await;

    let executor = VertexExecutor::with_static_token("my-proj", "us-east5", "fake-token")
        .with_api_root(server.uri());

    let cfg = SpawnConfig {
        model: Some("claude-opus-4-7@20251024".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };
    let session = executor
        .spawn(Path::new("/tmp"), "say hi", &cfg)
        .await
        .expect("spawn ok");

    assert_eq!(session.executor_type, ExecutorType::ClaudeCode);
    let cost = session.total_cost().await.unwrap();
    assert_eq!(cost.input_tokens, 42);
    assert_eq!(cost.output_tokens, 17);
    assert!(cost.total_usd > 0.0);
}

#[tokio::test]
async fn multi_turn_accumulates() {
    let server = MockServer::start().await;
    let url_path =
        "/v1/projects/p/locations/us-east5/publishers/anthropic/models/claude-sonnet-4@20251015:rawPredict";

    Mock::given(method("POST"))
        .and(path(url_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("first", 100, 50)))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path(url_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("second", 200, 100)))
        .mount(&server)
        .await;

    let executor =
        VertexExecutor::with_static_token("p", "us-east5", "tok").with_api_root(server.uri());
    let cfg = SpawnConfig {
        model: Some("claude-sonnet-4@20251015".into()),
        budget_usd: Some(50.0),
        ..Default::default()
    };
    let session = executor
        .spawn(Path::new("/tmp"), "go", &cfg)
        .await
        .expect("spawn");

    let resp = session.query("again").await.expect("turn 2");
    assert_eq!(resp.content, "second");

    let cost = session.total_cost().await.unwrap();
    assert_eq!(cost.input_tokens, 300);
    assert_eq!(cost.output_tokens, 150);
}

#[tokio::test]
async fn rate_limit_maps_to_rate_limited_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_string("Quota exceeded"))
        .mount(&server)
        .await;

    let executor =
        VertexExecutor::with_static_token("p", "us-east5", "tok").with_api_root(server.uri());
    let cfg = SpawnConfig {
        model: Some("claude-sonnet-4@20251015".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };

    let err = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .unwrap_err();
    assert!(matches!(err, AgentError::RateLimited { .. }));
}

#[tokio::test]
async fn http_error_maps_to_provider_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let executor =
        VertexExecutor::with_static_token("p", "us-east5", "tok").with_api_root(server.uri());
    let cfg = SpawnConfig {
        model: Some("claude-sonnet-4@20251015".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };

    let err = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .unwrap_err();
    match err {
        AgentError::Provider { provider, message } => {
            assert_eq!(provider, "vertex");
            assert!(message.contains("500"));
        }
        other => panic!("expected Provider error, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_json_maps_to_provider_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("not-json", "application/json"),
        )
        .mount(&server)
        .await;

    let executor =
        VertexExecutor::with_static_token("p", "us-east5", "tok").with_api_root(server.uri());
    let cfg = SpawnConfig {
        model: Some("claude-sonnet-4@20251015".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };
    let err = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .unwrap_err();
    assert!(matches!(err, AgentError::Provider { .. }));
}

#[tokio::test]
async fn budget_short_circuits_next_turn() {
    let server = MockServer::start().await;
    let url_path =
        "/v1/projects/p/locations/us-east5/publishers/anthropic/models/claude-sonnet-4@20251015:rawPredict";

    // First turn burns the budget.
    Mock::given(method("POST"))
        .and(path(url_path))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body("ok", 10_000_000, 10_000_000)),
        )
        .expect(1)
        .mount(&server)
        .await;

    let executor =
        VertexExecutor::with_static_token("p", "us-east5", "tok").with_api_root(server.uri());
    let cfg = SpawnConfig {
        model: Some("claude-sonnet-4@20251015".into()),
        budget_usd: Some(0.01),
        ..Default::default()
    };
    let session = executor
        .spawn(Path::new("/tmp"), "burn", &cfg)
        .await
        .expect("first turn ok");

    let err = session.query("again").await.unwrap_err();
    assert!(matches!(err, AgentError::BudgetExceeded { .. }));
}
