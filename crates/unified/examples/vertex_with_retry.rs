//! vertex_with_retry — construct a Vertex executor with a custom retry policy,
//! run a turn, and handle the result. Requires the `vertex` feature:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --features vertex --example vertex_with_retry -- my-gcp-project us-east5
//! ```
//!
//! Authentication uses GCP Application Default Credentials. Make sure one of
//! these is in place first:
//!
//! ```bash
//! gcloud auth application-default login
//! # or export GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json
//! ```
//!
//! Transient failures (connection drops, `429`, `503` *before* any response
//! body is read) are retried automatically with exponential backoff. Once the
//! model starts streaming a `2xx` body, errors are fatal — the SDK never
//! replays a turn that already produced output / incurred cost.

use std::path::Path;

use nucel_agent_sdk::{AgentExecutor, RetryPolicy, SpawnConfig};
use nucel_agent_vertex::VertexExecutor;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let project = args.next().unwrap_or_else(|| {
        eprintln!("usage: vertex_with_retry <gcp-project> <region>");
        std::process::exit(2);
    });
    let region = args.next().unwrap_or_else(|| "us-east5".to_string());

    // Build via ADC, then override the retry policy: 5 retries, 500 ms base,
    // capped at 10 s. Pass `RetryPolicy::none()` to opt out of retrying.
    let executor = VertexExecutor::with_adc(&project, &region)
        .await?
        .with_retry_policy(RetryPolicy {
            max_retries: 5,
            base_backoff: std::time::Duration::from_millis(500),
            max_backoff: std::time::Duration::from_secs(10),
        });

    let avail = executor.availability();
    println!(
        "vertex availability: available={} reason={:?}",
        avail.available, avail.reason
    );

    let session = executor
        .spawn(
            Path::new("."),
            "In one sentence, what is the capital of France?",
            &SpawnConfig {
                model: Some("claude-opus-4-7@20251024".into()),
                budget_usd: Some(1.0),
                max_tokens: Some(256),
                ..Default::default()
            },
        )
        .await?;

    println!("session_id = {}", session.session_id);

    let resp = session.query("And of Japan?").await?;
    println!("response: {}", resp.content);

    let cost = session.total_cost().await?;
    println!(
        "total cost: in/out {}/{} tokens, ${:.4}",
        cost.input_tokens, cost.output_tokens, cost.total_usd
    );

    session.close().await?;
    Ok(())
}
