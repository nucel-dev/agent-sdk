//! GCP Vertex AI provider — Claude models via the Vertex `rawPredict`
//! Anthropic Messages endpoint.
//!
//! Vertex serves Anthropic models through a standard Anthropic-style
//! Messages API hosted at:
//!
//! ```text
//! https://<region>-aiplatform.googleapis.com/v1/projects/<project>/locations/<region>/publishers/anthropic/models/<model>:rawPredict
//! ```
//!
//! Authentication uses Google Application Default Credentials minted into
//! a `Bearer` token for the `cloud-platform` scope. Tests can swap in a
//! [`StaticToken`] provider to bypass GCP.
//!
//! # Minimal example
//!
//! ```no_run
//! use nucel_agent_vertex::VertexExecutor;
//! use nucel_agent_core::{AgentExecutor, SpawnConfig};
//! use std::path::Path;
//!
//! # async fn run() -> nucel_agent_core::Result<()> {
//! let executor = VertexExecutor::with_adc("my-gcp-project", "us-east5").await?;
//! let session = executor.spawn(
//!     Path::new("/my/repo"),
//!     "Summarize this codebase.",
//!     &SpawnConfig {
//!         model: Some("claude-opus-4-7@20251024".into()),
//!         budget_usd: Some(2.0),
//!         ..Default::default()
//!     },
//! ).await?;
//! println!("{}", session.query("Any TODOs?").await?.content);
//! session.close().await?;
//! # Ok(()) }
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

mod auth;
mod pricing;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse, AgentSession,
    AvailabilityStatus, EventStream, ExecutorType, MessageEvent, Result, RetryPolicy, SessionImpl,
    SpawnConfig,
};
use tokio::sync::mpsc::Sender;

pub use auth::{AdcToken, StaticToken, TokenProvider};
pub use pricing::{lookup as lookup_price, ModelPrice};

/// Default model when [`SpawnConfig::model`] is `None`.
pub const DEFAULT_MODEL: &str = "claude-opus-4-7@20251024";

/// Vertex AI's required Anthropic protocol version header value.
const ANTHROPIC_VERSION: &str = "vertex-2023-10-16";

/// Vertex executor — issues HTTP POST against the Vertex Anthropic
/// `rawPredict` endpoint.
pub struct VertexExecutor {
    project: String,
    region: String,
    auth: Arc<dyn TokenProvider>,
    http: reqwest::Client,
    /// Override of the API root — defaults to
    /// `https://<region>-aiplatform.googleapis.com`. Tests point this at
    /// a `wiremock::MockServer`.
    api_root: Option<String>,
    /// Retry policy for *transient*, pre-side-effect failures (connection
    /// errors, `429`, `503` before any response body is read). Defaults to
    /// [`RetryPolicy::default`].
    retry: RetryPolicy,
}

impl VertexExecutor {
    /// Build with an arbitrary token provider — useful for tests or for
    /// callers that mint their own service-account tokens.
    pub fn new(
        project: impl Into<String>,
        region: impl Into<String>,
        auth: Arc<dyn TokenProvider>,
    ) -> Self {
        Self {
            project: project.into(),
            region: region.into(),
            auth,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .expect("reqwest client builds"),
            api_root: None,
            retry: RetryPolicy::default(),
        }
    }

    /// Build using GCP Application Default Credentials.
    ///
    /// Returns [`AgentError::Config`] if no GCP credentials are reachable.
    pub async fn with_adc(
        project: impl Into<String>,
        region: impl Into<String>,
    ) -> Result<Self> {
        let auth: Arc<dyn TokenProvider> = Arc::new(AdcToken::discover().await?);
        Ok(Self::new(project, region, auth))
    }

    /// Build with a pre-minted static bearer token (testing or sidecar
    /// flows).
    pub fn with_static_token(
        project: impl Into<String>,
        region: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        let auth: Arc<dyn TokenProvider> = Arc::new(StaticToken::new(token));
        Self::new(project, region, auth)
    }

    /// Override the API root URL — only meaningful for tests. Production
    /// callers should leave this unset so the regional endpoint is used.
    pub fn with_api_root(mut self, root: impl Into<String>) -> Self {
        self.api_root = Some(root.into().trim_end_matches('/').to_string());
        self
    }

    /// Override the retry policy for transient, pre-side-effect failures.
    ///
    /// Pass [`RetryPolicy::none`] to disable retrying entirely. Retries only
    /// ever apply to the request-dispatch phase (connection failure, `429`,
    /// `503` *before* any response body is consumed); once a `2xx` body starts
    /// streaming, errors are always fatal regardless of this policy.
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Resolve the regional `rawPredict` URL for `model`. Public mostly
    /// so integration tests can assert on the URL shape; production
    /// callers don't need it.
    pub fn endpoint_for(&self, model: &str) -> String {
        let root = self.api_root.clone().unwrap_or_else(|| {
            format!("https://{}-aiplatform.googleapis.com", self.region)
        });
        format!(
            "{root}/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:rawPredict",
            root = root,
            project = self.project,
            region = self.region,
            model = model,
        )
    }
}

// ── Wire types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct VertexMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
struct RawPredictRequest<'a> {
    anthropic_version: &'a str,
    messages: Vec<VertexMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawPredictResponse {
    #[serde(default)]
    content: Vec<ResponseBlock>,
    #[serde(default)]
    usage: Option<UsageInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct ResponseBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct UsageInfo {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

// ── Session ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct VertexSessionImpl {
    executor: Arc<VertexExecutorInner>,
    model: String,
    transcript: Arc<Mutex<Vec<VertexMessage>>>,
    cost: Arc<Mutex<AgentCost>>,
    budget: f64,
    system_prompt: Option<String>,
    max_tokens: u32,
}

/// Internal shareable state for the executor (so `Arc` clones are cheap).
struct VertexExecutorInner {
    project: String,
    region: String,
    auth: Arc<dyn TokenProvider>,
    http: reqwest::Client,
    api_root: Option<String>,
    retry: RetryPolicy,
}

impl VertexExecutorInner {
    fn endpoint_for(&self, model: &str) -> String {
        let root = self.api_root.clone().unwrap_or_else(|| {
            format!("https://{}-aiplatform.googleapis.com", self.region)
        });
        format!(
            "{root}/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:rawPredict",
            root = root,
            project = self.project,
            region = self.region,
            model = model,
        )
    }
}

impl VertexSessionImpl {
    async fn run_turn(&self, prompt: &str) -> Result<AgentResponse> {
        self.run_turn_inner(prompt, None).await
    }

    /// Run a single turn, optionally forwarding `ApiRetry` events to a
    /// streaming sink when a transient pre-side-effect failure is retried.
    async fn run_turn_inner(
        &self,
        prompt: &str,
        retry_sink: Option<&Sender<Result<MessageEvent>>>,
    ) -> Result<AgentResponse> {
        // Budget gate.
        {
            let c = self.cost.lock().unwrap();
            if c.total_usd >= self.budget {
                return Err(AgentError::BudgetExceeded {
                    limit: self.budget,
                    spent: c.total_usd,
                });
            }
        }

        // Append user turn and snapshot messages.
        let messages: Vec<VertexMessage> = {
            let mut t = self.transcript.lock().unwrap();
            t.push(VertexMessage {
                role: "user".into(),
                content: prompt.to_string(),
            });
            t.clone()
        };

        let req_body = RawPredictRequest {
            anthropic_version: ANTHROPIC_VERSION,
            messages,
            max_tokens: self.max_tokens,
            system: self.system_prompt.clone(),
        };

        let url = self.executor.endpoint_for(&self.model);

        // ── Dispatch with bounded retry ──────────────────────────────────
        //
        // SIDE-EFFECT RULE: we retry ONLY the pre-side-effect window — token
        // mint, connection establishment, and an outright server rejection
        // (`429`/`503`) where *no response body has been consumed* and the
        // model therefore did no work. The instant we begin reading a `2xx`
        // body (where tokens have been generated and cost incurred), errors
        // are fatal and never retried. This guarantees a retry can never
        // double-charge or duplicate a completed turn.
        let policy = self.executor.retry;
        let mut retries_done: u32 = 0;
        let parsed: RawPredictResponse = loop {
            // Mint a fresh token each attempt — a transient auth blip on one
            // try shouldn't poison the retry.
            let token = self.executor.auth.token().await?;

            let send_result = self
                .executor
                .http
                .post(&url)
                .bearer_auth(token)
                .json(&req_body)
                .send()
                .await;

            // Classify the dispatch outcome into Ok(body) | retryable | fatal.
            let attempt: Result<RawPredictResponse> = match send_result {
                Err(e) => {
                    // Connection/timeout failures: classify by reqwest flags.
                    if e.is_timeout() {
                        Err(AgentError::Timeout { seconds: 300 })
                    } else if e.is_connect() || e.is_request() {
                        // No bytes of a response body were consumed → safe to
                        // retry. Map to a transient Io error.
                        Err(AgentError::Io(std::io::Error::new(
                            std::io::ErrorKind::ConnectionReset,
                            format!("HTTP error contacting Vertex: {e}"),
                        )))
                    } else {
                        Err(AgentError::Provider {
                            provider: "vertex".into(),
                            message: format!("HTTP error contacting Vertex: {e}"),
                        })
                    }
                }
                Ok(response) => {
                    let status = response.status();
                    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        // Body not consumed for the model's sake — retryable.
                        let body = response.text().await.unwrap_or_default();
                        Err(AgentError::RateLimited { message: body })
                    } else if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
                        || status == reqwest::StatusCode::GATEWAY_TIMEOUT
                        || status == reqwest::StatusCode::BAD_GATEWAY
                    {
                        // Upstream not ready / overloaded; no work done.
                        let body = response.text().await.unwrap_or_default();
                        Err(AgentError::RateLimited {
                            message: format!("{status}: {body}"),
                        })
                    } else if !status.is_success() {
                        // 4xx (other than 429) / hard 5xx → fatal.
                        let body = response.text().await.unwrap_or_default();
                        Err(AgentError::Provider {
                            provider: "vertex".into(),
                            message: format!("Vertex returned {status}: {body}"),
                        })
                    } else {
                        // 2xx — we are now PAST the side-effect boundary.
                        // A decode failure here is fatal: do not replay.
                        response.json::<RawPredictResponse>().await.map_err(|e| {
                            AgentError::Provider {
                                provider: "vertex".into(),
                                message: format!("invalid JSON from Vertex: {e}"),
                            }
                        })
                    }
                }
            };

            match attempt {
                Ok(body) => break body,
                Err(err) => {
                    if policy.should_retry(&err, retries_done) {
                        let backoff = policy.backoff_for(retries_done);
                        let attempt_no = retries_done + 1;
                        tracing::warn!(
                            attempt = attempt_no,
                            backoff_ms = backoff.as_millis() as u64,
                            "vertex transient failure; retrying"
                        );
                        // Make the retry observable on the stream, if any.
                        if let Some(tx) = retry_sink {
                            let _ = tx
                                .send(Ok(MessageEvent::ApiRetry {
                                    attempt: attempt_no,
                                    message: format!("transient vertex failure: {err}"),
                                }))
                                .await;
                        }
                        tokio::time::sleep(backoff).await;
                        retries_done += 1;
                        continue;
                    }
                    return Err(err);
                }
            }
        };

        // Collect assistant text.
        let mut text = String::new();
        for block in &parsed.content {
            if block.block_type == "text" {
                text.push_str(&block.text);
            }
        }

        // Record assistant turn in transcript.
        self.transcript.lock().unwrap().push(VertexMessage {
            role: "assistant".into(),
            content: text.clone(),
        });

        let usage = parsed.usage.unwrap_or_default();
        let price = pricing::lookup(&self.model);
        let usd = price
            .map(|p| p.estimate(usage.input_tokens, usage.output_tokens))
            .unwrap_or(0.0);

        let turn_cost = AgentCost {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
            cache_creation_tokens: usage.cache_creation_input_tokens,
            total_usd: usd,
        };

        {
            let mut c = self.cost.lock().unwrap();
            c.input_tokens += turn_cost.input_tokens;
            c.output_tokens += turn_cost.output_tokens;
            c.cache_read_tokens += turn_cost.cache_read_tokens;
            c.cache_creation_tokens += turn_cost.cache_creation_tokens;
            c.total_usd += turn_cost.total_usd;
        }

        Ok(AgentResponse {
            content: text,
            cost: turn_cost,
            confidence: None,
            requests_escalation: false,
            tool_calls: vec![],
        })
    }
}

#[async_trait]
impl SessionImpl for VertexSessionImpl {
    async fn query(&self, prompt: &str) -> Result<AgentResponse> {
        self.run_turn(prompt).await
    }

    /// Streaming variant.
    ///
    /// Vertex's `rawPredict` is a single non-streaming request, so this isn't a
    /// token-level stream — but it surfaces [`MessageEvent::ApiRetry`] events
    /// live while the (retried) request is in flight, then terminates with a
    /// single `TextChunk` + `ResultDone`. This makes transient retries
    /// observable in `query_stream()` exactly as the umbrella API promises.
    async fn query_stream(&self, prompt: &str) -> Result<EventStream> {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<MessageEvent>>(16);
        // Every field is an `Arc`/`Copy`/`String`, so a clone is cheap and
        // shares the same transcript + cost state. We move it onto a task so
        // retries can stream out on `tx` while the request is in flight.
        let this = self.clone();
        let prompt = prompt.to_string();

        tokio::spawn(async move {
            match this.run_turn_inner(&prompt, Some(&tx)).await {
                Ok(resp) => {
                    let _ = tx
                        .send(Ok(MessageEvent::TextChunk {
                            text: resp.content.clone(),
                        }))
                        .await;
                    let _ = tx
                        .send(Ok(MessageEvent::ResultDone {
                            cost: resp.cost,
                            content: resp.content,
                            is_error: false,
                        }))
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(Ok(MessageEvent::Error {
                            message: e.to_string(),
                        }))
                        .await;
                }
            }
        });

        // Adapt the mpsc receiver into a `Stream` without pulling in
        // `tokio-stream` as a dependency.
        let stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        Ok(Box::pin(stream))
    }

    async fn total_cost(&self) -> Result<AgentCost> {
        Ok(self.cost.lock().unwrap().clone())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl AgentExecutor for VertexExecutor {
    fn executor_type(&self) -> ExecutorType {
        ExecutorType::ClaudeCode
    }

    async fn spawn(
        &self,
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession> {
        let budget = config.budget_usd.unwrap_or(f64::MAX);
        if budget <= 0.0 {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: 0.0,
            });
        }

        let model = config
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let session_id = format!("vertex-{}", uuid::Uuid::new_v4());

        let inner_exec = Arc::new(VertexExecutorInner {
            project: self.project.clone(),
            region: self.region.clone(),
            auth: Arc::clone(&self.auth),
            http: self.http.clone(),
            api_root: self.api_root.clone(),
            retry: self.retry,
        });

        let inner = Arc::new(VertexSessionImpl {
            executor: inner_exec,
            model: model.clone(),
            transcript: Arc::new(Mutex::new(Vec::new())),
            cost: Arc::new(Mutex::new(AgentCost::default())),
            budget,
            system_prompt: config.system_prompt.clone(),
            max_tokens: config.max_tokens.unwrap_or(4096),
        });

        let first = inner.run_turn(prompt).await?;

        tracing::debug!(
            content_len = first.content.len(),
            input = first.cost.input_tokens,
            output = first.cost.output_tokens,
            "vertex spawn first turn complete"
        );

        Ok(AgentSession::new(
            session_id,
            ExecutorType::ClaudeCode,
            working_dir.to_path_buf(),
            Some(model),
            inner,
        ))
    }

    async fn resume(
        &self,
        _working_dir: &Path,
        _session_id: &str,
        _prompt: &str,
        _config: &SpawnConfig,
    ) -> Result<AgentSession> {
        Err(AgentError::Provider {
            provider: "vertex".into(),
            message: "Vertex provider does not support resume — sessions are \
                      client-side only. Re-spawn with the saved transcript."
                .into(),
        })
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            session_resume: false,
            token_usage: true,
            mcp_support: false,
            autonomous_mode: false,
            structured_output: false,
            // `query_stream()` is implemented: it surfaces `ApiRetry` events
            // live and terminates with `TextChunk` + `ResultDone`.
            streaming: true,
            hooks: false,
            // Vertex passes through Anthropic's cache_read/creation tokens
            // when present in upstream responses.
            prompt_caching: true,
            extended_thinking: false,
        }
    }

    fn availability(&self) -> AvailabilityStatus {
        if self.project.is_empty() {
            return AvailabilityStatus {
                available: false,
                reason: Some("Vertex project id is empty".into()),
            };
        }
        if self.region.is_empty() {
            return AvailabilityStatus {
                available: false,
                reason: Some("Vertex region is empty".into()),
            };
        }
        AvailabilityStatus {
            available: true,
            reason: Some(format!(
                "Vertex region={} project={}",
                self.region, self.project
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_executor() -> VertexExecutor {
        VertexExecutor::with_static_token("my-proj", "us-east5", "token123")
    }

    #[test]
    fn executor_type_is_claude_code() {
        assert_eq!(test_executor().executor_type(), ExecutorType::ClaudeCode);
    }

    #[test]
    fn endpoint_uses_region_and_project() {
        let url = test_executor().endpoint_for("claude-opus-4-7@20251024");
        assert!(url.contains("us-east5-aiplatform.googleapis.com"));
        assert!(url.contains("projects/my-proj"));
        assert!(url.contains("locations/us-east5"));
        assert!(url.contains("publishers/anthropic"));
        assert!(url.ends_with(":rawPredict"));
    }

    #[test]
    fn api_root_override_takes_precedence() {
        let exec =
            test_executor().with_api_root("http://localhost:9999");
        let url = exec.endpoint_for("claude-opus-4-7@20251024");
        assert!(url.starts_with("http://localhost:9999"));
        assert!(!url.contains("googleapis.com"));
    }

    #[test]
    fn capabilities_expected_shape() {
        let caps = test_executor().capabilities();
        assert!(caps.token_usage);
        assert!(caps.prompt_caching);
        assert!(!caps.session_resume);
        // Streaming is now implemented (surfaces ApiRetry + ResultDone).
        assert!(caps.streaming);
    }

    #[test]
    fn availability_reports_empty_project() {
        let exec =
            VertexExecutor::with_static_token("", "us-east5", "token123");
        let avail = exec.availability();
        assert!(!avail.available);
        assert!(avail.reason.unwrap().contains("project"));
    }

    #[test]
    fn availability_reports_empty_region() {
        let exec = VertexExecutor::with_static_token("p", "", "token");
        let avail = exec.availability();
        assert!(!avail.available);
        assert!(avail.reason.unwrap().contains("region"));
    }

    #[test]
    fn default_model_is_claude_opus_4_7() {
        assert!(DEFAULT_MODEL.contains("claude-opus-4-7"));
    }

    #[tokio::test]
    async fn resume_returns_provider_error() {
        let exec = test_executor();
        let err = exec
            .resume(
                Path::new("/tmp"),
                "some",
                "hi",
                &SpawnConfig::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::Provider { .. }));
    }

    #[tokio::test]
    async fn zero_budget_rejected() {
        let exec = test_executor();
        let cfg = SpawnConfig {
            budget_usd: Some(0.0),
            ..Default::default()
        };
        let err = exec
            .spawn(Path::new("/tmp"), "hi", &cfg)
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::BudgetExceeded { .. }));
    }
}
