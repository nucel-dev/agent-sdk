//! codex_resume — spawn, save the session id, resume, query.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example codex_resume -- /path/to/repo
//! ```
//!
//! Requires the `codex` CLI on `$PATH` (see https://developers.openai.com/codex/cli/).

use std::path::PathBuf;

use nucel_agent_sdk::{AgentExecutor, CodexExecutor, SpawnConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let working_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let executor = CodexExecutor::new();
    if !executor.availability().available {
        eprintln!("codex CLI not available — install it first.");
        return Ok(());
    }

    let config = SpawnConfig {
        model: Some("gpt-5-codex".into()),
        budget_usd: Some(1.0),
        ..Default::default()
    };

    // 1. Spawn an initial session.
    println!("spawn…");
    let session = executor
        .spawn(
            &working_dir,
            "What language is this project written in? Just the name, one line.",
            &config,
        )
        .await?;

    // 2. Save the session id (in real apps: persist to disk / DB).
    let session_id = session.session_id.clone();
    println!("got session_id = {session_id}");
    let first = session.query("And what's the dependency manager?").await?;
    println!("first.content = {}", first.content);

    // Drop the session — pretend the process exits here.
    session.close().await?;

    // 3. ...later, in a fresh handle, resume by session id.
    println!("\nresume…");
    let resumed = executor
        .resume(
            &working_dir,
            &session_id,
            "Now tell me how to run the tests.",
            &config,
        )
        .await?;

    println!("resumed.session_id = {}", resumed.session_id);
    let second = resumed.query("Any common gotchas?").await?;
    println!("second.content = {}", second.content);

    let cost = resumed.total_cost().await?;
    println!("total spent: ${:.4}", cost.total_usd);

    resumed.close().await?;
    Ok(())
}
