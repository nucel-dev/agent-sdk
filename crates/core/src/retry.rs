//! Transient-error classification and backoff policy.
//!
//! Providers that talk to a network endpoint (Vertex, OpenCode, Bedrock)
//! occasionally hit *transient* failures — a dropped connection, a `503`
//! while the upstream scales, a `429` rate-limit before the request was ever
//! serviced. Retrying those is safe and improves robustness.
//!
//! # The side-effect rule (read this)
//!
//! A retry is only safe **before any side effect has occurred**. Once a
//! request has been accepted and the model has started producing output —
//! tokens streamed, cost incurred, a tool invoked, a file written — replaying
//! the whole operation would double the side effect. In that situation the
//! error must surface as *fatal*, never retried.
//!
//! Concretely, in this SDK the boundary is: **retry only the request-dispatch
//! phase**, i.e. the failure happened while establishing the connection or
//! the server rejected the request outright (`429`/`503`) *before any response
//! body was consumed*. The moment you begin reading a `2xx` body, you are past
//! the boundary — surface errors from there as fatal.
//!
//! [`RetryPolicy`] only decides *whether* and *how long* to wait. It is the
//! caller's job to place the retry loop strictly inside the pre-side-effect
//! window. See `crates/vertex/src/lib.rs` for the canonical wiring.
//!
//! # Example
//!
//! ```
//! use nucel_agent_core::retry::RetryPolicy;
//! use std::time::Duration;
//!
//! let policy = RetryPolicy::default();
//! // First retry waits ~base, each subsequent one doubles up to `max_backoff`.
//! assert_eq!(policy.backoff_for(0), Duration::from_millis(250));
//! assert_eq!(policy.backoff_for(1), Duration::from_millis(500));
//! assert_eq!(policy.backoff_for(2), Duration::from_millis(1000));
//! ```

use std::time::Duration;

use crate::error::AgentError;

/// Whether a failed operation may be safely retried, given a classification of
/// the error and how many attempts have already been made.
///
/// Pure, deterministic, and cheap to construct — designed to be unit-tested in
/// isolation and shared across every network provider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// Maximum number of *retries* (i.e. attempts after the first). `0`
    /// disables retrying entirely.
    pub max_retries: u32,
    /// Backoff applied before the first retry. Doubles each subsequent retry.
    pub base_backoff: Duration,
    /// Upper bound on a single backoff interval, so exponential growth can't
    /// run away on the later attempts.
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    /// A conservative default: up to 3 retries, 250 ms base, capped at 8 s.
    ///
    /// These numbers are deliberately modest — coding-agent calls are long and
    /// expensive, so we want to ride out a blip, not hammer a struggling
    /// endpoint.
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(8),
        }
    }
}

impl RetryPolicy {
    /// A policy that never retries. Useful when the caller is already past a
    /// side-effect boundary, or for tests.
    pub const fn none() -> Self {
        Self {
            max_retries: 0,
            base_backoff: Duration::from_millis(0),
            max_backoff: Duration::from_millis(0),
        }
    }

    /// Build a policy with a custom retry count, keeping the default backoff
    /// curve.
    pub fn with_max_retries(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Self::default()
        }
    }

    /// Should we retry, given the error and the number of retries already
    /// performed (`retries_done` is 0 on the first failure)?
    ///
    /// Returns `false` once the budget is exhausted **or** the error is not
    /// classified as transient by [`is_transient`].
    pub fn should_retry(&self, error: &AgentError, retries_done: u32) -> bool {
        retries_done < self.max_retries && is_transient(error)
    }

    /// Backoff duration before the retry indexed by `retries_done`
    /// (0 = first retry). Exponential: `base * 2^retries_done`, clamped to
    /// `max_backoff`. Deterministic — no jitter — so it can be asserted on.
    pub fn backoff_for(&self, retries_done: u32) -> Duration {
        // Saturating shift so a large `retries_done` can't panic/overflow.
        let factor = 1u64.checked_shl(retries_done).unwrap_or(u64::MAX);
        let millis = (self.base_backoff.as_millis() as u64).saturating_mul(factor);
        let capped = millis.min(self.max_backoff.as_millis() as u64);
        Duration::from_millis(capped)
    }
}

/// Classify an [`AgentError`] as a *transient* failure that is safe to retry
/// **in the pre-side-effect window**.
///
/// Transient:
/// - [`AgentError::RateLimited`] — upstream throttled us before doing work.
/// - [`AgentError::Timeout`] — the request never got a response.
/// - [`AgentError::Io`] — a connection-level error (reset, refused, DNS).
///
/// Everything else is treated as fatal. In particular
/// [`AgentError::Provider`], [`AgentError::BudgetExceeded`],
/// [`AgentError::Config`], and JSON-decode errors are **not** retried: a
/// provider error may already reflect partially-applied work, and config/JSON
/// errors will fail identically on every attempt.
pub fn is_transient(error: &AgentError) -> bool {
    match error {
        AgentError::RateLimited { .. } => true,
        AgentError::Timeout { .. } => true,
        AgentError::Io(e) => is_transient_io(e.kind()),
        // Conservative: never retry a generic provider error (it may have had
        // side effects), nor config/budget/JSON/etc.
        _ => false,
    }
}

/// Whether a `std::io::ErrorKind` represents a transient connection failure.
fn is_transient_io(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        kind,
        ConnectionReset
            | ConnectionAborted
            | ConnectionRefused
            | NotConnected
            | BrokenPipe
            | TimedOut
            | Interrupted
            | WouldBlock
            | UnexpectedEof
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn default_policy_is_conservative() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 3);
        assert_eq!(p.base_backoff, Duration::from_millis(250));
        assert_eq!(p.max_backoff, Duration::from_secs(8));
    }

    #[test]
    fn none_policy_never_retries() {
        let p = RetryPolicy::none();
        let err = AgentError::RateLimited { message: "slow down".into() };
        assert!(!p.should_retry(&err, 0));
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let p = RetryPolicy::default();
        assert_eq!(p.backoff_for(0), Duration::from_millis(250));
        assert_eq!(p.backoff_for(1), Duration::from_millis(500));
        assert_eq!(p.backoff_for(2), Duration::from_millis(1000));
        assert_eq!(p.backoff_for(3), Duration::from_millis(2000));
        // Eventually clamps at max_backoff (8s).
        assert_eq!(p.backoff_for(10), Duration::from_secs(8));
        // Absurd attempt count must not overflow/panic.
        assert_eq!(p.backoff_for(255), Duration::from_secs(8));
    }

    #[test]
    fn rate_limit_is_transient() {
        assert!(is_transient(&AgentError::RateLimited { message: "x".into() }));
    }

    #[test]
    fn timeout_is_transient() {
        assert!(is_transient(&AgentError::Timeout { seconds: 30 }));
    }

    #[test]
    fn connection_reset_is_transient() {
        let err = AgentError::Io(io::Error::from(io::ErrorKind::ConnectionReset));
        assert!(is_transient(&err));
    }

    #[test]
    fn provider_error_is_fatal() {
        // Provider errors may reflect side effects — never retried.
        let err = AgentError::Provider {
            provider: "vertex".into(),
            message: "500: model produced partial output".into(),
        };
        assert!(!is_transient(&err));
    }

    #[test]
    fn config_and_budget_errors_are_fatal() {
        assert!(!is_transient(&AgentError::Config("bad".into())));
        assert!(!is_transient(&AgentError::BudgetExceeded { limit: 1.0, spent: 2.0 }));
    }

    #[test]
    fn not_found_io_kind_is_fatal() {
        // A "file not found" style io error won't fix itself on retry.
        let err = AgentError::Io(io::Error::from(io::ErrorKind::NotFound));
        assert!(!is_transient(&err));
    }

    #[test]
    fn should_retry_respects_budget() {
        let p = RetryPolicy::with_max_retries(2);
        let err = AgentError::RateLimited { message: "x".into() };
        assert!(p.should_retry(&err, 0));
        assert!(p.should_retry(&err, 1));
        // Budget exhausted on the third failure.
        assert!(!p.should_retry(&err, 2));
    }

    #[test]
    fn should_retry_false_for_fatal_even_with_budget() {
        let p = RetryPolicy::with_max_retries(5);
        let err = AgentError::Config("nope".into());
        assert!(!p.should_retry(&err, 0));
    }

    #[test]
    fn with_max_retries_keeps_default_backoff_curve() {
        // Only the retry count changes; the backoff curve stays the default.
        let p = RetryPolicy::with_max_retries(7);
        assert_eq!(p.max_retries, 7);
        assert_eq!(p.base_backoff, RetryPolicy::default().base_backoff);
        assert_eq!(p.max_backoff, RetryPolicy::default().max_backoff);
        assert_eq!(p.backoff_for(0), Duration::from_millis(250));
        assert_eq!(p.backoff_for(1), Duration::from_millis(500));
    }

    #[test]
    fn none_policy_backoff_is_zero() {
        // A non-retrying policy still answers backoff queries safely (0).
        let p = RetryPolicy::none();
        assert_eq!(p.backoff_for(0), Duration::from_millis(0));
        assert_eq!(p.backoff_for(5), Duration::from_millis(0));
    }

    #[test]
    fn broken_pipe_and_unexpected_eof_are_transient() {
        // Connection-level drops mid-handshake are safe to retry.
        for kind in [
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionRefused,
        ] {
            let err = AgentError::Io(io::Error::from(kind));
            assert!(is_transient(&err), "{kind:?} should be transient");
        }
    }

    #[test]
    fn permission_denied_io_is_fatal() {
        // A permissions error won't fix itself on retry.
        let err = AgentError::Io(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(!is_transient(&err));
    }
}
