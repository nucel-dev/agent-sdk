//! Claude Code subprocess management.

use std::path::Path;
use std::process::Stdio;

use nucel_agent_core::{AgentError, PermissionMode, Result, SpawnConfig};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::protocol::{ClaudeMessage, parse_message};

/// Manages a Claude Code CLI subprocess.
pub struct ClaudeProcess {
    child: Child,
    stdout_reader: BufReader<tokio::process::ChildStdout>,
}

impl ClaudeProcess {
    /// Start a new Claude Code subprocess.
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

        // JSON output mode for machine parsing.
        cmd.arg("--output-format").arg("json");

        // Model.
        if let Some(model) = &config.model {
            cmd.arg("--model").arg(model);
        }

        // Max turns.
        cmd.arg("--max-turns").arg("1");

        // Permission mode.
        match config.permission_mode {
            Some(PermissionMode::BypassPermissions) | Some(PermissionMode::AcceptEdits) => {
                cmd.arg("--dangerously-skip-permissions");
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

        let mut child = cmd
            .spawn()
            .map_err(|e| {
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

        let stdout_reader = BufReader::new(stdout);

        Ok(Self {
            child,
            stdout_reader,
        })
    }

    /// Send a follow-up query (for multi-turn, if supported).
    pub async fn send_query(&mut self, _prompt: &str) -> Result<()> {
        // Note: The --output-format json + -p mode is typically one-shot.
        // For multi-turn, we'd need to use the interactive mode or SDK.
        // This is a simplified implementation.
        tracing::debug!("send_query called (one-shot mode limitation)");
        Ok(())
    }

    /// Read the JSON response from stdout.
    pub async fn read_response(&mut self, budget: f64) -> Result<super::AgentResponse> {
        let mut line = String::new();
        let mut content = String::new();
        let mut total_cost_usd = 0.0_f64;
        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;

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
                Ok(ClaudeMessage::Assistant { text, cost, usage }) => {
                    if !text.is_empty() {
                        if !content.is_empty() {
                            content.push('\n');
                        }
                        content.push_str(&text);
                    }
                    if let Some(c) = cost {
                        total_cost_usd += c;
                    }
                    if let Some(u) = usage {
                        input_tokens += u.input_tokens;
                        output_tokens += u.output_tokens;
                    }
                }
                Ok(ClaudeMessage::Result {
                    text,
                    cost,
                    is_error,
                }) => {
                    if let Some(t) = text {
                        if !content.is_empty() {
                            content.push('\n');
                        }
                        content.push_str(&t);
                    }
                    if let Some(c) = cost {
                        total_cost_usd = c;
                    }
                    if is_error {
                        return Err(AgentError::Provider {
                            provider: "claude-code".into(),
                            message: "agent completed with error".into(),
                        });
                    }
                    break;
                }
                Ok(ClaudeMessage::Other) => {
                    // Skip unrecognized messages (system, stream events, etc.)
                }
                Err(e) => {
                    tracing::warn!(error = %e, line = %trimmed, "failed to parse Claude message");
                }
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
            cost: nucel_agent_core::AgentCost {
                input_tokens,
                output_tokens,
                total_usd: total_cost_usd,
            },
            confidence: None,
            requests_escalation: false,
            tool_calls: vec![],
        })
    }

    /// Gracefully shut down the subprocess.
    pub async fn shutdown(&mut self) -> Result<()> {
        // Try SIGTERM first.
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }

        // Wait up to 5 seconds.
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.child.wait(),
        )
        .await
        {
            Ok(Ok(_status)) => Ok(()),
            Ok(Err(e)) => Err(AgentError::Io(e)),
            Err(_) => {
                // Force kill.
                let _ = self.child.kill().await;
                Ok(())
            }
        }
    }
}
