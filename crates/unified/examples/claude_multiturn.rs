//! claude_multiturn — one live session, several sequential `query()` calls.
//!
//! Unlike `resume_session` (which re-establishes the session across processes
//! via `session_id`), this keeps a **single subprocess alive** and sends each
//! follow-up prompt over its stdin. The conversation context is preserved by
//! the CLI between turns, so later prompts can refer back to earlier answers.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example claude_multiturn -- /path/to/repo
//! ```
//!
//! Requires the `claude` CLI on `$PATH`:
//!
//! ```bash
//! npm install -g @anthropic-ai/claude-code
//! ```

use std::path::PathBuf;

use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let working_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let executor = ClaudeCodeExecutor::new();
    let avail = executor.availability();
    if !avail.available {
        eprintln!(
            "Claude Code not available: {}",
            avail.reason.unwrap_or_else(|| "<no reason>".into())
        );
        return Ok(());
    }

    println!("Spawning Claude Code in {}…", working_dir.display());
    let session = executor
        .spawn(
            &working_dir,
            "In one sentence, what does this codebase do?",
            &SpawnConfig {
                model: Some("claude-opus-5".into()),
                // One budget cap covers the whole multi-turn session — cost
                // accumulates across every `query()` call below.
                budget_usd: Some(2.0),
                max_turns: Some(4),
                ..Default::default()
            },
        )
        .await?;

    println!("session_id = {}\n", session.session_id);

    // Each follow-up reuses the same live subprocess; the CLI keeps context.
    let follow_ups = [
        "Name the single most important module.",
        "Why is that one the most important — one sentence.",
        "What would you change first if you owned it?",
    ];

    for (i, prompt) in follow_ups.iter().enumerate() {
        let resp = session.query(prompt).await?;
        println!("--- turn {} ---\n{}\n", i + 1, resp.content.trim());
    }

    let cost = session.total_cost().await?;
    println!(
        "cumulative tokens in/out: {}/{}  (cache r/w: {}/{})  total: ${:.4}",
        cost.input_tokens,
        cost.output_tokens,
        cost.cache_read_tokens,
        cost.cache_creation_tokens,
        cost.total_usd,
    );

    session.close().await?;
    Ok(())
}
