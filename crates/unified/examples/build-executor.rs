//! Pick an executor at runtime by name.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example build-executor -- claude-code
//! cargo run -p nucel-agent-sdk --example build-executor -- codex
//! cargo run -p nucel-agent-sdk --example build-executor -- opencode http://127.0.0.1:4096
//! ```
//!
//! This is the same pattern higher-level orchestrators (e.g. `agent-operator`)
//! use to swap providers via a config string.

use nucel_agent_sdk::{available_providers, build_executor};

fn main() {
    let mut args = std::env::args().skip(1);
    let name = match args.next() {
        Some(n) => n,
        None => {
            eprintln!(
                "usage: build-executor <provider> [base_url]\n\navailable: {}",
                available_providers().join(", ")
            );
            std::process::exit(2);
        }
    };
    let base_url = args.next();

    match build_executor(&name, base_url) {
        Some(exec) => {
            let caps = exec.capabilities();
            let avail = exec.availability();
            println!("provider:           {}", exec.executor_type());
            println!("session_resume:     {}", caps.session_resume);
            println!("token_usage:        {}", caps.token_usage);
            println!("mcp_support:        {}", caps.mcp_support);
            println!("autonomous_mode:    {}", caps.autonomous_mode);
            println!("structured_output:  {}", caps.structured_output);
            println!("available:          {}", avail.available);
            if let Some(reason) = avail.reason {
                println!("reason:             {reason}");
            }
        }
        None => {
            eprintln!(
                "unknown provider '{name}'. available: {}",
                available_providers().join(", ")
            );
            std::process::exit(1);
        }
    }
}
