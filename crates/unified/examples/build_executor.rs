//! build_executor — pick a provider at runtime via a config string.
//!
//! Mirrors how higher-level orchestrators (e.g. `agent-operator`) swap
//! providers via `providers.agent = "claude-code" | "codex" | "opencode"`.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example build_executor -- claude-code
//! cargo run -p nucel-agent-sdk --example build_executor -- codex
//! cargo run -p nucel-agent-sdk --example build_executor -- opencode http://127.0.0.1:4096
//! ```

use nucel_agent_sdk::{available_providers, build_executor};

fn main() {
    let mut args = std::env::args().skip(1);
    let name = match args.next() {
        Some(n) => n,
        None => {
            eprintln!(
                "usage: build_executor <provider> [base_url]\n\navailable: {}",
                available_providers().join(", "),
            );
            std::process::exit(2);
        }
    };
    let base_url_or_key = args.next();

    match build_executor(&name, base_url_or_key) {
        Some(exec) => {
            let caps = exec.capabilities();
            let avail = exec.availability();
            println!("provider:          {}", exec.executor_type());
            println!("session_resume:    {}", caps.session_resume);
            println!("token_usage:       {}", caps.token_usage);
            println!("mcp_support:       {}", caps.mcp_support);
            println!("autonomous_mode:   {}", caps.autonomous_mode);
            println!("structured_output: {}", caps.structured_output);
            println!("available:         {}", avail.available);
            if let Some(reason) = avail.reason {
                println!("reason:            {reason}");
            }
        }
        None => {
            eprintln!(
                "unknown provider '{name}'. available: {}",
                available_providers().join(", "),
            );
            std::process::exit(1);
        }
    }
}
