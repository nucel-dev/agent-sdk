//! Claude Code provider — wraps the `claude` CLI as a subprocess.
//!
//! Spawns `claude --output-format stream-json --input-format stream-json
//! --verbose` (interactive mode, no `-p`) and speaks JSONL over stdio. Each
//! prompt — the first and every follow-up — is written to the child's stdin,
//! so the subprocess stays alive for the whole session. Supports:
//!
//! - Multi-turn queries on one live session (`spawn` then repeated `query`)
//! - Cross-process resume via `--resume <session_id>` ([`AgentExecutor::resume`])
//! - Cost tracking per session, including cache-read / cache-creation tokens
//! - Permission mode configuration ([`PermissionMode`] → `--permission-mode`)
//! - Budget enforcement (`budget_usd` → cancel on overrun)
//!
//! [`PermissionMode`]: nucel_agent_core::PermissionMode
//!
//! # Minimal example
//!
//! ```rust,no_run
//! use nucel_agent_claude_code::ClaudeCodeExecutor;
//! use nucel_agent_core::{AgentExecutor, SpawnConfig};
//! use std::path::Path;
//!
//! # async fn run() -> nucel_agent_core::Result<()> {
//! let executor = ClaudeCodeExecutor::new();
//! let session = executor.spawn(
//!     Path::new("/my/repo"),
//!     "Fix the failing test in src/lib.rs",
//!     &SpawnConfig {
//!         model: Some("claude-opus-4-6".into()),
//!         budget_usd: Some(5.0),
//!         max_turns: Some(10),
//!         ..Default::default()
//!     },
//! ).await?;
//!
//! let resp = session.query("Did CI pass?").await?;
//! println!("{}", resp.content);
//! session.close().await?;
//! # Ok(()) }
//! ```
//!
//! See also: [workspace README](https://github.com/nucel-dev/agent-sdk#readme)
//! and the runnable example `crates/unified/examples/claude_basic.rs`.

#![cfg_attr(docsrs, feature(doc_cfg))]

mod process;
mod protocol;

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse, AgentSession,
    AvailabilityStatus, EventStream, ExecutorType, MessageEvent, Result, SessionImpl, SpawnConfig,
};
use std::time::Duration;

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

    async fn query_stream(&self, prompt: &str) -> Result<EventStream> {
        // Budget guard up-front; the stream itself also checks.
        {
            let c = self.cost.lock().unwrap();
            if c.total_usd >= self.budget {
                return Err(AgentError::BudgetExceeded {
                    limit: self.budget,
                    spent: c.total_usd,
                });
            }
        }

        let process = self.process.clone();
        let cost_handle = self.cost.clone();
        let budget = self.budget;
        let prompt_owned = prompt.to_string();

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<MessageEvent>>(64);

        tokio::spawn(async move {
            let mut proc = process.lock().await;
            if let Err(e) = proc.send_query(&prompt_owned).await {
                let _ = tx.send(Err(e)).await;
                return;
            }
            let stderr_buf = proc.stderr_buf.clone();

            let timeout = Duration::from_secs(600);
            let mut input_tokens = 0_u64;
            let mut output_tokens = 0_u64;
            let mut cache_read = 0_u64;
            let mut cache_creation = 0_u64;
            let mut total_cost_usd = 0.0_f64;
            let mut saw_terminal = false;

            let res = tokio::time::timeout(timeout, async {
                use tokio::io::AsyncBufReadExt;
                let mut line = String::new();
                loop {
                    line.clear();
                    let n = match proc.stdout_reader.read_line(&mut line).await {
                        Ok(n) => n,
                        Err(e) => {
                            let _ = tx.send(Err(AgentError::Io(e))).await;
                            return;
                        }
                    };
                    if n == 0 { break; }
                    let trimmed = line.trim();
                    if trimmed.is_empty() { continue; }

                    let v: serde_json::Value = match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match msg_type {
                        "assistant" => {
                            let blocks = v["message"]["content"].as_array().cloned().unwrap_or_default();
                            for block in &blocks {
                                let bt = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                match bt {
                                    "text" => {
                                        if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                            let _ = tx.send(Ok(MessageEvent::TextChunk { text: t.to_string() })).await;
                                        }
                                    }
                                    "tool_use" => {
                                        let id = block.get("id").and_then(|s| s.as_str()).unwrap_or("").to_string();
                                        let name = block.get("name").and_then(|s| s.as_str()).unwrap_or("").to_string();
                                        let input = block.get("input").cloned().unwrap_or(serde_json::Value::Null);
                                        let _ = tx.send(Ok(MessageEvent::ToolUse { id, name, input })).await;
                                    }
                                    "thinking" => {
                                        let text = block.get("thinking").and_then(|t| t.as_str()).unwrap_or("").to_string();
                                        let _ = tx.send(Ok(MessageEvent::Thinking { text })).await;
                                    }
                                    _ => {}
                                }
                            }
                            if let Some(u) = v["message"].get("usage") {
                                input_tokens += u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                                output_tokens += u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                                cache_read += u.get("cache_read_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                                cache_creation += u.get("cache_creation_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                            }
                        }
                        "user" => {
                            let blocks = v["message"]["content"].as_array().cloned().unwrap_or_default();
                            for block in &blocks {
                                if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                                    let id = block.get("tool_use_id").and_then(|s| s.as_str()).unwrap_or("").to_string();
                                    let is_error = block.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                                    let output = block.get("content").and_then(|c| c.as_str()).map(String::from)
                                        .or_else(|| block.get("content").map(|c| c.to_string()))
                                        .unwrap_or_default();
                                    let _ = tx.send(Ok(MessageEvent::ToolResult { tool_use_id: id, success: !is_error, output })).await;
                                }
                            }
                        }
                        "rate_limit_event" => {
                            let _ = tx.send(Ok(MessageEvent::RateLimit { message: "rate limit event".into() })).await;
                        }
                        "result" => {
                            let result_text = v.get("result").and_then(|r| r.as_str()).unwrap_or("").to_string();
                            let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                            total_cost_usd = v.get("total_cost_usd").and_then(|c| c.as_f64()).unwrap_or(total_cost_usd);
                            if let Some(u) = v.get("usage") {
                                input_tokens = u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(input_tokens);
                                output_tokens = u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(output_tokens);
                                let crd = u.get("cache_read_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                                let ccr = u.get("cache_creation_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                                if crd > 0 { cache_read = crd; }
                                if ccr > 0 { cache_creation = ccr; }
                            }
                            let cost = AgentCost {
                                input_tokens,
                                output_tokens,
                                cache_read_tokens: cache_read,
                                cache_creation_tokens: cache_creation,
                                total_usd: total_cost_usd,
                            };
                            {
                                let mut c = cost_handle.lock().unwrap();
                                c.input_tokens += cost.input_tokens;
                                c.output_tokens += cost.output_tokens;
                                c.cache_read_tokens += cost.cache_read_tokens;
                                c.cache_creation_tokens += cost.cache_creation_tokens;
                                c.total_usd += cost.total_usd;
                            }
                            if total_cost_usd > budget {
                                let _ = tx.send(Err(AgentError::BudgetExceeded { limit: budget, spent: total_cost_usd })).await;
                                saw_terminal = true;
                                return;
                            }
                            let _ = tx.send(Ok(MessageEvent::ResultDone { cost, content: result_text, is_error })).await;
                            saw_terminal = true;
                            return;
                        }
                        _ => {}
                    }
                }
            }).await;

            if res.is_err() {
                let tail = stderr_buf.lock().await.clone();
                let msg = if tail.is_empty() {
                    "stream timed out".to_string()
                } else {
                    format!("stream timed out (stderr: {})", tail.trim())
                };
                let _ = tx.send(Err(AgentError::Provider { provider: "claude-code".into(), message: msg })).await;
            } else if !saw_terminal {
                let _ = tx.send(Err(AgentError::StreamInterrupted("claude stream ended without result".into()))).await;
            }
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Box::pin(stream))
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
        let cost = Arc::new(std::sync::Mutex::new(AgentCost::default()));
        let budget = config.budget_usd.unwrap_or(f64::MAX);

        if budget <= 0.0 {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: 0.0,
            });
        }

        // Start in interactive multi-turn mode so the subprocess stays alive
        // and subsequent `query()` / `query_stream()` calls — which write the
        // next prompt to stdin via `send_query` — actually reach a live CLI.
        // (Print mode `-p` exits after the first turn, closing stdin.)
        let mut proc = ClaudeProcess::start_interactive(
            working_dir,
            config,
            self.api_key.as_deref(),
        )
        .await?;

        // Capture the pre-minted session id before the read may consume `proc`.
        let session_id = proc.session_id().to_string();

        // Send the first prompt over stdin, then read its streamed response.
        proc.send_query(prompt).await?;
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
        let cost = Arc::new(std::sync::Mutex::new(AgentCost::default()));
        let budget = config.budget_usd.unwrap_or(f64::MAX);

        if budget <= 0.0 {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: 0.0,
            });
        }

        // Use official --resume <session_id> CLI flag. The resume keeps the
        // original session id so consumers can keep resuming.
        let mut proc = ClaudeProcess::start_resume(
            working_dir,
            session_id,
            prompt,
            config,
            self.api_key.as_deref(),
        )
        .await?;

        let resumed_session_id = proc.session_id().to_string();
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
            resumed_session_id,
            ExecutorType::ClaudeCode,
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
            streaming: true,
            hooks: true,
            prompt_caching: true,
            extended_thinking: true,
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
        assert!(caps.session_resume, "Claude Code supports --resume flag");
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
