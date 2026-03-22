//! Claude Code subprocess management.
//!
//! Based on official Claude Code CLI documentation:
//! https://code.claude.com/docs/en/cli-reference
//!
//! Permission modes (official CLI values):
//!   - `default` — standard permission behavior
//!   - `acceptEdits` — auto-approve file edits, still prompt for bash
//!   - `bypassPermissions` — skip all permission checks
//!   - `plan` — analysis only, no edits/execution
//!   - `dontAsk` — deny instead of prompting (TypeScript SDK only)
//!
//! Session resume: `--resume <session_id>`
//! Budget: `--max-budget-usd <amount>` (print mode only)
//! Multi-turn: keep subprocess alive, write prompts to stdin

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use nucel_agent_core::{AgentCost, AgentError, PermissionMode, Result, SpawnConfig};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::protocol::{parse_message, parse_single_result, ClaudeMessage};

/// Default timeout for Claude Code queries (10 minutes).
const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Map our PermissionMode enum to official CLI flag values.
fn permission_mode_to_cli(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::BypassPermissions => "bypassPermissions",
        PermissionMode::RejectAll => "plan",
        PermissionMode::Prompt => "default",
    }
}

/// Manages a Claude Code CLI subprocess.
pub struct ClaudeProcess {
    child: Child,
    stdout_reader: BufReader<tokio::process::ChildStdout>,
    stderr_reader: Option<BufReader<tokio::process::ChildStderr>>,
    stdin_writer: Option<tokio::process::ChildStdin>,
}

impl ClaudeProcess {
    /// Build the base command with common flags.
    fn build_command(
        working_dir: &Path,
        config: &SpawnConfig,
        api_key: Option<&str>,
    ) -> Command {
        let mut cmd = Command::new("claude");
        cmd.current_dir(working_dir);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Model.
        if let Some(model) = &config.model {
            cmd.arg("--model").arg(model);
        }

        // Permission mode (official CLI flag).
        if let Some(mode) = &config.permission_mode {
            cmd.arg("--permission-mode").arg(permission_mode_to_cli(*mode));
        }

        // Budget enforcement (official CLI flag, print mode only).
        if let Some(budget) = config.budget_usd {
            if budget > 0.0 && budget < f64::MAX {
                cmd.arg("--max-budget-usd").arg(format!("{budget}"));
            }
        }

        // System prompt.
        if let Some(system) = &config.system_prompt {
            cmd.arg("--system-prompt").arg(system);
        }

        // Environment.
        if let Some(key) = api_key {
            cmd.env("ANTHROPIC_API_KEY", key);
        }
        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        cmd
    }

    /// Start a new Claude Code subprocess with streaming JSONL output.
    /// Used for the initial spawn — sends the first prompt via -p flag.
    pub async fn start(
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let mut cmd = Self::build_command(working_dir, config, api_key);

        // Print mode (non-interactive) + streaming JSON output.
        cmd.arg("-p").arg(prompt);
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose"); // Required for stream-json with -p.
        cmd.arg("--max-turns").arg("1");

        Self::spawn_child(cmd).await
    }

    /// Start in interactive multi-turn mode (subprocess stays alive).
    /// Prompts are sent to stdin, responses read from stdout.
    pub async fn start_interactive(
        working_dir: &Path,
        config: &SpawnConfig,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let mut cmd = Self::build_command(working_dir, config, api_key);

        // Streaming JSON output without -p (keeps stdin open for multi-turn).
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose");
        cmd.arg("--input-format").arg("stream-json");

        Self::spawn_child(cmd).await
    }

    /// Start in non-streaming mode (single JSON result).
    pub async fn start_oneshot(
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let mut cmd = Self::build_command(working_dir, config, api_key);

        // Print mode + non-streaming JSON output.
        cmd.arg("-p").arg(prompt);
        cmd.arg("--output-format").arg("json");
        cmd.arg("--max-turns").arg("1");

        Self::spawn_child(cmd).await
    }

    /// Spawn a new session that resumes an existing session.
    /// Uses the official `--resume <session_id>` CLI flag.
    pub async fn start_resume(
        working_dir: &Path,
        session_id: &str,
        prompt: &str,
        config: &SpawnConfig,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let mut cmd = Self::build_command(working_dir, config, api_key);

        // Resume mode — official CLI flag.
        cmd.arg("--resume").arg(session_id);
        cmd.arg("-p").arg(prompt);
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose");
        cmd.arg("--max-turns").arg("1");

        Self::spawn_child(cmd).await
    }

    /// Internal: spawn the child process and extract streams.
    async fn spawn_child(mut cmd: Command) -> Result<Self> {
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
        let stdin = child.stdin.take();

        Ok(Self {
            child,
            stdout_reader: BufReader::new(stdout),
            stderr_reader: stderr.map(BufReader::new),
            stdin_writer: stdin,
        })
    }

    /// Read streaming JSONL response with default timeout.
    pub async fn read_response(&mut self, budget: f64) -> Result<super::AgentResponse> {
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
                            tracing::warn!(expected = %session_id, got = %sid, "session_id mismatch");
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
                    Ok(ClaudeMessage::Other) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, line = %trimmed, "failed to parse Claude message");
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
                let _ = self.shutdown().await;
                return Err(AgentError::Timeout {
                    seconds: timeout.as_secs(),
                });
            }
        }

        // Budget check (client-side, in addition to CLI --max-budget-usd).
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

    /// Read non-streaming JSON response.
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

    /// Send a prompt for multi-turn mode (writes to stdin).
    /// The subprocess must be started with `start_interactive()`.
    pub async fn send_query(&mut self, prompt: &str) -> Result<()> {
        if let Some(ref mut stdin) = self.stdin_writer {
            let msg = serde_json::json!({
                "type": "human",
                "message": prompt,
            });
            let line = format!("{}\n", serde_json::to_string(&msg)?);
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(AgentError::Io)?;
            stdin.flush().await.map_err(AgentError::Io)?;
            Ok(())
        } else {
            Err(AgentError::Provider {
                provider: "claude-code".into(),
                message: "stdin not available — use start_interactive() for multi-turn".into(),
            })
        }
    }

    /// Gracefully shut down the subprocess.
    pub async fn shutdown(&mut self) -> Result<()> {
        // Drop stdin first to signal EOF.
        self.stdin_writer.take();

        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }

        match tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await {
            Ok(Ok(_status)) => Ok(()),
            Ok(Err(e)) => Err(AgentError::Io(e)),
            Err(_) => {
                let _ = self.child.kill().await;
                Ok(())
            }
        }
    }
}
