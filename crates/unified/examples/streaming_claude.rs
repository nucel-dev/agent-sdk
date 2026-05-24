//! streaming_claude — print tokens as they arrive using the 0.2.0 streaming API.
//!
//! Uses [`AgentSession::query_stream`] to consume [`MessageEvent`]s live instead
//! of waiting for the full response. Useful for terminal UIs, server-sent events,
//! cancellation on budget overruns, and surfacing tool-use as it happens.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example streaming_claude -- /path/to/repo
//! ```
//!
//! Requires the `claude` CLI on `$PATH`:
//!
//! ```bash
//! npm install -g @anthropic-ai/claude-code
//! ```

use std::io::Write;
use std::path::PathBuf;

use futures::StreamExt;
use nucel_agent_sdk::{
    AgentExecutor, ClaudeCodeExecutor, MessageEvent, SpawnConfig,
};

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
            "Give me a two-sentence summary of this repo.",
            &SpawnConfig {
                model: Some("claude-opus-4-6".into()),
                budget_usd: Some(0.50),
                max_turns: Some(2),
                ..Default::default()
            },
        )
        .await?;

    println!("session_id = {}\n", session.session_id);

    // Open a stream and drain events live.
    let mut stream = session
        .query_stream("Now list the top-level directories, one per line.")
        .await?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    while let Some(evt) = stream.next().await {
        match evt? {
            MessageEvent::TextChunk { text } => {
                // Print the chunk and flush so the user sees tokens as they arrive.
                out.write_all(text.as_bytes())?;
                out.flush()?;
            }
            MessageEvent::ToolUse { name, .. } => {
                writeln!(out, "\n[tool start: {name}]")?;
            }
            MessageEvent::ToolResult { success, .. } => {
                writeln!(out, "[tool done: success={success}]")?;
            }
            MessageEvent::Thinking { text } => {
                // Extended-thinking content — only emitted when thinking_budget is set.
                writeln!(out, "\n[thinking] {text}")?;
            }
            MessageEvent::ApiRetry { attempt, message } => {
                writeln!(out, "\n[retry #{attempt}] {message}")?;
            }
            MessageEvent::RateLimit { message } => {
                writeln!(out, "\n[rate limited] {message}")?;
            }
            MessageEvent::ResultDone { cost, is_error, .. } => {
                writeln!(
                    out,
                    "\n\n=> done (error={is_error})  in/out: {}/{}  cache_read: {}  total: ${:.4}",
                    cost.input_tokens,
                    cost.output_tokens,
                    cost.cache_read_tokens,
                    cost.total_usd,
                )?;
                break;
            }
            MessageEvent::Error { message } => {
                writeln!(out, "\n[error] {message}")?;
                break;
            }
            _ => {}
        }
    }

    session.close().await?;
    Ok(())
}
