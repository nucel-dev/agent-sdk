//! budget_control — set `budget_usd`, hit the cap mid-loop, handle
//! [`AgentError::BudgetExceeded`] gracefully.
//!
//! This example uses an intentionally tiny cap so the very first or second
//! turn trips it. In a real app you'd:
//!
//! 1. Set a per-session ceiling via `SpawnConfig::budget_usd`.
//! 2. Drive a multi-turn loop with `session.query(...)`.
//! 3. On `BudgetExceeded`, log the spend, close the session, and stop. The
//!    session is dead — subsequent calls will keep returning the same error.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example budget_control -- /path/to/repo
//! ```

use std::path::PathBuf;

use nucel_agent_sdk::{AgentError, AgentExecutor, ClaudeCodeExecutor, SpawnConfig};

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

    // Deliberately tiny cap: a few cents. Most prompts will trip it on turn 1.
    let cap_usd: f64 = 0.05;

    println!("spawning with hard cap ${:.2}…", cap_usd);
    let session = match executor
        .spawn(
            &working_dir,
            "Read every file in this repo, summarize each, then write a 500-word \
             README describing the architecture. Be thorough.",
            &SpawnConfig {
                budget_usd: Some(cap_usd),
                max_turns: Some(20),
                ..Default::default()
            },
        )
        .await
    {
        Ok(s) => s,
        Err(AgentError::BudgetExceeded { spent, limit }) => {
            // Even spawn can trip the cap (e.g. cap is 0 or the first turn is huge).
            eprintln!("spawn already over budget: spent ${spent:.4} of ${limit:.4} — exiting");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    println!("session_id = {}", session.session_id);

    // Multi-turn loop. Each follow-up will likely trip the cap.
    let prompts = [
        "Summarize file 1.",
        "Summarize file 2.",
        "Summarize file 3.",
        "Combine into final README.",
    ];

    let mut turn = 0;
    for prompt in prompts {
        turn += 1;
        match session.query(prompt).await {
            Ok(resp) => {
                let cost = session.total_cost().await.unwrap_or_default();
                println!(
                    "[turn {turn}] ok — running total ${:.4}\n  {}",
                    cost.total_usd,
                    first_line(&resp.content)
                );
            }
            Err(AgentError::BudgetExceeded { spent, limit }) => {
                // Terminal: subsequent query() calls keep returning this. Stop.
                eprintln!("\n[turn {turn}] budget hit: spent ${spent:.4} of ${limit:.4}");
                eprintln!("closing session and bailing out cleanly.");
                break;
            }
            Err(e) => {
                eprintln!("[turn {turn}] non-budget error: {e}");
                break;
            }
        }
    }

    // Always sample final cost even after BudgetExceeded — providers usually
    // still report the spend up to the failure point.
    let final_cost = session.total_cost().await.unwrap_or_default();
    println!(
        "\nfinal cost: ${:.4}  ({} in / {} out tokens)",
        final_cost.total_usd, final_cost.input_tokens, final_cost.output_tokens
    );

    session.close().await?;
    Ok(())
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("").trim()
}
