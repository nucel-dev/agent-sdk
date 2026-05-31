//! AWS Bedrock provider — Claude models via the Bedrock Runtime `Converse`
//! API.
//!
//! This crate implements [`AgentExecutor`] on top of `aws-sdk-bedrockruntime`.
//! It is a thin shim: each `query()` issues one `Converse` request, the
//! transcript is kept client-side in `Arc<Mutex<Vec<Message>>>`, and cost is
//! estimated from invocation-metadata token counts.
//!
//! # Credentials
//!
//! Credentials are resolved with the **default AWS provider chain** —
//! environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SESSION_TOKEN`),
//! `~/.aws/credentials`, IMDS, ECS task roles, SSO, etc. If no credentials
//! are configured, `availability()` reports the failure but `spawn()` will
//! still attempt the call so the actual SDK error reaches the caller.
//!
//! # Minimal example
//!
//! ```no_run
//! use nucel_agent_bedrock::BedrockExecutor;
//! use nucel_agent_core::{AgentExecutor, SpawnConfig};
//! use std::path::Path;
//!
//! # async fn run() -> nucel_agent_core::Result<()> {
//! let executor = BedrockExecutor::new().await;
//! let session = executor.spawn(
//!     Path::new("/my/repo"),
//!     "Summarize this codebase.",
//!     &SpawnConfig {
//!         model: Some("anthropic.claude-opus-4-7-20251024-v2:0".into()),
//!         budget_usd: Some(2.0),
//!         ..Default::default()
//!     },
//! ).await?;
//! println!("{}", session.query("Any TODOs?").await?.content);
//! session.close().await?;
//! # Ok(()) }
//! ```
//!
//! See also the runnable tutorial at
//! `docs/tutorials/bedrock-vertex.md`.

#![cfg_attr(docsrs, feature(doc_cfg))]

mod pricing;

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use aws_sdk_bedrockruntime::error::SdkError;
use aws_sdk_bedrockruntime::operation::converse::ConverseError;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, InferenceConfiguration, Message, SystemContentBlock,
};
use aws_sdk_bedrockruntime::Client as BedrockClient;

use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse, AgentSession,
    AvailabilityStatus, ExecutorType, Result, SessionImpl, SpawnConfig,
};

pub use pricing::{lookup as lookup_price, ModelPrice};

/// Default model used when [`SpawnConfig::model`] is `None`.
pub const DEFAULT_MODEL: &str = "anthropic.claude-opus-4-7-20251024-v2:0";

/// Bedrock executor — wraps an `aws-sdk-bedrockruntime` client.
pub struct BedrockExecutor {
    client: BedrockClient,
    /// Optional override for credential-availability check.
    has_credentials: bool,
}

impl BedrockExecutor {
    /// Create a new executor using the default AWS provider chain
    /// (`aws_config::defaults`).
    pub async fn new() -> Self {
        use aws_config::BehaviorVersion;
        let conf = aws_config::defaults(BehaviorVersion::latest()).load().await;
        let has_credentials = conf.credentials_provider().is_some();
        let client = BedrockClient::new(&conf);
        Self {
            client,
            has_credentials,
        }
    }

    /// Build from an existing Bedrock client (lets callers customize
    /// retries, region, identity_cache, etc.).
    pub fn from_client(client: BedrockClient) -> Self {
        Self {
            client,
            // We can't introspect the borrowed client config for creds
            // without re-loading; assume callers passing a hand-built
            // client know what they're doing.
            has_credentials: true,
        }
    }

    /// Reference to the underlying SDK client — useful in tests or for
    /// callers that want to issue ad-hoc Bedrock calls outside the
    /// `AgentExecutor` flow.
    pub fn client(&self) -> &BedrockClient {
        &self.client
    }
}

/// Classify a Bedrock `Converse` SDK error into the SDK-wide [`AgentError`]
/// taxonomy so callers (and the umbrella's `retry::is_transient`) can tell a
/// *transient* throttle/overload apart from a *fatal* request error.
///
/// Mapping:
/// - `ThrottlingException` / `ServiceUnavailableException` → [`AgentError::RateLimited`]
///   (transient: the model did no work, the upstream asked us to back off).
/// - `ModelTimeoutException`, and the transport-level `SdkError::TimeoutError`
///   → [`AgentError::Timeout`] (transient: never got a response).
/// - `SdkError::DispatchFailure` (DNS/connect/TLS — request never left) →
///   [`AgentError::Io`] with a transient kind (safe to retry pre-side-effect).
/// - Everything else (validation, access-denied, model errors, 4xx, decode) →
///   [`AgentError::Provider`] (fatal: replaying would fail identically or has
///   already had a side effect).
///
/// Note the AWS SDK applies its *own* retry layer to throttling/5xx before this
/// classifier ever sees the error, so by the time a `ThrottlingException`
/// surfaces here the SDK has already exhausted its standard retry budget. We
/// still classify it as `RateLimited` so the error *type* is honest and any
/// caller-side policy can react.
fn classify_converse_error(err: &SdkError<ConverseError>) -> AgentError {
    // Transport-level failures that never reached the service.
    match err {
        SdkError::TimeoutError(_) => {
            return AgentError::Timeout { seconds: 0 };
        }
        SdkError::DispatchFailure(_) | SdkError::ConstructionFailure(_) => {
            return AgentError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                format!("Bedrock request dispatch failed: {err}"),
            ));
        }
        _ => {}
    }

    // Service-modeled errors: inspect the typed `ConverseError`.
    if let Some(service_err) = err.as_service_error() {
        if service_err.is_throttling_exception() || service_err.is_service_unavailable_exception() {
            return AgentError::RateLimited {
                message: format!("Bedrock throttled/unavailable: {err}"),
            };
        }
        if service_err.is_model_timeout_exception() {
            return AgentError::Timeout { seconds: 0 };
        }
    }

    // Anything else is fatal.
    AgentError::Provider {
        provider: "bedrock".into(),
        message: format!("Bedrock Converse failed: {err}"),
    }
}

/// Internal session — holds transcript and accumulated cost.
struct BedrockSessionImpl {
    client: BedrockClient,
    model_id: String,
    transcript: Arc<Mutex<Vec<Message>>>,
    cost: Arc<Mutex<AgentCost>>,
    budget: f64,
    system_prompt: Option<String>,
    max_tokens: Option<u32>,
}

impl BedrockSessionImpl {
    async fn run_turn(&self, prompt: &str) -> Result<AgentResponse> {
        // Budget check up-front.
        {
            let c = self.cost.lock().unwrap();
            if c.total_usd >= self.budget {
                return Err(AgentError::BudgetExceeded {
                    limit: self.budget,
                    spent: c.total_usd,
                });
            }
        }

        // Append user turn.
        let user_msg = Message::builder()
            .role(ConversationRole::User)
            .content(ContentBlock::Text(prompt.to_string()))
            .build()
            .map_err(|e| AgentError::Provider {
                provider: "bedrock".into(),
                message: format!("failed to build user message: {e}"),
            })?;

        let messages: Vec<Message> = {
            let mut t = self.transcript.lock().unwrap();
            t.push(user_msg);
            t.clone()
        };

        // Build the Converse request.
        let mut req = self
            .client
            .converse()
            .model_id(&self.model_id)
            .set_messages(Some(messages));

        if let Some(sp) = &self.system_prompt {
            req = req.system(SystemContentBlock::Text(sp.clone()));
        }

        if let Some(max) = self.max_tokens {
            let cfg = InferenceConfiguration::builder()
                .max_tokens(max as i32)
                .build();
            req = req.inference_config(cfg);
        }

        let out = req
            .send()
            .await
            .map_err(|e| classify_converse_error(&e))?;

        // Extract assistant text from the output. The Converse response shape
        // is `output.message.content: Vec<ContentBlock>`.
        let mut text = String::new();
        let assistant_message_opt = out.output().and_then(|o| o.as_message().ok().cloned());

        if let Some(msg) = &assistant_message_opt {
            for block in msg.content() {
                if let ContentBlock::Text(t) = block {
                    text.push_str(t);
                }
            }
            // Append to transcript for multi-turn.
            self.transcript.lock().unwrap().push(msg.clone());
        }

        // Token usage from invocation metadata. Bedrock Converse reports
        // prompt-cache effects via `cacheReadInputTokens` /
        // `cacheWriteInputTokens` when a `cachePoint` was used; surface them so
        // cost analytics see cache hits/writes. Negative/None values clamp to 0.
        let usage = out.usage();
        let (input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens) = match usage {
            Some(u) => (
                u.input_tokens().max(0) as u64,
                u.output_tokens().max(0) as u64,
                u.cache_read_input_tokens().unwrap_or(0).max(0) as u64,
                u.cache_write_input_tokens().unwrap_or(0).max(0) as u64,
            ),
            None => (0, 0, 0, 0),
        };

        let price = pricing::lookup(&self.model_id);
        let usd = price
            .map(|p| p.estimate(input_tokens, output_tokens))
            .unwrap_or(0.0);

        let cost = AgentCost {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            total_usd: usd,
        };

        // Accumulate.
        {
            let mut c = self.cost.lock().unwrap();
            c.input_tokens += cost.input_tokens;
            c.output_tokens += cost.output_tokens;
            c.cache_read_tokens += cost.cache_read_tokens;
            c.cache_creation_tokens += cost.cache_creation_tokens;
            c.total_usd += cost.total_usd;
        }

        Ok(AgentResponse {
            content: text,
            cost,
            confidence: None,
            requests_escalation: false,
            tool_calls: vec![],
        })
    }
}

#[async_trait]
impl SessionImpl for BedrockSessionImpl {
    async fn query(&self, prompt: &str) -> Result<AgentResponse> {
        self.run_turn(prompt).await
    }

    async fn total_cost(&self) -> Result<AgentCost> {
        Ok(self.cost.lock().unwrap().clone())
    }

    async fn close(&self) -> Result<()> {
        // No server-side resources to release — transcript is in-process.
        Ok(())
    }
}

#[async_trait]
impl AgentExecutor for BedrockExecutor {
    fn executor_type(&self) -> ExecutorType {
        // Bedrock-served Claude reports as ClaudeCode for ExecutorType
        // routing purposes (the core enum is closed to additions in 0.2.x).
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

        let model_id = config
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let session_id = format!("bedrock-{}", uuid::Uuid::new_v4());

        let inner = Arc::new(BedrockSessionImpl {
            client: self.client.clone(),
            model_id: model_id.clone(),
            transcript: Arc::new(Mutex::new(Vec::new())),
            cost: Arc::new(Mutex::new(AgentCost::default())),
            budget,
            system_prompt: config.system_prompt.clone(),
            max_tokens: config.max_tokens,
        });

        // Issue the first turn so the caller can immediately read `content`.
        let first = inner.run_turn(prompt).await?;

        let session = AgentSession::new(
            session_id,
            ExecutorType::ClaudeCode,
            working_dir.to_path_buf(),
            Some(model_id),
            inner,
        );

        // Make sure the first-turn output isn't lost: log it (the SessionImpl
        // already accumulated cost + transcript).
        tracing::debug!(
            content_len = first.content.len(),
            input = first.cost.input_tokens,
            output = first.cost.output_tokens,
            "bedrock spawn first turn complete"
        );

        Ok(session)
    }

    async fn resume(
        &self,
        _working_dir: &Path,
        _session_id: &str,
        _prompt: &str,
        _config: &SpawnConfig,
    ) -> Result<AgentSession> {
        // Bedrock has no server-side session store; client-side transcripts
        // can't be looked up by id. Callers should persist the transcript
        // themselves and re-spawn.
        Err(AgentError::Provider {
            provider: "bedrock".into(),
            message: "Bedrock provider does not support resume — sessions are \
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
            streaming: false,
            hooks: false,
            // Bedrock Converse surfaces cache_read/cache_write input tokens when
            // a `cachePoint` is present; we capture them into `AgentCost`.
            prompt_caching: true,
            extended_thinking: false,
        }
    }

    fn availability(&self) -> AvailabilityStatus {
        if self.has_credentials {
            AvailabilityStatus {
                available: true,
                reason: None,
            }
        } else {
            AvailabilityStatus {
                available: false,
                reason: Some(
                    "AWS credentials not found — configure via environment, \
                     ~/.aws/credentials, or instance profile."
                        .into(),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // We avoid hitting the real AWS endpoint in unit tests by constructing
    // the executor via `from_client` with a dummy client built from a
    // never-loaded SdkConfig. Real integration coverage lives in
    // `tests/bedrock_integration.rs` behind the aws-smithy-mocks crate.

    fn dummy_client() -> BedrockClient {
        // Build with default region us-east-1 + no creds. This client will
        // refuse to actually send requests, but for capability/static
        // tests that's fine.
        let conf = aws_sdk_bedrockruntime::Config::builder()
            .behavior_version(aws_sdk_bedrockruntime::config::BehaviorVersion::latest())
            .region(aws_sdk_bedrockruntime::config::Region::new("us-east-1"))
            .build();
        BedrockClient::from_conf(conf)
    }

    #[test]
    fn executor_type_is_claude_code() {
        let exec = BedrockExecutor::from_client(dummy_client());
        assert_eq!(exec.executor_type(), ExecutorType::ClaudeCode);
    }

    #[test]
    fn capabilities_match_expected_surface() {
        let caps = BedrockExecutor::from_client(dummy_client()).capabilities();
        assert!(caps.token_usage, "Bedrock reports token usage");
        assert!(!caps.session_resume, "Bedrock has no server-side sessions");
        assert!(!caps.streaming, "Bedrock provider uses Converse (non-stream)");
        assert!(!caps.mcp_support, "Bedrock provider does not bridge MCP");
        assert!(
            caps.prompt_caching,
            "Bedrock surfaces cache_read/cache_write tokens"
        );
    }

    #[test]
    fn availability_marks_unknown_creds_as_available() {
        // from_client assumes caller-managed creds — should report available.
        let avail = BedrockExecutor::from_client(dummy_client()).availability();
        assert!(avail.available);
    }

    #[test]
    fn default_model_id_is_claude_opus_4_7() {
        assert!(DEFAULT_MODEL.contains("claude-opus-4-7"));
    }

    #[tokio::test]
    async fn resume_returns_provider_error() {
        let exec = BedrockExecutor::from_client(dummy_client());
        let err = exec
            .resume(
                Path::new("/tmp"),
                "some-session",
                "hi",
                &SpawnConfig::default(),
            )
            .await
            .unwrap_err();
        match err {
            AgentError::Provider { provider, .. } => {
                assert_eq!(provider, "bedrock");
            }
            _ => panic!("expected Provider error, got {err:?}"),
        }
    }

    #[tokio::test]
    async fn zero_budget_rejected_at_spawn() {
        let exec = BedrockExecutor::from_client(dummy_client());
        let cfg = SpawnConfig {
            budget_usd: Some(0.0),
            ..Default::default()
        };
        let err = exec
            .spawn(Path::new("/tmp"), "hi", &cfg)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AgentError::BudgetExceeded { .. }),
            "zero budget must be rejected at spawn: {err:?}"
        );
    }

    #[test]
    fn pricing_lookup_known_model() {
        let p = pricing::lookup("anthropic.claude-opus-4-7-20251024-v2:0");
        assert!(p.is_some());
    }
}
