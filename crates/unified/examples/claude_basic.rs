//! claude_basic — spawn → query → close against the Claude Code provider.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example claude_basic -- /path/to/repo
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
    // First CLI arg, or the current dir.
    let working_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let executor = ClaudeCodeExecutor::new();

    // Check the CLI is installed before spending real money.
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
            "Give me a one-paragraph summary of this codebase.",
            &SpawnConfig {
                // Pin a model so cost is predictable.
                model: Some("claude-opus-5".into()),
                // Hard cap to avoid runaway spend in demos.
                budget_usd: Some(1.0),
                // Keep autonomous loops short.
                max_turns: Some(4),
                ..Default::default()
            },
        )
        .await?;

    println!("session_id = {}", session.session_id);

    // Follow-up query reusing the same session.
    let resp = session.query("Now list the top three modules.").await?;
    println!("\n--- response ---\n{}\n----------------", resp.content);

    let cost = session.total_cost().await?;
    println!(
        "tokens in/out: {}/{}    total: ${:.4}",
        cost.input_tokens, cost.output_tokens, cost.total_usd,
    );

    session.close().await?;
    Ok(())
}
