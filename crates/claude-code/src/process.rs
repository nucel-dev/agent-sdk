//! Claude Code subprocess management.
//!
//! Supports two modes:
//! - `stream-json`: Streaming JSONL output (system → assistant → result)
//! - `json`: Single JSON result at the end

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use nucel_agent_core::{AgentCost, AgentError, PermissionMode, Result, SpawnConfig};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::protocol::{parse_message, parse_single_result, ClaudeMessage};

/// Default timeout for Claude Code queries (10 minutes).
const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Manages a Claude Code CLI subprocess.
pub struct ClaudeProcess {
    child: Child,
    stdout_reader: BufReader<tokio::process::ChildStdout>,
    stderr_reader: Option<BufReader<tokio::process::ChildStderr>>,
}

impl ClaudeProcess {
    /// Start a new Claude Code subprocess with streaming output.
    pub async fn start(
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let mut cmd = Command::new("claude");

        cmd.current_dir(working_dir);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Streaming JSON output mode.
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose");

        // Model.
        if let Some(model) = &config.model {
            cmd.arg("--model").arg(model);
        }

        // Max turns.
        cmd.arg("--max-turns").arg("1");

        // Permission mode.
        match config.permission_mode {
            Some(PermissionMode::BypassPermissions) => {
                cmd.arg("--dangerously-skip-permissions");
            }
            Some(PermissionMode::AcceptEdits) => {
                // AcceptEdits: allow file edits but not arbitrary commands.
                // Use allowedTools to restrict to safe file operations.
                cmd.arg("--allowedTools")
                    .arg("Edit,Write,Read,Glob,Grep,NotebookEdit");
            }
            Some(PermissionMode::RejectAll) => {
                cmd.arg("--print");
            }
            _ => {}
        }

        // System prompt.
        if let Some(system) = &config.system_prompt {
            cmd.arg("--system-prompt").arg(system);
        }

        // The prompt itself.
        cmd.arg("-p").arg(prompt);

        // Environment.
        if let Some(key) = api_key {
            cmd.env("ANTHROPIC_API_KEY", key);
        }
        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::CliNotFound {
                    cli_name: "claude".to_string(),
                }
            } else {
                AgentError::Io(e)
            }
        })?;

        let stdout = child.stdout.take().ok_or_else(|| AgentError::Provider {
            provider: "claude-code".into(),
            message: "failed to capture stdout".into(),
        })?;

        let stderr = child.stderr.take();
        let stdout_reader = BufReader::new(stdout);
        let stderr_reader = stderr.map(BufReader::new);

        Ok(Self {
            child,
            stdout_reader,
            stderr_reader,
        })
    }

    /// Start in non-streaming mode (single JSON result).
    pub async fn start_oneshot(
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let mut cmd = Command::new("claude");

        cmd.current_dir(working_dir);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Non-streaming JSON output mode.
        cmd.arg("--output-format").arg("json");

        if let Some(model) = &config.model {
            cmd.arg("--model").arg(model);
        }

        cmd.arg("--max-turns").arg("1");

        match config.permission_mode {
            Some(PermissionMode::BypassPermissions) => {
                cmd.arg("--dangerously-skip-permissions");
            }
            Some(PermissionMode::AcceptEdits) => {
                cmd.arg("--allowedTools")
                    .arg("Edit,Write,Read,Glob,Grep,NotebookEdit");
            }
            _ => {}
        }

        if let Some(system) = &config.system_prompt {
            cmd.arg("--system-prompt").arg(system);
        }

        cmd.arg("-p").arg(prompt);

        if let Some(key) = api_key {
            cmd.env("ANTHROPIC_API_KEY", key);
        }
        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::CliNotFound {
                    cli_name: "claude".to_string(),
                }
            } else {
                AgentError::Io(e)
            }
        })?;

        let stdout = child.stdout.take().ok_or_else(|| AgentError::Provider {
            provider: "claude-code".into(),
            message: "failed to capture stdout".into(),
        })?;

        let stderr = child.stderr.take();
        let stdout_reader = BufReader::new(stdout);
        let stderr_reader = stderr.map(BufReader::new);

        Ok(Self {
            child,
            stdout_reader,
            stderr_reader,
        })
    }

    /// Read streaming JSONL response with timeout.
    pub async fn read_response(
        &mut self,
        budget: f64,
    ) -> Result<super::AgentResponse> {
        let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
        self.read_response_with_timeout(budget, timeout).await
    }

    /// Read streaming JSONL response with configurable timeout.
    pub async fn read_response_with_timeout(
        &mut self,
        budget: f64,
        timeout: Duration,
    ) -> Result<super::AgentResponse> {
        let mut line = String::new();
        let mut content = String::new();
        let mut total_cost_usd = 0.0_f64;
        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        let mut session_id = String::new();
        let mut system_model = String::new();

        let result = tokio::time::timeout(timeout, async {
            loop {
                line.clear();
                let bytes_read = self
                    .stdout_reader
                    .read_line(&mut line)
                    .await
                    .map_err(AgentError::Io)?;

                if bytes_read == 0 {
                    break; // EOF
                }

                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                match parse_message(trimmed) {
                    Ok(ClaudeMessage::SystemInit {
                        session_id: sid,
                        model,
                        ..
                    }) => {
                        session_id = sid;
                        system_model = model;
                        tracing::debug!(session_id = %session_id, model = %system_model, "claude session started");
                    }
                    Ok(ClaudeMessage::Assistant {
                        text,
                        usage,
                        session_id: sid,
                    }) => {
                        if !session_id.is_empty() && sid != session_id {
                            // Session mismatch — log but continue.
                            tracing::warn!(expected = %session_id, got = %sid, "session_id mismatch in assistant message");
                        }
                        if !text.is_empty() {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(&text);
                        }
                        if let Some(u) = usage {
                            input_tokens += u.input_tokens;
                            output_tokens += u.output_tokens;
                        }
                    }
                    Ok(ClaudeMessage::RateLimit { .. }) => {
                        tracing::info!("rate limit event received");
                    }
                    Ok(ClaudeMessage::Result {
                        text,
                        is_error,
                        cost,
                        session_id: _,
                        duration_ms,
                        num_turns,
                    }) => {
                        if !text.is_empty() && !content.contains(&text) {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(&text);
                        }
                        // Use the final result's cost as authoritative.
                        total_cost_usd = cost.total_usd;
                        input_tokens = cost.input_tokens;
                        output_tokens = cost.output_tokens;

                        tracing::info!(
                            duration_ms = duration_ms,
                            num_turns = num_turns,
                            cost_usd = total_cost_usd,
                            "claude session completed"
                        );

                        if is_error {
                            return Err(AgentError::Provider {
                                provider: "claude-code".into(),
                                message: format!("agent returned error: {text}"),
                            });
                        }
                        break;
                    }
                    Ok(ClaudeMessage::Other) => {
                        // Skip unrecognized messages (tool_use, thinking, etc.)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, line = %trimmed, "failed to parse Claude message line");
                    }
                }
            }
            Ok::<(), AgentError>(())
        })
        .await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                // Timeout — try to kill the process.
                let _ = self.shutdown().await;
                return Err(AgentError::Timeout {
                    seconds: timeout.as_secs(),
                });
            }
        }

        if total_cost_usd > budget {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: total_cost_usd,
            });
        }

        Ok(super::AgentResponse {
            content,
            cost: AgentCost {
                input_tokens,
                output_tokens,
                total_usd: total_cost_usd,
            },
            confidence: None,
            requests_escalation: false,
            tool_calls: vec![],
        })
    }

    /// Read non-streaming JSON response with timeout.
    pub async fn read_oneshot_response(
        &mut self,
        budget: f64,
    ) -> Result<super::AgentResponse> {
        let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);

        let result = tokio::time::timeout(timeout, async {
            let mut buf = String::new();
            self.stdout_reader
                .read_to_string(&mut buf)
                .await
                .map_err(AgentError::Io)?;

            // Also capture stderr if available.
            if let Some(ref mut stderr_reader) = self.stderr_reader {
                let mut stderr_buf = String::new();
                let _ = stderr_reader.read_to_string(&mut stderr_buf).await;
                if !stderr_buf.is_empty() {
                    tracing::debug!(stderr = %stderr_buf, "claude stderr output");
                }
            }

            parse_single_result(&buf)
        })
        .await;

        match result {
            Ok(resp) => {
                let resp = resp?;
                if resp.cost.total_usd > budget {
                    return Err(AgentError::BudgetExceeded {
                        limit: budget,
                        spent: resp.cost.total_usd,
                    });
                }
                Ok(resp)
            }
            Err(_) => {
                let _ = self.shutdown().await;
                Err(AgentError::Timeout {
                    seconds: timeout.as_secs(),
                })
            }
        }
    }

    /// Send a follow-up query (for multi-turn sessions).
    ///
    /// Currently unsupported — Claude CLI runs in one-shot mode.
    /// Returns an error to prevent silent failures.
    pub async fn send_query(&mut self, _prompt: &str) -> Result<()> {
        Err(AgentError::Provider {
            provider: "claude-code".into(),
            message: "multi-turn queries not supported in CLI subprocess mode".into(),
        })
    }

    /// Gracefully shut down the subprocess.
    pub async fn shutdown(&mut self) -> Result<()> {
        // Request stop via tokio's safe, cross-platform API.
        let _ = self.child.start_kill();

        // Wait up to 5 seconds for exit.
        match tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await {
            Ok(Ok(_status)) => Ok(()),
            Ok(Err(e)) => Err(AgentError::Io(e)),
            Err(_) => {
                // Force kill if still alive.
                let _ = self.child.kill().await;
                Ok(())
            }
        }
    }
}
