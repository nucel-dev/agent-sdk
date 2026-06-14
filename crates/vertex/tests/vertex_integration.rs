//! Wiremock integration tests for `nucel-agent-vertex`.
//!
//! Each test boots a local `MockServer`, points the executor at it via
//! `with_api_root`, and asserts request/response handling end-to-end —
//! without touching real GCP.

use std::path::Path;

use futures::StreamExt;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use nucel_agent_core::{
    AgentError, AgentExecutor, ExecutorType, MessageEvent, RetryPolicy, SpawnConfig,
};
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
async fn cache_tokens_are_captured_from_usage() {
    // Vertex passes Anthropic's prompt-cache token counters straight through;
    // they must land in AgentCost so cost analytics see cache hits/writes.
    let server = MockServer::start().await;
    let body = json!({
        "id": "msg_cache",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-4-7@20251024",
        "content": [ { "type": "text", "text": "cached" } ],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 20,
            "output_tokens": 8,
            "cache_read_input_tokens": 512,
            "cache_creation_input_tokens": 128
        }
    });
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
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
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .expect("spawn ok");

    let cost = session.total_cost().await.unwrap();
    assert_eq!(cost.input_tokens, 20);
    assert_eq!(cost.output_tokens, 8);
    assert_eq!(cost.cache_read_tokens, 512, "cache read tokens captured");
    assert_eq!(
        cost.cache_creation_tokens, 128,
        "cache creation tokens captured"
    );
}

#[tokio::test]
async fn multi_turn_accumulates() {
    let server = MockServer::start().await;
    let url_path = "/v1/projects/p/locations/us-east5/publishers/anthropic/models/claude-sonnet-4@20251015:rawPredict";

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

    // Disable retries so the test asserts mapping, not backoff timing.
    let executor = VertexExecutor::with_static_token("p", "us-east5", "tok")
        .with_api_root(server.uri())
        .with_retry_policy(RetryPolicy::none());
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
        .respond_with(ResponseTemplate::new(200).set_body_raw("not-json", "application/json"))
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
    let url_path = "/v1/projects/p/locations/us-east5/publishers/anthropic/models/claude-sonnet-4@20251015:rawPredict";

    // First turn burns the budget.
    Mock::given(method("POST"))
        .and(path(url_path))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_body("ok", 10_000_000, 10_000_000)),
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

#[tokio::test]
async fn transient_503_then_success_is_retried() {
    let server = MockServer::start().await;
    let url_path = "/v1/projects/p/locations/us-east5/publishers/anthropic/models/claude-sonnet-4@20251015:rawPredict";

    // First call → 503 (transient). Second call → 200.
    Mock::given(method("POST"))
        .and(path(url_path))
        .respond_with(ResponseTemplate::new(503).set_body_string("scaling up"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(url_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("recovered", 5, 3)))
        .mount(&server)
        .await;

    // Fast backoff so the test stays snappy.
    let policy = RetryPolicy {
        max_retries: 2,
        base_backoff: std::time::Duration::from_millis(1),
        max_backoff: std::time::Duration::from_millis(5),
    };
    let executor = VertexExecutor::with_static_token("p", "us-east5", "tok")
        .with_api_root(server.uri())
        .with_retry_policy(policy);
    let cfg = SpawnConfig {
        model: Some("claude-sonnet-4@20251015".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };

    let session = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .expect("spawn should succeed after retry");
    let cost = session.total_cost().await.unwrap();
    // Cost recorded exactly once — the retried (failed) attempt added nothing.
    assert_eq!(cost.input_tokens, 5);
    assert_eq!(cost.output_tokens, 3);
}

#[tokio::test]
async fn retry_budget_exhaustion_surfaces_last_error() {
    let server = MockServer::start().await;
    // Always 429 — should exhaust the retry budget and surface RateLimited.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_string("still throttled"))
        .mount(&server)
        .await;

    let policy = RetryPolicy {
        max_retries: 2,
        base_backoff: std::time::Duration::from_millis(1),
        max_backoff: std::time::Duration::from_millis(5),
    };
    let executor = VertexExecutor::with_static_token("p", "us-east5", "tok")
        .with_api_root(server.uri())
        .with_retry_policy(policy);
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
async fn fatal_4xx_is_not_retried() {
    let server = MockServer::start().await;
    // A 400 must be returned immediately and only once — never retried.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .expect(1)
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
    // The `.expect(1)` mount assertion verifies no retry happened on drop.
}

#[tokio::test]
async fn spawn_config_retry_policy_overrides_executor_default() {
    // The executor is built with the *default* (retrying) policy, but the
    // per-spawn `SpawnConfig::retry_policy` is `none()`. Parity contract: the
    // config wins, so a transient 503 must surface immediately with NO retry.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).set_body_string("scaling up"))
        // If the config's `none()` policy is honored, the endpoint is hit
        // exactly once. If the executor default (3 retries) leaked through,
        // this would be hit 4 times and the assertion fails on drop.
        .expect(1)
        .mount(&server)
        .await;

    // Executor default = RetryPolicy::default() (retries enabled).
    let executor =
        VertexExecutor::with_static_token("p", "us-east5", "tok").with_api_root(server.uri());

    let cfg = SpawnConfig {
        model: Some("claude-sonnet-4@20251015".into()),
        budget_usd: Some(5.0),
        retry_policy: RetryPolicy::none(),
        ..Default::default()
    };

    let err = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .unwrap_err();
    assert!(matches!(err, AgentError::RateLimited { .. }));
}

#[tokio::test]
async fn query_stream_emits_api_retry_on_transient_then_completes() {
    let server = MockServer::start().await;
    let url_path = "/v1/projects/p/locations/us-east5/publishers/anthropic/models/claude-sonnet-4@20251015:rawPredict";

    // Spawn's first turn succeeds so we can obtain a session, then the
    // streamed turn hits a transient 503 before recovering.
    Mock::given(method("POST"))
        .and(path(url_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("hello", 1, 1)))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(url_path))
        .respond_with(ResponseTemplate::new(503).set_body_string("scaling up"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(url_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("streamed", 4, 2)))
        .mount(&server)
        .await;

    let policy = RetryPolicy {
        max_retries: 2,
        base_backoff: std::time::Duration::from_millis(1),
        max_backoff: std::time::Duration::from_millis(5),
    };
    let executor = VertexExecutor::with_static_token("p", "us-east5", "tok")
        .with_api_root(server.uri())
        .with_retry_policy(policy);
    let cfg = SpawnConfig {
        model: Some("claude-sonnet-4@20251015".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };

    let session = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .expect("spawn ok");

    let mut stream = session.query_stream("again").await.expect("stream ok");
    let mut saw_api_retry = false;
    let mut saw_result_done = false;
    while let Some(evt) = stream.next().await {
        match evt.expect("event ok") {
            MessageEvent::ApiRetry { attempt, .. } => {
                assert_eq!(attempt, 1);
                saw_api_retry = true;
            }
            MessageEvent::ResultDone { content, .. } => {
                assert_eq!(content, "streamed");
                saw_result_done = true;
            }
            _ => {}
        }
    }
    assert!(saw_api_retry, "query_stream must surface an ApiRetry event");
    assert!(
        saw_result_done,
        "query_stream must terminate with ResultDone"
    );
}
