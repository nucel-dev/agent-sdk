//! multi_provider_handoff — run the same prompt against all 3 providers
//! sequentially and print a side-by-side cost comparison.
//!
//! Skips any provider whose CLI / server isn't available locally, so you can
//! run this with just one of them installed and still get useful output.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example multi_provider_handoff -- /path/to/repo
//! ```

use std::path::PathBuf;
use std::time::Instant;

use nucel_agent_sdk::{
    AgentCost, AgentExecutor, ClaudeCodeExecutor, CodexExecutor, OpencodeExecutor, SpawnConfig,
};

const PROMPT: &str = "In one sentence, describe what this codebase does. No preamble, no markdown.";

#[allow(dead_code)]
struct Row {
    provider: &'static str,
    skipped: Option<String>,
    elapsed_ms: u128,
    cost: AgentCost,
    snippet: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let working_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let claude: Box<dyn AgentExecutor> = Box::new(ClaudeCodeExecutor::new());
    let codex: Box<dyn AgentExecutor> = Box::new(CodexExecutor::new());
    let opencode: Box<dyn AgentExecutor> = Box::new(OpencodeExecutor::new());

    let executors: [(&'static str, Box<dyn AgentExecutor>); 3] = [
        ("claude-code", claude),
        ("codex", codex),
        ("opencode", opencode),
    ];

    let mut rows: Vec<Row> = Vec::with_capacity(3);

    for (name, exec) in executors {
        let avail = exec.availability();
        if !avail.available {
            rows.push(Row {
                provider: name,
                skipped: Some(avail.reason.unwrap_or_else(|| "unavailable".into())),
                elapsed_ms: 0,
                cost: AgentCost::default(),
                snippet: String::new(),
            });
            continue;
        }

        println!("\n--- {name} ---");
        let cfg = SpawnConfig {
            // Hard cap so a misbehaving provider can't dominate the comparison.
            budget_usd: Some(0.50),
            max_turns: Some(2),
            ..Default::default()
        };

        let started = Instant::now();
        let result = exec.spawn(&working_dir, PROMPT, &cfg).await;
        match result {
            Ok(session) => {
                // Pull the final cost for the spawn turn.
                let cost = session.total_cost().await.unwrap_or_default();
                let snippet = truncate(
                    // Re-use the response from spawn via a no-op follow-up isn't free,
                    // so we just collect what spawn already returned via total_cost +
                    // a single inexpensive recap.
                    "(see provider stdout)",
                    140,
                );
                let elapsed = started.elapsed().as_millis();
                println!("ok in {elapsed} ms — ${:.4}", cost.total_usd);

                rows.push(Row {
                    provider: name,
                    skipped: None,
                    elapsed_ms: elapsed,
                    cost,
                    snippet,
                });

                let _ = session.close().await;
            }
            Err(e) => {
                eprintln!("error: {e}");
                rows.push(Row {
                    provider: name,
                    skipped: Some(format!("error: {e}")),
                    elapsed_ms: started.elapsed().as_millis(),
                    cost: AgentCost::default(),
                    snippet: String::new(),
                });
            }
        }
    }

    // Pretty-print the comparison table.
    println!("\n\n=== cost comparison ===");
    println!(
        "{:<14} {:>10} {:>10} {:>10} {:>10} {:>14}",
        "provider", "in_tok", "out_tok", "cache_r", "elapsed", "total_usd"
    );
    for row in &rows {
        if let Some(why) = &row.skipped {
            println!("{:<14} (skipped: {why})", row.provider);
            continue;
        }
        println!(
            "{:<14} {:>10} {:>10} {:>10} {:>9}ms {:>13.4}",
            row.provider,
            row.cost.input_tokens,
            row.cost.output_tokens,
            row.cost.cache_read_tokens,
            row.elapsed_ms,
            row.cost.total_usd,
        );
    }

    let total: AgentCost = rows
        .iter()
        .map(|r| r.cost.clone())
        .fold(AgentCost::default(), |acc, c| acc + c);
    println!(
        "\nrun total: ${:.4} across {} providers",
        total.total_usd,
        rows.len()
    );

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
