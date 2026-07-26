//! OpenCode provider — HTTP client to an `opencode serve` instance.
//!
//! OpenCode runs as a server (`opencode serve` on `:4096` by default). This
//! provider talks to it via HTTP REST. The client is stateless — sessions
//! live on the server.
//!
//! Supports:
//! - Session creation and prompting
//! - Multi-turn conversations
//! - Session resume (native — returns the same OpenCode session id)
//! - Basic-auth credentials (`api_key` → HTTP basic password)
//!
//! # Minimal example
//!
//! Start a local server in another shell:
//!
//! ```bash
//! opencode serve --port 4096
//! ```
//!
//! Then:
//!
//! ```rust,no_run
//! use nucel_agent_opencode::OpencodeExecutor;
//! use nucel_agent_core::{AgentExecutor, SpawnConfig};
//! use std::path::Path;
//!
//! # async fn run() -> nucel_agent_core::Result<()> {
//! let executor = OpencodeExecutor::with_base_url("http://127.0.0.1:4096");
//! let session = executor.spawn(
//!     Path::new("/my/repo"),
//!     "Read the README and summarize this project.",
//!     &SpawnConfig::default(),
//! ).await?;
//!
//! println!("{}", session.query("Any TODOs?").await?.content);
//! session.close().await?;
//! # Ok(()) }
//! ```
//!
//! See also: [workspace README](https://github.com/nucel-dev/agent-sdk#readme)
//! and the runnable example `crates/unified/examples/opencode_http.rs`.

#![cfg_attr(docsrs, feature(doc_cfg))]

mod client;
mod protocol;

use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse, AgentSession,
    AvailabilityStatus, EventStream, ExecutorType, Result, RetryPolicy, SessionImpl, SpawnConfig,
};

use client::OpencodeClient;

/// How long [`OpencodeExecutor::availability`] waits for the TCP connect
/// before declaring the server unreachable.
///
/// Short enough that callers can poll availability on a heartbeat without
/// paying a noticeable stall, long enough for a healthy local or in-cluster
/// server to answer. Matches the probe budget `nucel-server` used while it
/// had to work around this method not probing at all.
const AVAILABILITY_PROBE_TIMEOUT: Duration = Duration::from_millis(750);

/// Extract `host:port` from an OpenCode base URL such as
/// `http://127.0.0.1:4096`.
///
/// When the URL omits an explicit port, default to 443 for `https://` and 80
/// otherwise. Returns `None` when there is no authority to connect to.
fn host_port(base_url: &str) -> Option<String> {
    let trimmed = base_url.trim();
    let is_https = trimmed.starts_with("https://");
    // Strip the scheme first (before touching slashes), then keep only the
    // authority, dropping any trailing path/query/fragment.
    let rest = trimmed
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if authority.is_empty() {
        return None;
    }
    if authority.contains(':') {
        Some(authority.to_string())
    } else {
        let port = if is_https { 443 } else { 80 };
        Some(format!("{authority}:{port}"))
    }
}

/// TCP connectivity check with a hard per-address timeout. `true` only when a
/// connection is established; any resolution failure, refusal, or timeout is
/// `false`.
///
/// Blocking on purpose: [`AgentExecutor::availability`] is a synchronous trait
/// method (the subprocess providers shell out to `which` from it), so this
/// cannot await. The cost is bounded by [`AVAILABILITY_PROBE_TIMEOUT`] per
/// resolved address.
fn tcp_reachable(addr: &str, timeout: Duration) -> bool {
    let Ok(resolved) = addr.to_socket_addrs() else {
        return false;
    };
    resolved.into_iter().any(|sock| {
        TcpStream::connect_timeout(&sock, timeout)
            .map(|stream| {
                // Close immediately — this is a liveness probe, not a session.
                drop(stream);
            })
            .is_ok()
    })
}

/// OpenCode executor — connects to OpenCode HTTP server.
pub struct OpencodeExecutor {
    base_url: String,
    api_key: Option<String>,
    /// Retry policy for *transient*, pre-side-effect request failures, applied
    /// to every client this executor builds. Defaults to
    /// [`RetryPolicy::default`].
    retry: RetryPolicy,
}

impl OpencodeExecutor {
    pub fn new() -> Self {
        Self {
            base_url: "http://127.0.0.1:4096".to_string(),
            api_key: None,
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: None,
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Override the retry policy for transient, pre-side-effect failures.
    ///
    /// Pass [`RetryPolicy::none`] to disable retrying. Retries only ever apply
    /// to the request-dispatch phase (connection failure, timeout, `429`/`502`/
    /// `503`/`504` *before* any response body is consumed); once a `2xx` body
    /// starts being read, errors are always fatal regardless of this policy.
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Build a client scoped to a given working dir. The underlying
    /// `reqwest::Client` will pool HTTP keep-alive connections per executor
    /// invocation (`spawn`, `resume`, and within a session's `query` loop —
    /// see `OpenCodeSessionImpl::client`).
    fn make_client(&self, working_dir: &Path, retry: RetryPolicy) -> OpencodeClient {
        OpencodeClient::new(
            &self.base_url,
            self.api_key.as_deref(),
            working_dir.to_str(),
        )
        .with_retry(retry)
    }

    /// Resolve the effective retry policy for a spawn/resume: a non-default
    /// [`SpawnConfig::retry_policy`] wins, otherwise the executor-level policy
    /// applies. This keeps both the builder (`with_retry_policy`) and the
    /// per-call config knobs working without an extra "unset" sentinel.
    fn effective_retry(&self, config: &SpawnConfig) -> RetryPolicy {
        if config.retry_policy == RetryPolicy::default() {
            self.retry
        } else {
            config.retry_policy
        }
    }
}

impl Default for OpencodeExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal session implementation for OpenCode.
struct OpenCodeSessionImpl {
    cost: Arc<Mutex<AgentCost>>,
    budget: f64,
    /// One client per session — preserves HTTP keep-alive across queries.
    client: OpencodeClient,
    opencode_session_id: String,
    config: SpawnConfig,
}

#[async_trait]
impl SessionImpl for OpenCodeSessionImpl {
    async fn query(&self, prompt: &str) -> Result<AgentResponse> {
        {
            let c = self.cost.lock().unwrap();
            if c.total_usd >= self.budget {
                return Err(AgentError::BudgetExceeded {
                    limit: self.budget,
                    spent: c.total_usd,
                });
            }
        }

        let resp = self
            .client
            .prompt(&self.opencode_session_id, prompt, &self.config, self.budget)
            .await?;

        {
            let mut c = self.cost.lock().unwrap();
            c.input_tokens += resp.cost.input_tokens;
            c.output_tokens += resp.cost.output_tokens;
            c.total_usd += resp.cost.total_usd;
        }

        Ok(resp)
    }

    async fn query_stream(&self, prompt: &str) -> Result<EventStream> {
        {
            let c = self.cost.lock().unwrap();
            if c.total_usd >= self.budget {
                return Err(AgentError::BudgetExceeded {
                    limit: self.budget,
                    spent: c.total_usd,
                });
            }
        }
        self.client
            .stream_events(
                self.opencode_session_id.clone(),
                prompt.to_string(),
                self.config.clone(),
                self.budget,
                // Fold the streamed turn's cost into the session total so
                // `total_cost()` and subsequent budget guards see streamed
                // spend — parity with the non-streaming `query()` path and with
                // the claude-code / codex streaming implementations.
                self.cost.clone(),
            )
            .await
    }

    async fn total_cost(&self) -> Result<AgentCost> {
        Ok(self.cost.lock().unwrap().clone())
    }

    async fn close(&self) -> Result<()> {
        // Best-effort abort of any in-flight server-side work.
        self.client.abort(&self.opencode_session_id).await
    }
}

#[async_trait]
impl AgentExecutor for OpencodeExecutor {
    fn executor_type(&self) -> ExecutorType {
        ExecutorType::OpenCode
    }

    async fn spawn(
        &self,
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession> {
        let cost = Arc::new(Mutex::new(AgentCost::default()));
        let budget = config.budget_usd.unwrap_or(f64::MAX);

        if budget <= 0.0 {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: 0.0,
            });
        }

        let client = self.make_client(working_dir, self.effective_retry(config));

        // Create session on server.
        let session_data = client.create_session().await?;
        let opencode_session_id = session_data
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Provider {
                provider: "opencode".into(),
                message: "session response missing id".into(),
            })?
            .to_string();

        // Send first prompt — reusing the same client (HTTP keep-alive).
        let response = client
            .prompt(&opencode_session_id, prompt, config, budget)
            .await?;

        {
            let mut c = cost.lock().unwrap();
            *c = response.cost.clone();
        }

        let inner = Arc::new(OpenCodeSessionImpl {
            cost: cost.clone(),
            budget,
            client,
            opencode_session_id: opencode_session_id.clone(),
            config: config.clone(),
        });

        Ok(AgentSession::new(
            opencode_session_id,
            ExecutorType::OpenCode,
            working_dir.to_path_buf(),
            config.model.clone(),
            inner,
        ))
    }

    async fn resume(
        &self,
        working_dir: &Path,
        session_id: &str,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession> {
        // OpenCode supports native session resume — we just keep prompting the
        // existing server session id.
        let cost = Arc::new(Mutex::new(AgentCost::default()));
        let budget = config.budget_usd.unwrap_or(f64::MAX);

        if budget <= 0.0 {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: 0.0,
            });
        }

        let client = self.make_client(working_dir, self.effective_retry(config));

        let response = client.prompt(session_id, prompt, config, budget).await?;

        {
            let mut c = cost.lock().unwrap();
            *c = response.cost.clone();
        }

        let inner = Arc::new(OpenCodeSessionImpl {
            cost: cost.clone(),
            budget,
            client,
            opencode_session_id: session_id.to_string(),
            config: config.clone(),
        });

        Ok(AgentSession::new(
            session_id.to_string(),
            ExecutorType::OpenCode,
            working_dir.to_path_buf(),
            config.model.clone(),
            inner,
        ))
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            session_resume: true,
            // True now that we actually parse info.tokens / tokens.
            token_usage: true,
            mcp_support: true,
            autonomous_mode: true,
            structured_output: false,
            streaming: true,
            hooks: false,
            prompt_caching: false,
            extended_thinking: false,
        }
    }

    /// Probe the configured OpenCode server with a short TCP connect.
    ///
    /// Unlike the subprocess providers there is no CLI to look for — OpenCode
    /// is a server, so the only meaningful availability signal is whether that
    /// server answers. `reason` is surfaced verbatim to end users by callers
    /// (Nucel renders it as the failure message on a skipped agent run), so it
    /// names the exact endpoint that was dialled and the action that fixes it.
    ///
    /// Costs at most [`AVAILABILITY_PROBE_TIMEOUT`] per resolved address.
    fn availability(&self) -> AvailabilityStatus {
        let Some(addr) = host_port(&self.base_url) else {
            return AvailabilityStatus {
                available: false,
                reason: Some(format!(
                    "OpenCode base URL `{}` has no host:port to connect to",
                    self.base_url
                )),
            };
        };

        if tcp_reachable(&addr, AVAILABILITY_PROBE_TIMEOUT) {
            AvailabilityStatus {
                available: true,
                reason: None,
            }
        } else {
            AvailabilityStatus {
                available: false,
                reason: Some(format!(
                    "OpenCode server not reachable at {} — start one with \
                     `opencode serve` or point the executor at a running instance",
                    self.base_url
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_type_is_opencode() {
        let exec = OpencodeExecutor::new();
        assert_eq!(exec.executor_type(), ExecutorType::OpenCode);
    }

    #[test]
    fn capabilities_declares_session_resume() {
        let caps = OpencodeExecutor::new().capabilities();
        assert!(caps.session_resume);
        assert!(caps.autonomous_mode);
        assert!(caps.mcp_support);
        assert!(caps.token_usage);
    }

    #[test]
    fn default_base_url_is_localhost() {
        let exec = OpencodeExecutor::new();
        assert_eq!(exec.base_url, "http://127.0.0.1:4096");
    }

    #[test]
    fn custom_base_url_strips_trailing_slash() {
        let exec = OpencodeExecutor::with_base_url("http://my-server:8080/");
        assert_eq!(exec.base_url, "http://my-server:8080");
    }

    #[test]
    fn host_port_keeps_explicit_port() {
        assert_eq!(
            host_port("http://127.0.0.1:4096").as_deref(),
            Some("127.0.0.1:4096")
        );
    }

    #[test]
    fn host_port_defaults_by_scheme_and_drops_path() {
        assert_eq!(
            host_port("http://example.com").as_deref(),
            Some("example.com:80")
        );
        assert_eq!(
            host_port("https://example.com").as_deref(),
            Some("example.com:443")
        );
        assert_eq!(
            host_port("http://example.com/some/path?x=1").as_deref(),
            Some("example.com:80")
        );
    }

    #[test]
    fn host_port_rejects_authority_less_url() {
        assert!(host_port("http://").is_none());
        assert!(host_port("").is_none());
    }

    /// The regression this replaces: `availability()` used to hardcode
    /// `available: true` and never touch the network, so a caller could not
    /// distinguish "server up" from "nothing listening". Port 1 on loopback is
    /// reserved and never bound, so the probe must report unavailable — and the
    /// reason must name the endpoint, because callers surface it verbatim.
    #[test]
    fn availability_is_false_when_nothing_is_listening() {
        let exec = OpencodeExecutor::with_base_url("http://127.0.0.1:1");
        let status = exec.availability();
        assert!(
            !status.available,
            "expected unavailable when no server is listening"
        );
        let reason = status.reason.expect("unavailable must carry a reason");
        assert!(
            reason.contains("http://127.0.0.1:1"),
            "reason must name the endpoint that was probed, got: {reason}"
        );
    }

    #[test]
    fn availability_is_true_when_a_listener_answers() {
        // Bind an ephemeral port and leave the listener open for the probe.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();

        let exec = OpencodeExecutor::with_base_url(format!("http://127.0.0.1:{port}"));
        let status = exec.availability();

        assert!(
            status.available,
            "expected available while a listener is bound, reason: {:?}",
            status.reason
        );
        assert!(
            status.reason.is_none(),
            "available path should not carry a failure reason"
        );
    }
}
