//! Cross-cutting tests for the umbrella crate.
//!
//! These exercise behaviour that has to stay *consistent across providers*
//! rather than any single adapter's wire format:
//!
//! 1. **Provider selection** — `build_executor` / `available_providers` map
//!    config strings to the right executor (incl. feature-gated arms).
//! 2. **Cost accumulation** — folding [`AgentCost`] across many turns / many
//!    providers must be associative, saturating, and cache-token aware.
//! 3. **Retry classification** — the shared [`is_transient`] / [`RetryPolicy`]
//!    must classify a given error identically no matter which provider raised
//!    it, because all network providers funnel through the same classifier.
//!
//! Nothing here touches the network or a real CLI: cost math is pure, and the
//! retry classifier is a pure function over [`AgentError`].

use nucel_agent_sdk::{
    available_providers, build_executor, is_transient, AgentCost, AgentError, ExecutorType,
    RetryPolicy,
};

// ── 1. Provider selection ────────────────────────────────────────────────

#[test]
fn build_executor_maps_each_base_provider_to_its_type() {
    let cases = [
        ("claude-code", ExecutorType::ClaudeCode),
        ("codex", ExecutorType::Codex),
        ("opencode", ExecutorType::OpenCode),
    ];
    for (name, want) in cases {
        let exec = build_executor(name, None)
            .unwrap_or_else(|| panic!("build_executor({name}) returned None"));
        assert_eq!(exec.executor_type(), want, "provider {name}");
    }
}

#[test]
fn available_providers_is_in_sync_with_build_executor() {
    // Every name advertised by `available_providers()` must actually build
    // (the locally-constructible ones, anyway — bedrock/vertex need cloud
    // creds and are validated in their own crates). A drift here means the
    // advertised list lies to callers.
    for provider in available_providers() {
        if *provider == "bedrock" || *provider == "vertex" {
            continue;
        }
        assert!(
            build_executor(provider, None).is_some(),
            "advertised provider {provider} failed to build",
        );
    }
}

#[test]
fn unknown_and_malformed_provider_strings_return_none() {
    for bad in [
        "",
        " ",
        "gpt-4",
        "claude code",
        "CLAUDE-CODE",
        "bedrock-runtime",
    ] {
        assert!(
            build_executor(bad, None).is_none(),
            "{bad:?} should not resolve to a provider",
        );
    }
}

// ── 2. Cost accumulation across providers ────────────────────────────────

/// A run that hands a prompt across several providers folds each turn's cost
/// into one running total. The `+` operator must be associative so the order
/// of folding never changes the answer.
#[test]
fn cost_fold_is_associative_across_provider_turns() {
    let claude = AgentCost {
        input_tokens: 1_000,
        output_tokens: 200,
        cache_read_tokens: 800,
        cache_creation_tokens: 100,
        total_usd: 0.05,
    };
    let codex = AgentCost {
        input_tokens: 2_000,
        output_tokens: 500,
        total_usd: 0.03,
        ..Default::default()
    };
    let opencode = AgentCost {
        input_tokens: 1_500,
        output_tokens: 300,
        total_usd: 0.02,
        ..Default::default()
    };

    let left = (claude.clone() + codex.clone()) + opencode.clone();
    let right = claude.clone() + (codex.clone() + opencode.clone());

    assert_eq!(left.input_tokens, right.input_tokens);
    assert_eq!(left.output_tokens, right.output_tokens);
    assert_eq!(left.cache_read_tokens, right.cache_read_tokens);
    assert_eq!(left.cache_creation_tokens, right.cache_creation_tokens);
    assert!((left.total_usd - right.total_usd).abs() < 1e-12);

    // And the totals are what we'd expect by hand.
    assert_eq!(left.input_tokens, 4_500);
    assert_eq!(left.output_tokens, 1_000);
    assert_eq!(left.cache_read_tokens, 800);
    assert_eq!(left.cache_creation_tokens, 100);
    assert!((left.total_usd - 0.10).abs() < 1e-12);
}

/// `AddAssign` is the idiom every session uses to roll each turn into a running
/// total; it must match `Add` exactly.
#[test]
fn cost_add_assign_matches_add() {
    let a = AgentCost {
        input_tokens: 10,
        output_tokens: 5,
        total_usd: 0.01,
        ..Default::default()
    };
    let b = AgentCost {
        input_tokens: 20,
        output_tokens: 7,
        total_usd: 0.02,
        ..Default::default()
    };

    let folded = a.clone() + b.clone();

    let mut acc = AgentCost::default();
    acc += a;
    acc += b;

    assert_eq!(acc.input_tokens, folded.input_tokens);
    assert_eq!(acc.output_tokens, folded.output_tokens);
    assert!((acc.total_usd - folded.total_usd).abs() < 1e-12);
}

/// Token accumulation saturates instead of overflowing — a very long session
/// (or a buggy provider returning absurd counts) must never panic in a debug
/// build. USD is an `f64` so it grows past any realistic bound without wrapping.
#[test]
fn cost_token_accumulation_saturates() {
    let huge = AgentCost {
        input_tokens: u64::MAX,
        output_tokens: u64::MAX,
        cache_read_tokens: u64::MAX,
        cache_creation_tokens: u64::MAX,
        total_usd: 1.0,
    };
    let one = AgentCost {
        input_tokens: 1,
        output_tokens: 1,
        total_usd: 1.0,
        ..Default::default()
    };

    let sum = huge + one;
    assert_eq!(sum.input_tokens, u64::MAX, "must saturate, not wrap to 0");
    assert_eq!(sum.output_tokens, u64::MAX);
    assert_eq!(sum.cache_read_tokens, u64::MAX);
    assert_eq!(sum.cache_creation_tokens, u64::MAX);
    assert!((sum.total_usd - 2.0).abs() < 1e-12);
}

/// Folding a multi-provider run via `Iterator::fold` (the pattern in
/// `multi_provider_handoff`) sums every dimension including cache tokens.
#[test]
fn cost_fold_over_iterator_sums_all_dimensions() {
    let runs = vec![
        AgentCost {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            total_usd: 0.01,
            ..Default::default()
        },
        AgentCost {
            input_tokens: 200,
            output_tokens: 80,
            cache_read_tokens: 20,
            total_usd: 0.02,
            ..Default::default()
        },
        AgentCost {
            input_tokens: 300,
            output_tokens: 90,
            cache_read_tokens: 30,
            total_usd: 0.03,
            ..Default::default()
        },
    ];

    let total = runs
        .into_iter()
        .fold(AgentCost::default(), |acc, c| acc + c);
    assert_eq!(total.input_tokens, 600);
    assert_eq!(total.output_tokens, 220);
    assert_eq!(total.cache_read_tokens, 60);
    assert!((total.total_usd - 0.06).abs() < 1e-12);
}

// ── 3. Retry classification consistency ──────────────────────────────────

/// Errors that any network provider may raise (Bedrock throttling, Vertex 503,
/// OpenCode connection reset, a request timeout) must all classify as
/// *transient* — regardless of which provider produced them. This is the
/// contract that lets `build_executor` swap providers without changing retry
/// behaviour.
#[test]
fn transient_classification_is_provider_agnostic() {
    let transient: Vec<AgentError> = vec![
        // Bedrock ThrottlingException → RateLimited
        AgentError::RateLimited {
            message: "Bedrock throttled".into(),
        },
        // Vertex 503 → RateLimited
        AgentError::RateLimited {
            message: "vertex 503: model overloaded".into(),
        },
        // Request never got a response.
        AgentError::Timeout { seconds: 300 },
        // OpenCode/Vertex dropped connection mid-handshake.
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
    ];
    for err in &transient {
        assert!(is_transient(err), "expected transient: {err:?}");
    }
}

/// Conversely, errors that may reflect *partially-applied work* or a permanent
/// condition must classify as fatal on every provider — replaying them would
/// double-charge or fail identically.
#[test]
fn fatal_classification_is_provider_agnostic() {
    let fatal: Vec<AgentError> = vec![
        // A generic provider error may have already produced output.
        AgentError::Provider {
            provider: "bedrock".into(),
            message: "validation".into(),
        },
        AgentError::Provider {
            provider: "vertex".into(),
            message: "500 mid-stream".into(),
        },
        AgentError::Provider {
            provider: "opencode".into(),
            message: "500".into(),
        },
        AgentError::BudgetExceeded {
            limit: 1.0,
            spent: 2.0,
        },
        AgentError::Config("bad endpoint".into()),
        AgentError::CliNotFound {
            cli_name: "claude".into(),
        },
        AgentError::SessionNotFound {
            session_id: "x".into(),
        },
        // File-not-found / permission io kinds won't fix themselves on retry.
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::NotFound)),
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
    ];
    for err in &fatal {
        assert!(!is_transient(err), "expected fatal: {err:?}");
    }
}

/// The default policy must make the *same* retry decision for the same error
/// class regardless of provider, and stop once the budget is exhausted.
#[test]
fn retry_policy_decision_is_stable_per_error_class() {
    let policy = RetryPolicy::default();

    // A transient error is retried up to the budget, then stops.
    let throttle = AgentError::RateLimited {
        message: "slow down".into(),
    };
    assert!(policy.should_retry(&throttle, 0));
    assert!(policy.should_retry(&throttle, policy.max_retries - 1));
    assert!(!policy.should_retry(&throttle, policy.max_retries));

    // A fatal error is never retried, even with budget remaining.
    let provider_err = AgentError::Provider {
        provider: "any".into(),
        message: "x".into(),
    };
    assert!(!policy.should_retry(&provider_err, 0));
}

/// `RetryPolicy::none()` disables retrying for *every* provider uniformly — the
/// escape hatch a caller uses once it knows it's past a side-effect boundary.
#[test]
fn none_policy_disables_retry_for_all_error_classes() {
    let policy = RetryPolicy::none();
    let errs = [
        AgentError::RateLimited {
            message: "x".into(),
        },
        AgentError::Timeout { seconds: 1 },
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
    ];
    for err in &errs {
        assert!(
            !policy.should_retry(err, 0),
            "none() must not retry {err:?}"
        );
    }
    assert_eq!(policy.backoff_for(0), std::time::Duration::from_millis(0));
}

/// The backoff curve is identical no matter which provider drives it (it's a
/// pure function of the policy + attempt index). Lock the default curve so a
/// regression in one provider's wiring can't silently change timing.
#[test]
fn default_backoff_curve_is_deterministic() {
    let p = RetryPolicy::default();
    use std::time::Duration;
    assert_eq!(p.backoff_for(0), Duration::from_millis(250));
    assert_eq!(p.backoff_for(1), Duration::from_millis(500));
    assert_eq!(p.backoff_for(2), Duration::from_millis(1000));
    assert_eq!(p.backoff_for(3), Duration::from_millis(2000));
    // Clamps at max_backoff and never overflows on absurd attempt counts.
    assert_eq!(p.backoff_for(100), Duration::from_secs(8));
    assert_eq!(p.backoff_for(u32::MAX), Duration::from_secs(8));
}
