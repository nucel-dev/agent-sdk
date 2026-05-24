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
    AvailabilityStatus, ExecutorType, Result, SessionImpl, SpawnConfig,
};

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

        let token = self.executor.auth.token().await?;
        let url = self.executor.endpoint_for(&self.model);

        let response = self
            .executor
            .http
            .post(&url)
            .bearer_auth(token)
            .json(&req_body)
            .send()
            .await
            .map_err(|e| AgentError::Provider {
                provider: "vertex".into(),
                message: format!("HTTP error contacting Vertex: {e}"),
            })?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::RateLimited { message: body });
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::Provider {
                provider: "vertex".into(),
                message: format!("Vertex returned {status}: {body}"),
            });
        }

        let parsed: RawPredictResponse =
            response.json().await.map_err(|e| AgentError::Provider {
                provider: "vertex".into(),
                message: format!("invalid JSON from Vertex: {e}"),
            })?;

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
            streaming: false,
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
        assert!(!caps.streaming);
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
