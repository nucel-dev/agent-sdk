//! Integration tests for `nucel-agent-bedrock`.
//!
//! Uses `aws-smithy-mocks` to intercept Converse requests so the test
//! suite never hits real AWS. Each test wires up a mock rule, builds the
//! `BedrockExecutor` from that client, and asserts on the resulting
//! `AgentResponse` / cost accumulation.

use std::path::Path;

// The operation output (struct) and the wire `ConverseOutput` enum share
// a name in the SDK; alias them to keep the intent obvious.
use aws_sdk_bedrockruntime::operation::converse::{
    ConverseError, ConverseOutput as ConverseOpOutput,
};
use aws_sdk_bedrockruntime::types::error::{ServiceUnavailableException, ThrottlingException};
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, ConverseOutput as ConverseOutputUnion, Message, StopReason,
    TokenUsage,
};
use aws_smithy_mocks::{RuleMode, mock, mock_client};

use nucel_agent_bedrock::BedrockExecutor;
use nucel_agent_core::{AgentError, AgentExecutor, ExecutorType, SpawnConfig};

fn build_converse_op_output(text: &str, input: i32, output: i32) -> ConverseOpOutput {
    build_converse_op_output_cached(text, input, output, None, None)
}

/// Like [`build_converse_op_output`] but also sets the prompt-cache token
/// counters Bedrock returns when a `cachePoint` was used.
fn build_converse_op_output_cached(
    text: &str,
    input: i32,
    output: i32,
    cache_read: Option<i32>,
    cache_write: Option<i32>,
) -> ConverseOpOutput {
    let assistant_msg = Message::builder()
        .role(ConversationRole::Assistant)
        .content(ContentBlock::Text(text.to_string()))
        .build()
        .expect("build assistant message");

    let mut usage = TokenUsage::builder()
        .input_tokens(input)
        .output_tokens(output)
        .total_tokens(input + output);
    if let Some(r) = cache_read {
        usage = usage.cache_read_input_tokens(r);
    }
    if let Some(w) = cache_write {
        usage = usage.cache_write_input_tokens(w);
    }
    let usage = usage.build().expect("build usage");

    ConverseOpOutput::builder()
        .output(ConverseOutputUnion::Message(assistant_msg))
        .usage(usage)
        .stop_reason(StopReason::EndTurn)
        .build()
        .expect("build converse output")
}

#[tokio::test]
async fn spawn_first_turn_records_cost_and_transcript() {
    let rule = mock!(aws_sdk_bedrockruntime::Client::converse)
        .then_output(|| build_converse_op_output("hello from claude", 42, 17));

    let client = mock_client!(aws_sdk_bedrockruntime, RuleMode::Sequential, &[&rule]);

    let executor = BedrockExecutor::from_client(client);

    let cfg = SpawnConfig {
        model: Some("anthropic.claude-sonnet-4-20251015-v1:0".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };

    let session = executor
        .spawn(Path::new("/tmp"), "say hi", &cfg)
        .await
        .expect("spawn should succeed");

    assert_eq!(session.executor_type, ExecutorType::ClaudeCode);

    let cost = session.total_cost().await.expect("read total cost");
    assert_eq!(cost.input_tokens, 42);
    assert_eq!(cost.output_tokens, 17);
    assert!(cost.total_usd > 0.0, "expected non-zero estimate");
    assert_eq!(rule.num_calls(), 1);
}

#[tokio::test]
async fn multi_turn_accumulates_cost() {
    // `then_output` only sets a single response; for multi-turn we use the
    // sequence builder so the same rule can match repeatedly.
    let rule = mock!(aws_sdk_bedrockruntime::Client::converse)
        .sequence()
        .output(|| build_converse_op_output("turn-1", 10, 5))
        .output(|| build_converse_op_output("turn-2", 20, 7))
        .build();

    let client = mock_client!(aws_sdk_bedrockruntime, RuleMode::Sequential, &[&rule]);

    let executor = BedrockExecutor::from_client(client);
    let cfg = SpawnConfig {
        model: Some("anthropic.claude-sonnet-4-20251015-v1:0".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };

    let session = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .expect("spawn");

    let resp = session.query("again").await.expect("second turn");
    assert_eq!(resp.content, "turn-2");

    let cost = session.total_cost().await.unwrap();
    assert_eq!(cost.input_tokens, 30);
    assert_eq!(cost.output_tokens, 12);
    assert_eq!(rule.num_calls(), 2);
}

#[tokio::test]
async fn budget_exceeded_short_circuits_next_turn() {
    // First turn comes back with absurd token counts that blow the budget.
    let rule = mock!(aws_sdk_bedrockruntime::Client::converse)
        .then_output(|| build_converse_op_output("ok", 10_000_000, 10_000_000));

    let client = mock_client!(aws_sdk_bedrockruntime, RuleMode::Sequential, &[&rule]);

    let executor = BedrockExecutor::from_client(client);
    let cfg = SpawnConfig {
        model: Some("anthropic.claude-sonnet-4-20251015-v1:0".into()),
        budget_usd: Some(0.01), // 1 cent cap
        ..Default::default()
    };

    let session = executor
        .spawn(Path::new("/tmp"), "burn budget", &cfg)
        .await
        .expect("spawn ok");

    // Now any further turn should be rejected before hitting the network.
    let err = session.query("again").await.unwrap_err();
    assert!(matches!(
        err,
        nucel_agent_core::AgentError::BudgetExceeded { .. }
    ));
    assert_eq!(rule.num_calls(), 1, "second turn must not hit the wire");
}

#[tokio::test]
async fn unknown_model_falls_back_to_zero_cost() {
    let rule = mock!(aws_sdk_bedrockruntime::Client::converse)
        .then_output(|| build_converse_op_output("hi", 100, 50));

    let client = mock_client!(aws_sdk_bedrockruntime, RuleMode::Sequential, &[&rule]);
    let executor = BedrockExecutor::from_client(client);

    let cfg = SpawnConfig {
        model: Some("meta.llama3-70b-instruct-v1:0".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };
    let session = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .expect("spawn ok");

    let cost = session.total_cost().await.unwrap();
    assert_eq!(cost.input_tokens, 100);
    assert_eq!(cost.output_tokens, 50);
    assert_eq!(cost.total_usd, 0.0);
}

#[tokio::test]
async fn resume_is_not_supported() {
    // Build with a dummy real config (no calls will be made).
    let conf = aws_sdk_bedrockruntime::Config::builder()
        .behavior_version(aws_sdk_bedrockruntime::config::BehaviorVersion::latest())
        .region(aws_sdk_bedrockruntime::config::Region::new("us-east-1"))
        .build();
    let client = aws_sdk_bedrockruntime::Client::from_conf(conf);
    let executor = BedrockExecutor::from_client(client);

    let err = executor
        .resume(
            Path::new("/tmp"),
            "some-session",
            "hi",
            &SpawnConfig::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(err, nucel_agent_core::AgentError::Provider { .. }));
}

#[tokio::test]
async fn throttling_maps_to_rate_limited() {
    // A Bedrock `ThrottlingException` must surface as the transient
    // `RateLimited` variant (not an opaque `Provider` error) so callers and the
    // umbrella's `retry::is_transient` can treat it as a back-off signal.
    let rule = mock!(aws_sdk_bedrockruntime::Client::converse).then_error(|| {
        ConverseError::ThrottlingException(
            ThrottlingException::builder()
                .message("Too many requests, please wait")
                .build(),
        )
    });
    let client = mock_client!(aws_sdk_bedrockruntime, RuleMode::Sequential, &[&rule]);
    let executor = BedrockExecutor::from_client(client);

    let cfg = SpawnConfig {
        model: Some("anthropic.claude-sonnet-4-20251015-v1:0".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };
    let err = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .unwrap_err();
    assert!(
        matches!(err, AgentError::RateLimited { .. }),
        "throttling must map to RateLimited, got {err:?}"
    );
}

#[tokio::test]
async fn service_unavailable_maps_to_rate_limited() {
    // A 503-style `ServiceUnavailableException` is transient (upstream
    // overloaded / scaling) → `RateLimited`, mirroring the Vertex provider.
    let rule = mock!(aws_sdk_bedrockruntime::Client::converse).then_error(|| {
        ConverseError::ServiceUnavailableException(
            ServiceUnavailableException::builder()
                .message("Service is temporarily unavailable")
                .build(),
        )
    });
    let client = mock_client!(aws_sdk_bedrockruntime, RuleMode::Sequential, &[&rule]);
    let executor = BedrockExecutor::from_client(client);

    let cfg = SpawnConfig {
        model: Some("anthropic.claude-sonnet-4-20251015-v1:0".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };
    let err = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .unwrap_err();
    assert!(
        matches!(err, AgentError::RateLimited { .. }),
        "service-unavailable must map to RateLimited, got {err:?}"
    );
}

#[tokio::test]
async fn cache_tokens_are_captured() {
    // When Bedrock returns cache_read/cache_write input tokens, they must be
    // folded into AgentCost so cost analytics see prompt-cache effects.
    let rule = mock!(aws_sdk_bedrockruntime::Client::converse)
        .then_output(|| build_converse_op_output_cached("cached", 30, 12, Some(100), Some(40)));
    let client = mock_client!(aws_sdk_bedrockruntime, RuleMode::Sequential, &[&rule]);
    let executor = BedrockExecutor::from_client(client);

    let cfg = SpawnConfig {
        model: Some("anthropic.claude-sonnet-4-20251015-v1:0".into()),
        budget_usd: Some(5.0),
        ..Default::default()
    };
    let session = executor
        .spawn(Path::new("/tmp"), "hi", &cfg)
        .await
        .expect("spawn ok");

    let cost = session.total_cost().await.unwrap();
    assert_eq!(cost.input_tokens, 30);
    assert_eq!(cost.output_tokens, 12);
    assert_eq!(cost.cache_read_tokens, 100, "cache read tokens captured");
    assert_eq!(
        cost.cache_creation_tokens, 40,
        "cache write tokens captured"
    );
}
