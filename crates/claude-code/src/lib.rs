//! Claude Code provider — wraps the `claude` CLI as a subprocess.
//!
//! Communicates via JSONL stdio protocol. Supports:
//! - One-shot and multi-turn queries
//! - Cost tracking per session
//! - Permission mode configuration
//! - Budget enforcement

mod process;
mod protocol;

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use uuid::Uuid;

use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse, AgentSession,
    AvailabilityStatus, ExecutorType, Result, SessionImpl, SpawnConfig,
};

use process::ClaudeProcess;

/// Claude Code executor — spawns `claude` CLI subprocess.
pub struct ClaudeCodeExecutor {
    api_key: Option<String>,
}

impl ClaudeCodeExecutor {
    pub fn new() -> Self {
        Self { api_key: None }
    }

    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            api_key: Some(api_key.into()),
        }
    }

    fn check_cli_available() -> bool {
        std::process::Command::new("which")
            .arg("claude")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl Default for ClaudeCodeExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal session implementation for Claude Code.
struct ClaudeSessionImpl {
    process: Arc<Mutex<ClaudeProcess>>,
    cost: Arc<std::sync::Mutex<AgentCost>>,
    budget: f64,
}

#[async_trait]
impl SessionImpl for ClaudeSessionImpl {
    async fn query(&self, prompt: &str) -> Result<AgentResponse> {
        // Budget guard.
        {
            let c = self.cost.lock().unwrap();
            if c.total_usd >= self.budget {
                return Err(AgentError::BudgetExceeded {
                    limit: self.budget,
                    spent: c.total_usd,
                });
            }
        }

        let mut proc = self.process.lock().await;
        proc.send_query(prompt).await?;
        let resp = proc.read_response(self.budget).await?;

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
        let mut proc = self.process.lock().await;
        proc.shutdown().await
    }
}

#[async_trait]
impl AgentExecutor for ClaudeCodeExecutor {
    fn executor_type(&self) -> ExecutorType {
        ExecutorType::ClaudeCode
    }

    async fn spawn(
        &self,
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
    ) -> Result<AgentSession> {
        let session_id = Uuid::new_v4().to_string();
        let cost = Arc::new(std::sync::Mutex::new(AgentCost::default()));
        let budget = config.budget_usd.unwrap_or(f64::MAX);

        if budget <= 0.0 {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: 0.0,
            });
        }

        let mut proc = ClaudeProcess::start(
            working_dir,
            prompt,
            config,
            self.api_key.as_deref(),
        )
        .await?;

        let response = proc.read_response(budget).await?;

        {
            let mut c = cost.lock().unwrap();
            *c = response.cost.clone();
        }

        let inner = Arc::new(ClaudeSessionImpl {
            process: Arc::new(Mutex::new(proc)),
            cost: cost.clone(),
            budget,
        });

        Ok(AgentSession::new(
            session_id,
            ExecutorType::ClaudeCode,
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
        tracing::warn!(
            session_id = %session_id,
            "Claude Code resume: spawning new session (native resume not yet supported via CLI)"
        );
        self.spawn(working_dir, prompt, config).await
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            session_resume: false,
            token_usage: true,
            mcp_support: true,
            autonomous_mode: true,
            structured_output: false,
        }
    }

    fn availability(&self) -> AvailabilityStatus {
        if Self::check_cli_available() {
            AvailabilityStatus {
                available: true,
                reason: None,
            }
        } else {
            AvailabilityStatus {
                available: false,
                reason: Some(
                    "`claude` CLI not found. Install: npm install -g @anthropic-ai/claude-code"
                        .to_string(),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_type_is_claude_code() {
        let exec = ClaudeCodeExecutor::new();
        assert_eq!(exec.executor_type(), ExecutorType::ClaudeCode);
    }

    #[test]
    fn capabilities_declares_autonomous_mode() {
        let exec = ClaudeCodeExecutor::new();
        let caps = exec.capabilities();
        assert!(caps.autonomous_mode);
        assert!(caps.token_usage);
        assert!(caps.mcp_support);
        assert!(!caps.session_resume);
    }

    #[tokio::test]
    async fn budget_zero_returns_error_before_spawn() {
        let exec = ClaudeCodeExecutor::new();
        let result = exec
            .spawn(
                Path::new("/tmp"),
                "test",
                &SpawnConfig {
                    budget_usd: Some(0.0),
                    ..Default::default()
                },
            )
            .await;
        assert!(matches!(result, Err(AgentError::BudgetExceeded { .. })));
    }
}
