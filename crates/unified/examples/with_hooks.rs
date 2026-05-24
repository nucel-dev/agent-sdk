//! with_hooks — demonstrates [`HookConfig`] (Claude Code only).
//!
//! Hooks are shell commands the provider runs at lifecycle points (before/after
//! every tool use, on session stop, on user prompt submit). They receive the
//! hook context on stdin as JSON. Common uses:
//!
//! - audit logging ("agent just ran `rm -rf node_modules`")
//! - sandbox enforcement (deny tool use when the matcher fires)
//! - integration glue (poke metrics / Slack / a queue)
//!
//! Codex and OpenCode currently no-op on hooks with a debug log.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example with_hooks -- /path/to/repo
//! ```

use std::path::PathBuf;

use nucel_agent_sdk::{
    AgentExecutor, ClaudeCodeExecutor, HookConfig, HookHandler, SpawnConfig,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let working_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let executor = ClaudeCodeExecutor::new();
    if !executor.capabilities().hooks {
        eprintln!("This provider doesn't support hooks — exiting.");
        return Ok(());
    }
    let avail = executor.availability();
    if !avail.available {
        eprintln!(
            "Claude Code not available: {}",
            avail.reason.unwrap_or_else(|| "<no reason>".into())
        );
        return Ok(());
    }

    // Build a HookConfig with one handler per lifecycle point.
    //
    // Each command receives the hook payload on stdin as JSON, so a real
    // implementation typically pipes to `jq` or invokes a small script.
    let hook_config = HookConfig {
        // Run BEFORE the model invokes a tool. Use this for sandboxing /
        // access control — exit non-zero from the script to block the tool.
        pre_tool_use: Some(
            HookHandler::new(
                "sh -c 'echo \"[pre_tool_use] $(date -u +%FT%TZ) $(cat)\" >> /tmp/agent-hooks.log'",
            )
            // Provider-specific matcher — Claude Code accepts a tool-name regex.
            .with_matcher("Bash|Edit|Write")
            .with_timeout(5),
        ),
        // Run AFTER a tool finishes. Use this for audit logs, metrics, etc.
        post_tool_use: Some(
            HookHandler::new(
                "sh -c 'echo \"[post_tool_use] $(date -u +%FT%TZ) $(cat)\" >> /tmp/agent-hooks.log'",
            )
            .with_timeout(5),
        ),
        // Fires when the user (you) submits a new prompt.
        user_prompt_submit: Some(HookHandler::new(
            "sh -c 'echo \"[user_prompt_submit] $(cat)\" >> /tmp/agent-hooks.log'",
        )),
        // Fires when the session terminates.
        on_stop: Some(HookHandler::new(
            "sh -c 'echo \"[on_stop] session ending $(cat)\" >> /tmp/agent-hooks.log'",
        )),
    };

    println!("Spawning Claude Code with hooks (log → /tmp/agent-hooks.log)…");
    let session = executor
        .spawn(
            &working_dir,
            "List the files in this directory using the appropriate tool.",
            &SpawnConfig {
                budget_usd: Some(0.50),
                max_turns: Some(3),
                hook_config: Some(hook_config),
                ..Default::default()
            },
        )
        .await?;

    println!("session_id = {}", session.session_id);

    let resp = session.query("Now summarize what you found in one sentence.").await?;
    println!("\n--- response ---\n{}\n----------------", resp.content);

    let cost = session.total_cost().await?;
    println!("total spent: ${:.4}", cost.total_usd);

    session.close().await?;

    println!("\nCheck /tmp/agent-hooks.log to see the captured lifecycle events.");
    Ok(())
}
