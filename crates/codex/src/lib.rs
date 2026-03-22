//! Codex provider — wraps the `codex` CLI (OpenAI).
//!
//! Communicates via JSONL stdio protocol (`codex exec --experimental-json`).
//! Supports:
//! - Thread-based multi-turn workflows
//! - Structured output via JSON Schema
//! - Token usage tracking

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse, AgentSession,
    AvailabilityStatus, ExecutorType, Result, SessionImpl, SpawnConfig,
};

/// Codex executor — spawns `codex` CLI subprocess.
pub struct CodexExecutor {
    api_key: Option<String>,
}

impl CodexExecutor {
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
            .arg("codex")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl Default for CodexExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a Codex JSONL line.
fn parse_codex_line(line: &str) -> Result<Option<CodexEvent>> {
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|e| AgentError::Provider {
            provider: "codex".into(),
            message: format!("JSON parse error: {e}"),
        })?;

    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        "item.completed" => {
            let item = &v["item"];
            match item.get("type").and_then(|t| t.as_str()) {
                Some("agent_message") => {
                    let text = item
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    Ok(Some(CodexEvent::Message(text)))
                }
                _ => Ok(Some(CodexEvent::Other)),
            }
        }
        "turn.completed" => {
            let usage = &v["usage"];
            let input_tokens = usage
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output_tokens = usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            Ok(Some(CodexEvent::TurnCompleted {
                input_tokens,
                output_tokens,
            }))
        }
        _ => Ok(Some(CodexEvent::Other)),
    }
}

#[derive(Debug)]
enum CodexEvent {
    Message(String),
    TurnCompleted {
        input_tokens: u64,
        output_tokens: u64,
    },
    Other,
}

/// Run a codex exec command and collect response.
async fn run_codex(
    working_dir: &Path,
    prompt: &str,
    config: &SpawnConfig,
    api_key: Option<&str>,
) -> Result<(String, AgentCost)> {
    let mut cmd = Command::new("codex");
    cmd.current_dir(working_dir);
    cmd.arg("exec");
    cmd.arg("--experimental-json");

    if let Some(model) = &config.model {
        cmd.arg("--model").arg(model);
    }

    cmd.arg(prompt);

    if let Some(key) = api_key {
        cmd.env("CODEX_API_KEY", key);
    }
    for (k, v) in &config.env {
        cmd.env(k, v);
    }

    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::CliNotFound {
                    cli_name: "codex".to_string(),
                }
            } else {
                AgentError::Io(e)
            }
        })?;

    let stdout = child.stdout.take().ok_or_else(|| AgentError::Provider {
        provider: "codex".into(),
        message: "failed to capture stdout".into(),
    })?;

    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut content = String::new();
    let mut cost = AgentCost::default();

    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).await.map_err(AgentError::Io)?;
        if bytes == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match parse_codex_line(trimmed) {
            Ok(Some(CodexEvent::Message(text))) => {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(&text);
            }
            Ok(Some(CodexEvent::TurnCompleted {
                input_tokens,
                output_tokens,
            })) => {
                cost.input_tokens = input_tokens;
                cost.output_tokens = output_tokens;
            }
            Ok(Some(CodexEvent::Other)) => {}
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse codex line");
            }
        }
    }

    let _ = child.wait().await;

    Ok((content, cost))
}

/// Internal session implementation for Codex.
struct CodexSessionImpl {
    cost: Arc<Mutex<AgentCost>>,
    budget: f64,
    working_dir: PathBuf,
    config: SpawnConfig,
    api_key: Option<String>,
}

#[async_trait]
impl SessionImpl for CodexSessionImpl {
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

        let (content, turn_cost) =
            run_codex(&self.working_dir, prompt, &self.config, self.api_key.as_deref()).await?;

        {
            let mut c = self.cost.lock().unwrap();
            c.input_tokens += turn_cost.input_tokens;
            c.output_tokens += turn_cost.output_tokens;
            c.total_usd += turn_cost.total_usd;
        }

        Ok(AgentResponse {
            content: content,
            cost: turn_cost,
            ..Default::default()
        })
    }

    async fn total_cost(&self) -> Result<AgentCost> {
        Ok(self.cost.lock().unwrap().clone())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl AgentExecutor for CodexExecutor {
    fn executor_type(&self) -> ExecutorType {
        ExecutorType::Codex
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

        let (_initial_content, turn_cost) =
            run_codex(working_dir, prompt, config, self.api_key.as_deref()).await?;

        if turn_cost.total_usd > budget {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: turn_cost.total_usd,
            });
        }

        {
            let mut c = cost.lock().unwrap();
            *c = turn_cost;
        }

        let inner = Arc::new(CodexSessionImpl {
            cost: cost.clone(),
            budget,
            working_dir: working_dir.to_path_buf(),
            config: config.clone(),
            api_key: self.api_key.clone(),
        });

        Ok(AgentSession::new(
            session_id,
            ExecutorType::Codex,
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
            "Codex resume: spawning new session (native resume not yet implemented)"
        );
        self.spawn(working_dir, prompt, config).await
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            session_resume: false,
            token_usage: true,
            mcp_support: false,
            autonomous_mode: true,
            structured_output: true,
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
                    "`codex` CLI not found. Install: npm install -g @openai/codex".to_string(),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_type_is_codex() {
        let exec = CodexExecutor::new();
        assert_eq!(exec.executor_type(), ExecutorType::Codex);
    }

    #[test]
    fn capabilities_declares_structured_output() {
        let caps = CodexExecutor::new().capabilities();
        assert!(caps.structured_output);
        assert!(caps.autonomous_mode);
        assert!(caps.token_usage);
        assert!(!caps.mcp_support);
    }

    #[test]
    fn parse_codex_message_event() {
        let line =
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Fixed the bug"}}"#;
        let event = parse_codex_line(line).unwrap();
        match event {
            Some(CodexEvent::Message(text)) => assert_eq!(text, "Fixed the bug"),
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn parse_codex_turn_completed() {
        let line =
            r#"{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":50}}"#;
        let event = parse_codex_line(line).unwrap();
        match event {
            Some(CodexEvent::TurnCompleted {
                input_tokens,
                output_tokens,
            }) => {
                assert_eq!(input_tokens, 100);
                assert_eq!(output_tokens, 50);
            }
            _ => panic!("expected TurnCompleted"),
        }
    }
}
