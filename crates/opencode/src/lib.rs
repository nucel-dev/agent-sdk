//! OpenCode provider — HTTP client to OpenCode server.
//!
//! OpenCode runs as a server (`opencode serve` on `:4096`). This provider
//! connects to it via HTTP REST API.
//!
//! Supports:
//! - Session creation and prompting
//! - Multi-turn conversations
//! - Session resume (native)

mod client;
mod protocol;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uuid::Uuid;

use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse, AgentSession,
    AvailabilityStatus, ExecutorType, Result, SessionImpl, SpawnConfig,
};

use client::OpencodeClient;

/// OpenCode executor — connects to OpenCode HTTP server.
pub struct OpencodeExecutor {
    base_url: String,
    api_key: Option<String>,
}

impl OpencodeExecutor {
    pub fn new() -> Self {
        Self {
            base_url: "http://127.0.0.1:4096".to_string(),
            api_key: None,
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: None,
        }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
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
    base_url: String,
    api_key: Option<String>,
    working_dir: PathBuf,
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

        let client = OpencodeClient::new(
            &self.base_url,
            self.api_key.as_deref(),
            self.working_dir.to_str(),
        );

        let resp = client
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

    async fn total_cost(&self) -> Result<AgentCost> {
        Ok(self.cost.lock().unwrap().clone())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
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
        let session_id = Uuid::new_v4().to_string();
        let cost = Arc::new(Mutex::new(AgentCost::default()));
        let budget = config.budget_usd.unwrap_or(f64::MAX);

        if budget <= 0.0 {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: 0.0,
            });
        }

        let client = OpencodeClient::new(
            &self.base_url,
            self.api_key.as_deref(),
            working_dir.to_str(),
        );

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

        // Send first prompt.
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
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            working_dir: working_dir.to_path_buf(),
            opencode_session_id,
            config: config.clone(),
        });

        Ok(AgentSession::new(
            session_id,
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
        // OpenCode supports native session resume.
        let cost = Arc::new(Mutex::new(AgentCost::default()));
        let budget = config.budget_usd.unwrap_or(f64::MAX);

        let client = OpencodeClient::new(
            &self.base_url,
            self.api_key.as_deref(),
            working_dir.to_str(),
        );

        let response = client
            .prompt(session_id, prompt, config, budget)
            .await?;

        {
            let mut c = cost.lock().unwrap();
            *c = response.cost.clone();
        }

        let new_session_id = Uuid::new_v4().to_string();

        let inner = Arc::new(OpenCodeSessionImpl {
            cost: cost.clone(),
            budget,
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            working_dir: working_dir.to_path_buf(),
            opencode_session_id: session_id.to_string(),
            config: config.clone(),
        });

        Ok(AgentSession::new(
            new_session_id,
            ExecutorType::OpenCode,
            working_dir.to_path_buf(),
            config.model.clone(),
            inner,
        ))
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            session_resume: true,
            token_usage: true,
            mcp_support: true,
            autonomous_mode: true,
            structured_output: false,
        }
    }

    fn availability(&self) -> AvailabilityStatus {
        AvailabilityStatus {
            available: true,
            reason: Some(format!(
                "Run `opencode serve` to start server at {}",
                self.base_url
            )),
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
}
