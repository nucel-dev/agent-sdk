//! bedrock_basic — run Claude on AWS Bedrock through the unified SDK.
//!
//! This is the AWS-native path: no `claude` CLI, no Anthropic API key — just
//! the Bedrock Runtime `Converse` API reached with your standard AWS
//! credentials. Requires the `bedrock` feature:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --features bedrock --example bedrock_basic
//! # or target a specific model / region:
//! cargo run -p nucel-agent-sdk --features bedrock --example bedrock_basic -- \
//!     anthropic.claude-sonnet-4-20251015-v1:0
//! ```
//!
//! Credentials are resolved with the **default AWS provider chain** — env vars
//! (`AWS_ACCESS_KEY_ID` / `AWS_SESSION_TOKEN`), `~/.aws/credentials`, SSO, an
//! ECS task role, or an EC2/EKS instance profile (IRSA). Set the region the
//! usual way, e.g.:
//!
//! ```bash
//! export AWS_REGION=us-east-1
//! aws sso login                 # or: export AWS_ACCESS_KEY_ID=...
//! ```
//!
//! Notes that make Bedrock different from the CLI providers:
//!
//! - **No server-side sessions.** The transcript is kept client-side, so
//!   `resume()` is unsupported — persist the transcript yourself and re-spawn.
//! - **Retries are the AWS SDK's job.** Throttling / 5xx are retried by
//!   `aws-sdk-bedrockruntime` before the SDK ever sees them; what surfaces here
//!   is already classified into the SDK-wide [`AgentError`] taxonomy (a
//!   `ThrottlingException` becomes [`AgentError::RateLimited`], etc.).
//! - **Prompt-cache tokens are captured.** When a `cachePoint` is in play,
//!   `cache_read_tokens` / `cache_creation_tokens` are folded into the cost.

use std::path::Path;

use nucel_agent_sdk::{AgentError, AgentExecutor, BedrockExecutor, SpawnConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Optional positional arg: the Bedrock model id (defaults to the crate's
    // DEFAULT_MODEL — Claude Opus on Bedrock).
    let model = std::env::args().nth(1);

    // Build the executor straight from the provider crate so we stay fully
    // async (the `build_executor("bedrock", _)` umbrella helper spins a short
    // throwaway runtime for the credential lookup — fine for sync startup, but
    // unnecessary here).
    let executor = BedrockExecutor::new().await;

    let avail = executor.availability();
    println!(
        "bedrock availability: available={} reason={:?}",
        avail.available, avail.reason
    );
    if !avail.available {
        eprintln!(
            "No AWS credentials found. Configure them and re-run — see the \
             module docs at the top of this file."
        );
        // Continue anyway so the actual SDK error reaches the caller; on a CI
        // box without creds this prints the underlying auth failure.
    }

    let cfg = SpawnConfig {
        model,
        budget_usd: Some(1.0),
        max_tokens: Some(512),
        system_prompt: Some("You are a terse code reviewer. No preamble.".into()),
        ..Default::default()
    };

    let session = match executor
        .spawn(
            Path::new("."),
            "In one sentence, what does this directory appear to contain?",
            &cfg,
        )
        .await
    {
        Ok(s) => s,
        Err(AgentError::RateLimited { message }) => {
            // The AWS SDK already exhausted its retry budget before we saw this.
            eprintln!("Bedrock throttled even after SDK retries: {message}");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    println!("session_id = {}", session.session_id);

    // Multi-turn: the transcript carries over client-side.
    let resp = session.query("Now list one risk you'd watch for.").await?;
    println!("response: {}", resp.content);

    let cost = session.total_cost().await?;
    println!(
        "total cost: in/out {}/{} tokens (cache r/w {}/{}), ${:.4}",
        cost.input_tokens,
        cost.output_tokens,
        cost.cache_read_tokens,
        cost.cache_creation_tokens,
        cost.total_usd,
    );

    session.close().await?;
    Ok(())
}
