//! Minimal example: spawn a Claude Code session, run one query, print the result.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example spawn-claude -- /path/to/repo "your prompt"
//! ```
//!
//! Requirements:
//!   - `claude` CLI on PATH: `npm install -g @anthropic-ai/claude-code`
//!   - `ANTHROPIC_API_KEY` env var, or a logged-in `claude` session.

use std::path::Path;

use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI args (very basic).
    let mut args = std::env::args().skip(1);
    let working_dir = args
        .next()
        .unwrap_or_else(|| std::env::current_dir().unwrap().to_string_lossy().into_owned());
    let prompt = args
        .next()
        .unwrap_or_else(|| "Summarize this repository in one paragraph.".to_string());

    let executor = ClaudeCodeExecutor::new();

    // Availability probe.
    let avail = executor.availability();
    if !avail.available {
        eprintln!("claude not available: {}", avail.reason.unwrap_or_default());
        return Ok(());
    }

    // Spawn a session.
    let session = executor
        .spawn(
            Path::new(&working_dir),
            &prompt,
            &SpawnConfig {
                budget_usd: Some(1.0),
                max_turns: Some(5),
                ..Default::default()
            },
        )
        .await?;

    // The first prompt's response is consumed during spawn. Issue a follow-up.
    let resp = session.query("Anything you'd flag as a next step?").await?;
    println!("--- response ---\n{}", resp.content);

    let cost = session.total_cost().await?;
    println!(
        "--- cost --- input_tokens={} output_tokens={} total=${:.4}",
        cost.input_tokens, cost.output_tokens, cost.total_usd
    );

    session.close().await?;
    Ok(())
}
