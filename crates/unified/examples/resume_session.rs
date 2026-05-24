//! resume_session — spawn, save session_id, close, resume, continue.
//!
//! Demonstrates the **cross-process resume** pattern: the second handle to the
//! session doesn't share any in-memory state with the first. The only thing
//! that crosses the boundary is `session_id` (and the `working_dir`), so this
//! exact flow works across crashes, restarts, and across processes / hosts.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example resume_session -- /path/to/repo
//! ```
//!
//! Works with `claude-code`, `codex`, and `opencode` — switch the executor
//! type below to test each.

use std::path::PathBuf;

use nucel_agent_sdk::{AgentExecutor, ClaudeCodeExecutor, SpawnConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let working_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let executor = ClaudeCodeExecutor::new();
    if !executor.capabilities().session_resume {
        eprintln!("This provider doesn't support session_resume.");
        return Ok(());
    }
    let avail = executor.availability();
    if !avail.available {
        eprintln!(
            "Provider unavailable: {}",
            avail.reason.unwrap_or_else(|| "<no reason>".into())
        );
        return Ok(());
    }

    let cfg = SpawnConfig {
        budget_usd: Some(1.0),
        max_turns: Some(4),
        ..Default::default()
    };

    // ---- Phase 1: spawn and do one turn. ----
    println!("phase 1: spawn…");
    let session = executor
        .spawn(
            &working_dir,
            "I'm going to ask you questions about this repo across two \
             different processes. Remember: my favorite color is teal. \
             Confirm you got it.",
            &cfg,
        )
        .await?;

    // SAVE THE SESSION ID. In a real app: persist to disk, Redis, a DB row.
    // Here we just shadow it through a local variable to simulate the boundary.
    let saved_session_id: String = session.session_id.clone();
    println!("saved session_id = {saved_session_id}");

    let resp = session.query("What language is this project written in?").await?;
    println!("phase 1 response: {}", first_line(&resp.content));

    // Phase-1 cost so we can verify cumulative cost survives the resume.
    let cost_after_phase1 = session.total_cost().await.unwrap_or_default();
    println!("phase 1 cost: ${:.4}", cost_after_phase1.total_usd);

    // ---- Boundary: close the session. Pretend the process exits here. ----
    session.close().await?;
    println!("\n--- session closed; pretend the process exited ---\n");

    // ---- Phase 2: resume from session_id alone. ----
    println!("phase 2: resume from saved id…");
    let resumed = executor
        .resume(
            &working_dir,
            &saved_session_id,
            "What was my favorite color, again? Just the color name.",
            &cfg,
        )
        .await?;

    println!("resumed.session_id = {}", resumed.session_id);

    let follow = resumed.query("And how do I run the tests in this repo?").await?;
    println!("phase 2 follow-up: {}", first_line(&follow.content));

    let final_cost = resumed.total_cost().await.unwrap_or_default();
    println!(
        "\nfinal cost (this handle): ${:.4} ({} in / {} out tokens)",
        final_cost.total_usd, final_cost.input_tokens, final_cost.output_tokens,
    );

    resumed.close().await?;
    Ok(())
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("").trim()
}
