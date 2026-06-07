//! Wiremock tests for the **executor-level** retry knob and its precedence
//! against the per-spawn [`SpawnConfig::retry_policy`].
//!
//! `vertex_integration.rs` already covers the *config-wins* direction (a
//! per-spawn `none()` suppressing the executor default) and the streaming
//! `ApiRetry` event. This file pins the complementary branch of
//! `effective_retry`: when the per-spawn config is left at its default
//! sentinel, the **builder-level** policy set via `with_retry_policy` is the
//! one that governs the request — including disabling retries entirely and
//! bounding the retry count.

use std::path::Path;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use nucel_agent_core::{AgentError, AgentExecutor, RetryPolicy, SpawnConfig};
use nucel_agent_vertex::VertexExecutor;

const MODEL: &str = "claude-sonnet-4@20251015";

fn ok_body(text: &str, input: u64, output: u64) -> serde_json::Value {
    json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": MODEL,
        "content": [ { "type": "text", "text": text } ],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": input, "output_tokens": output }
    })
}

/// Fast, deterministic policy so retry tests stay snappy (no real backoff).
fn fast_retry(max_retries: u32) -> RetryPolicy {
    RetryPolicy {
        max_retries,
        base_backoff: std::time::Duration::from_millis(1),
        max_backoff: std::time::Duration::from_millis(5),
    }
}

/// Executor built with `RetryPolicy::none()`, spawn driven by a *default*
/// config. Because the config is at the default sentinel, `effective_retry`
/// must fall through to the executor's `none()` policy — so a transient 503 is
/// surfaced immediately and the endpoint is hit exactly once.
///
/// This is the mirror image of `spawn_config_retry_policy_overrides_executor_default`
/// in the integration suite (there the config disables retries; here the
/// *builder* does, with the config left untouched).
#[tokio::test]
async fn executor_level_none_disables_retry_when_config_is_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).set_body_string("scaling up"))
        // `none()` on the executor must yield exactly one dispatch.
        .expect(1)
        .mount(&server)
        .await;

    let executor = VertexExecutor::with_static_token("p", "us-east5", "tok")
        .with_api_root(server.uri())
        .with_retry_policy(RetryPolicy::none());

    // Default config → `retry_policy == RetryPolicy::default()` (the sentinel).
    let cfg = SpawnConfig {
        model: Some(MODEL.into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };

    let err = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .unwrap_err();
    assert!(
        matches!(err, AgentError::RateLimited { .. }),
        "transient 503 surfaces as RateLimited, got {err:?}"
    );
    // `.expect(1)` verifies on drop that no retry happened.
}

/// Executor built with a custom *retrying* policy (2 retries), spawn driven by
/// a default config. The builder policy must govern: two transient 503s are
/// retried and the third attempt (a 200) succeeds. This isolates that the
/// executor knob — not the config — drove the retry budget.
#[tokio::test]
async fn executor_level_policy_drives_retry_count_when_config_is_default() {
    let server = MockServer::start().await;
    let url_path = format!(
        "/v1/projects/p/locations/us-east5/publishers/anthropic/models/{MODEL}:rawPredict"
    );

    // Two transient failures...
    Mock::given(method("POST"))
        .and(path(url_path.clone()))
        .respond_with(ResponseTemplate::new(503).set_body_string("scaling up"))
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    // ...then success.
    Mock::given(method("POST"))
        .and(path(url_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("recovered", 5, 3)))
        .mount(&server)
        .await;

    let executor = VertexExecutor::with_static_token("p", "us-east5", "tok")
        .with_api_root(server.uri())
        .with_retry_policy(fast_retry(2));

    let cfg = SpawnConfig {
        model: Some(MODEL.into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };

    let session = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .expect("spawn should recover after two retries");

    let cost = session.total_cost().await.unwrap();
    assert_eq!(cost.input_tokens, 5, "cost comes from the successful attempt");
    assert_eq!(cost.output_tokens, 3);
}

/// When BOTH knobs are set to non-default values, the per-spawn config must win
/// (it is the more specific intent). Executor allows 2 retries, config allows
/// 1: a stream of 503s must exhaust after exactly the *config's* budget — i.e.
/// 2 total dispatches (1 initial + 1 retry), not 3.
#[tokio::test]
async fn config_policy_beats_executor_policy_when_both_non_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).set_body_string("still scaling"))
        // config budget = 1 retry → exactly 2 dispatches total.
        .expect(2)
        .mount(&server)
        .await;

    let executor = VertexExecutor::with_static_token("p", "us-east5", "tok")
        .with_api_root(server.uri())
        .with_retry_policy(fast_retry(2)); // executor would allow 2 retries

    let cfg = SpawnConfig {
        model: Some(MODEL.into()),
        budget_usd: Some(5.0),
        retry_policy: fast_retry(1), // but config caps at 1 retry
        ..Default::default()
    };

    let err = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .unwrap_err();
    assert!(
        matches!(err, AgentError::RateLimited { .. }),
        "exhausted retries surface as RateLimited, got {err:?}"
    );
    // `.expect(2)` confirms the config's 1-retry budget (not the executor's 2)
    // governed the loop.
}
