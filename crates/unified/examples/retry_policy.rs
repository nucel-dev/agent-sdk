//! retry_policy — inspect how the SDK classifies transient failures and how
//! the default backoff curve behaves. Provider-agnostic; needs no CLI, no
//! network, no credentials.
//!
//! Network providers (Vertex today) retry transient, *pre-side-effect* failures
//! automatically using [`RetryPolicy`]. This example shows what that policy
//! decides so you can reason about (or override) it in your own integration.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nucel-agent-sdk --example retry_policy
//! ```

use nucel_agent_sdk::{AgentError, RetryPolicy, is_transient};

fn main() {
    // The default policy: 3 retries, 250 ms base, exponential, capped at 8 s.
    let policy = RetryPolicy::default();
    println!("default policy = {policy:?}\n");

    println!("backoff curve (deterministic, no jitter):");
    for attempt in 0..6 {
        println!("  retry #{attempt}: wait {:?}", policy.backoff_for(attempt));
    }
    println!();

    // Classification: which errors are safe to retry before any side effect?
    let cases: Vec<(&str, AgentError)> = vec![
        (
            "rate limited (429)",
            AgentError::RateLimited {
                message: "slow down".into(),
            },
        ),
        ("request timeout", AgentError::Timeout { seconds: 300 }),
        (
            "connection reset",
            AgentError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
        ),
        (
            "provider error (may have side effects)",
            AgentError::Provider {
                provider: "vertex".into(),
                message: "500".into(),
            },
        ),
        (
            "budget exceeded",
            AgentError::BudgetExceeded {
                limit: 1.0,
                spent: 2.0,
            },
        ),
        ("config error", AgentError::Config("bad endpoint".into())),
    ];

    println!("transient classification:");
    for (label, err) in &cases {
        let transient = is_transient(err);
        let decision = policy.should_retry(err, 0);
        println!("  {label:<42} transient={transient:<5} would_retry={decision}");
    }

    println!(
        "\nNote: a `Provider` error is treated as FATAL on purpose — once the \
         model has started\nproducing output, replaying the whole turn would \
         double-charge and duplicate side effects."
    );
}
