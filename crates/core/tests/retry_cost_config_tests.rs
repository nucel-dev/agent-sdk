//! Hardening tests for the cross-cutting reliability primitives that every
//! network provider leans on: transient-error classification, the
//! [`RetryPolicy`] backoff/budget contract, overflow-safe [`AgentCost`]
//! accounting, and the construction-time defaults of [`SpawnConfig`] /
//! [`ExecutorConfig`].
//!
//! These complement the in-module unit tests in `core/src/retry.rs` and the
//! type tests in `core/tests/types_tests.rs`; they deliberately probe the
//! *interactions* (budget × classification, repeated `+=` accumulation,
//! default precedence) rather than re-asserting single-call behavior.

use std::time::Duration;

use nucel_agent_core::retry::is_transient;
use nucel_agent_core::{
    AgentCost, AgentError, ExecutorConfig, RetryPolicy, SpawnConfig,
};

// ── Error classification: transient vs fatal ────────────────────────────────

/// The whole retry machinery hinges on this mapping, so pin every arm of the
/// taxonomy that providers can surface. A drift here (e.g. accidentally making
/// `Provider` transient) would silently double-fire side effects.
#[test]
fn classification_matrix_covers_the_taxonomy() {
    // Transient: upstream rejected/never-serviced the request.
    let transient: Vec<AgentError> = vec![
        AgentError::RateLimited {
            message: "slow down".into(),
        },
        AgentError::Timeout { seconds: 30 },
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionRefused)),
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::TimedOut)),
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
    ];
    for err in &transient {
        assert!(is_transient(err), "expected transient: {err:?}");
    }

    // Fatal: may reflect a side effect, or will fail identically on retry.
    let fatal: Vec<AgentError> = vec![
        AgentError::Provider {
            provider: "vertex".into(),
            message: "500 partial output".into(),
        },
        AgentError::Config("missing key".into()),
        AgentError::BudgetExceeded {
            limit: 1.0,
            spent: 2.0,
        },
        AgentError::SessionNotFound {
            session_id: "s1".into(),
        },
        AgentError::CliNotFound {
            cli_name: "codex".into(),
        },
        AgentError::EscalationRequested,
        AgentError::StreamInterrupted("truncated".into()),
        AgentError::HookFailed {
            hook: "pre".into(),
            message: "boom".into(),
        },
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::NotFound)),
        AgentError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
    ];
    for err in &fatal {
        assert!(!is_transient(err), "expected fatal: {err:?}");
    }
}

// ── RetryPolicy: budget × classification interaction ────────────────────────

/// `should_retry` is the AND of two conditions: budget remaining AND the error
/// being transient. Exercise the full truth table so neither condition can be
/// dropped without a failing test.
#[test]
fn should_retry_is_budget_and_transient() {
    let policy = RetryPolicy::with_max_retries(2);
    let transient = AgentError::RateLimited {
        message: "x".into(),
    };
    let fatal = AgentError::Config("nope".into());

    // transient + budget left → retry
    assert!(policy.should_retry(&transient, 0));
    assert!(policy.should_retry(&transient, 1));
    // transient + budget exhausted → stop
    assert!(!policy.should_retry(&transient, 2));
    assert!(!policy.should_retry(&transient, 99));
    // fatal, regardless of budget → never retry
    assert!(!policy.should_retry(&fatal, 0));
    assert!(!policy.should_retry(&fatal, 1));
}

/// A zero-retry policy must refuse even the first retry of a transient error —
/// this is the contract callers rely on when they are past a side-effect
/// boundary and pass `RetryPolicy::none()`.
#[test]
fn none_policy_refuses_transient_immediately() {
    let policy = RetryPolicy::none();
    let transient = AgentError::Timeout { seconds: 5 };
    assert!(!policy.should_retry(&transient, 0));
    assert_eq!(policy.max_retries, 0);
}

/// Backoff must be (a) monotonically non-decreasing, (b) never exceed the cap,
/// and (c) never panic for an absurd attempt index. The closed-form values are
/// asserted in the unit tests; here we assert the *invariants* hold across a
/// wide sweep, which guards against a future refactor of the growth function.
#[test]
fn backoff_invariants_hold_across_a_sweep() {
    let policy = RetryPolicy {
        max_retries: 100,
        base_backoff: Duration::from_millis(200),
        max_backoff: Duration::from_secs(10),
    };

    let mut prev = Duration::ZERO;
    for attempt in 0..64u32 {
        let b = policy.backoff_for(attempt);
        assert!(
            b >= prev,
            "backoff must be non-decreasing: attempt {attempt} gave {b:?} < {prev:?}"
        );
        assert!(
            b <= policy.max_backoff,
            "backoff must never exceed cap: attempt {attempt} gave {b:?}"
        );
        prev = b;
    }
    // First step equals the base; cap is reached and held at the tail.
    assert_eq!(policy.backoff_for(0), Duration::from_millis(200));
    assert_eq!(policy.backoff_for(63), policy.max_backoff);
    // u32::MAX attempt index must not panic or wrap below the cap.
    assert_eq!(policy.backoff_for(u32::MAX), policy.max_backoff);
}

/// A degenerate cap *below* the base backoff must clamp the very first retry —
/// the cap always wins, even on attempt 0.
#[test]
fn cap_below_base_clamps_first_retry() {
    let policy = RetryPolicy {
        max_retries: 3,
        base_backoff: Duration::from_secs(5),
        max_backoff: Duration::from_millis(100),
    };
    assert_eq!(policy.backoff_for(0), Duration::from_millis(100));
    assert_eq!(policy.backoff_for(3), Duration::from_millis(100));
}

// ── AgentCost: overflow-safe accumulation ───────────────────────────────────

/// Accumulating many per-turn costs via `+=` must never overflow-panic, even
/// when individual turns are huge. This mirrors how a long-running session
/// folds each turn's cost into a running total.
#[test]
fn repeated_add_assign_saturates_without_panic() {
    let mut acc = AgentCost::default();
    let big_turn = AgentCost {
        input_tokens: u64::MAX / 4,
        output_tokens: u64::MAX / 4,
        cache_read_tokens: u64::MAX / 4,
        cache_creation_tokens: u64::MAX / 4,
        total_usd: 1_000.0,
    };

    // 10 folds of a quarter-max turn would overflow plain `u64` addition
    // (debug-panic / release-wrap). Saturating add must pin at u64::MAX.
    for _ in 0..10 {
        acc += big_turn.clone();
    }

    assert_eq!(acc.input_tokens, u64::MAX);
    assert_eq!(acc.output_tokens, u64::MAX);
    assert_eq!(acc.cache_read_tokens, u64::MAX);
    assert_eq!(acc.cache_creation_tokens, u64::MAX);
    // USD is an f64 sum and simply grows; it must remain finite and positive.
    assert!(acc.total_usd.is_finite());
    assert!(acc.total_usd >= 10_000.0);
}

/// Token accounting is commutative and associative for the saturating-add
/// channels (order of folding turns must not change the running total).
#[test]
fn token_accumulation_is_order_independent() {
    let a = AgentCost {
        input_tokens: 100,
        output_tokens: 40,
        cache_read_tokens: 7,
        cache_creation_tokens: 3,
        total_usd: 0.01,
    };
    let b = AgentCost {
        input_tokens: 250,
        output_tokens: 60,
        cache_read_tokens: 11,
        cache_creation_tokens: 5,
        total_usd: 0.02,
    };
    let c = AgentCost {
        input_tokens: 9,
        output_tokens: 1,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        total_usd: 0.005,
    };

    let left = (a.clone() + b.clone()) + c.clone();
    let right = a + (b + c);
    assert_eq!(left.input_tokens, right.input_tokens);
    assert_eq!(left.output_tokens, right.output_tokens);
    assert_eq!(left.cache_read_tokens, right.cache_read_tokens);
    assert_eq!(left.cache_creation_tokens, right.cache_creation_tokens);
    assert_eq!(left.input_tokens, 359);
    assert_eq!(left.output_tokens, 101);
}

/// A single huge turn just below the boundary plus a small turn that crosses it
/// must clamp exactly at `u64::MAX`, not wrap to a tiny number.
#[test]
fn near_boundary_single_add_clamps_exactly() {
    let near = AgentCost {
        input_tokens: u64::MAX - 5,
        output_tokens: u64::MAX - 5,
        cache_read_tokens: u64::MAX - 5,
        cache_creation_tokens: u64::MAX - 5,
        total_usd: 0.0,
    };
    let over = AgentCost {
        input_tokens: 10,
        output_tokens: 10,
        cache_read_tokens: 10,
        cache_creation_tokens: 10,
        total_usd: 0.0,
    };
    let sum = near + over;
    assert_eq!(sum.input_tokens, u64::MAX);
    assert_eq!(sum.output_tokens, u64::MAX);
    assert_eq!(sum.cache_read_tokens, u64::MAX);
    assert_eq!(sum.cache_creation_tokens, u64::MAX);
}

// ── Config construction defaults & precedence ───────────────────────────────

/// `ExecutorConfig::default()` must default its retry policy to the standard
/// retrying policy (not `none()`), so providers built from a bare default are
/// resilient out of the box.
#[test]
fn executor_config_default_uses_default_retry_policy() {
    let cfg = ExecutorConfig::default();
    assert!(cfg.api_key.is_none());
    assert!(cfg.base_url.is_none());
    assert!(cfg.working_dir.is_none());
    assert_eq!(cfg.retry_policy, RetryPolicy::default());
    // The default is the *retrying* policy, not the no-op one.
    assert_ne!(cfg.retry_policy, RetryPolicy::none());
    assert_eq!(cfg.retry_policy.max_retries, 3);
}

/// `SpawnConfig::default()` must likewise carry the default retry policy, which
/// is the sentinel the network providers use to decide "fall back to the
/// executor-level policy". If this drifted away from `RetryPolicy::default()`,
/// the per-spawn override detection (`config == default ? executor : config`)
/// would break.
#[test]
fn spawn_config_default_retry_policy_is_the_override_sentinel() {
    let cfg = SpawnConfig::default();
    assert_eq!(cfg.retry_policy, RetryPolicy::default());
    assert!(cfg.model.is_none());
    assert!(cfg.budget_usd.is_none());
    assert!(cfg.cache_breakpoints.is_empty());
    assert!(cfg.thinking_budget.is_none());
}

/// `..Default::default()` struct-update must leave `retry_policy` at the
/// default sentinel when the caller doesn't mention it — the additive-field
/// contract that keeps existing construction sites source-compatible.
#[test]
fn spawn_config_struct_update_preserves_default_retry_policy() {
    let cfg = SpawnConfig {
        model: Some("claude-sonnet-4@20251015".into()),
        budget_usd: Some(2.5),
        ..Default::default()
    };
    assert_eq!(cfg.retry_policy, RetryPolicy::default());

    // And an explicit override is faithfully retained.
    let overridden = SpawnConfig {
        retry_policy: RetryPolicy::none(),
        ..Default::default()
    };
    assert_eq!(overridden.retry_policy, RetryPolicy::none());
    assert_ne!(overridden.retry_policy, RetryPolicy::default());
}

/// Cloning a config must deep-copy the retry policy (it is `Copy`, but the
/// surrounding struct is `Clone`) — a clone used for a retry attempt must not
/// observe a mutated policy.
#[test]
fn config_clone_preserves_custom_retry_policy() {
    let custom = RetryPolicy {
        max_retries: 7,
        base_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_secs(2),
    };
    let cfg = SpawnConfig {
        retry_policy: custom,
        ..Default::default()
    };
    let cloned = cfg.clone();
    assert_eq!(cloned.retry_policy, custom);
    assert_eq!(cloned.retry_policy.max_retries, 7);
}
