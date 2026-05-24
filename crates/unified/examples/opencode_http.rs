//! opencode_http — spawn a session against a local `opencode serve`.
//!
//! Start the server in another shell:
//!
//! ```bash
//! opencode serve --port 4096
//! ```
//!
//! Then run:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example opencode_http -- /path/to/repo
//! ```
//!
//! Optionally set `OPENCODE_BASE_URL` to point at a non-default URL, and
//! `OPENCODE_API_KEY` for HTTP basic auth.

use std::path::PathBuf;

use nucel_agent_sdk::{AgentExecutor, OpencodeExecutor, SpawnConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let working_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let base_url =
        std::env::var("OPENCODE_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:4096".into());

    let mut executor = OpencodeExecutor::with_base_url(&base_url);
    if let Ok(key) = std::env::var("OPENCODE_API_KEY") {
        executor = executor.with_api_key(key);
    }

    // OpencodeExecutor::availability always reports `available = true` and
    // surfaces the URL as `reason` — the server existence is only verified
    // when we actually issue a request.
    let avail = executor.availability();
    println!(
        "target: {}  (reason: {})",
        base_url,
        avail.reason.unwrap_or_default(),
    );

    let session = executor
        .spawn(
            &working_dir,
            "Summarize the top-level directory layout in two sentences.",
            &SpawnConfig {
                budget_usd: Some(0.5),
                ..Default::default()
            },
        )
        .await?;

    println!("session_id = {}", session.session_id);
    let resp = session
        .query("Is there a CONTRIBUTING.md? If so, what's its first heading?")
        .await?;
    println!("\n--- response ---\n{}\n----------------", resp.content);

    let cost = session.total_cost().await?;
    println!(
        "tokens in/out: {}/{}    total: ${:.4}",
        cost.input_tokens, cost.output_tokens, cost.total_usd,
    );

    session.close().await?;
    Ok(())
}
