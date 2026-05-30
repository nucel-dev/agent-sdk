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
//! Session resume: `--resume <session_id>` or `--session-id <uuid>` to pre-mint.
//! Budget: `--max-budget-usd <amount>` (print mode only)
//! Multi-turn: keep subprocess alive, write prompts to stdin

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use nucel_agent_core::{AgentCost, AgentError, HookConfig, PermissionMode, Result, SpawnConfig};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex as AsyncMutex;

use crate::protocol::{parse_message, parse_single_result, ClaudeMessage};

/// Default timeout for Claude Code queries (10 minutes).
const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Maximum bytes of stderr to keep in the rolling buffer for error context.
const STDERR_BUFFER_BYTES: usize = 4096;

/// Map our PermissionMode enum to official CLI flag values.
pub(crate) fn permission_mode_to_cli(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::BypassPermissions => "bypassPermissions",
        // Legacy alias — `plan` mode allows reads. Use DontAsk for a true deny.
        PermissionMode::RejectAll => "plan",
        PermissionMode::DontAsk => "dontAsk",
        // `Auto` lets the provider pick its default → CLI `default`.
        PermissionMode::Auto | PermissionMode::Prompt => "default",
        // Forward-compat: any future variant falls back to CLI `default`.
        _ => "default",
    }
}

/// Manages a Claude Code CLI subprocess.
pub struct ClaudeProcess {
    pub(crate) child: Child,
    pub(crate) stdout_reader: BufReader<tokio::process::ChildStdout>,
    /// Shared rolling stderr buffer. Populated by a background drainer task.
    pub(crate) stderr_buf: Arc<AsyncMutex<String>>,
    stdin_writer: Option<tokio::process::ChildStdin>,
    /// The pre-minted session id passed via `--session-id`. Returned to caller
    /// for round-trip resume.
    pub(crate) session_id: String,
}

impl ClaudeProcess {
    /// Build the base command with common flags.
    fn build_command(
        working_dir: &Path,
        config: &SpawnConfig,
        api_key: Option<&str>,
        session_id: &str,
    ) -> Command {
        let mut cmd = Command::new("claude");
        cmd.current_dir(working_dir);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Pre-minted session id so the SDK can round-trip resume.
        cmd.arg("--session-id").arg(session_id);

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

        // Reasoning effort.
        if let Some(effort) = &config.reasoning {
            cmd.arg("--effort").arg(effort);
        }

        // Extended-thinking budget (Claude only).
        if let Some(budget) = config.thinking_budget {
            cmd.arg("--thinking-budget-tokens").arg(budget.to_string());
        }

        // Hooks -> --settings <json>.
        if let Some(hook_cfg) = &config.hook_config {
            let settings = hook_config_to_settings_json(hook_cfg);
            if let Ok(serialized) = serde_json::to_string(&settings) {
                cmd.arg("--settings").arg(serialized);
            }
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

    /// Append `--max-turns <n>` if configured. If `None`, omit the flag so the
    /// CLI default (no per-call cap) applies.
    fn apply_max_turns(cmd: &mut Command, config: &SpawnConfig) {
        if let Some(n) = config.max_turns {
            cmd.arg("--max-turns").arg(n.to_string());
        }
    }

    /// Start a new Claude Code subprocess with streaming JSONL output.
    /// Used for the initial spawn — sends the first prompt via -p flag.
    pub async fn start(
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let mut cmd = Self::build_command(working_dir, config, api_key, &session_id);

        // Print mode (non-interactive) + streaming JSON output.
        cmd.arg("-p").arg(prompt);
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose"); // Required for stream-json with -p.
        Self::apply_max_turns(&mut cmd, config);

        Self::spawn_child(cmd, session_id).await
    }

    /// Start in interactive multi-turn mode (subprocess stays alive).
    /// Prompts are sent to stdin, responses read from stdout.
    pub async fn start_interactive(
        working_dir: &Path,
        config: &SpawnConfig,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let mut cmd = Self::build_command(working_dir, config, api_key, &session_id);

        // Streaming JSON output without -p (keeps stdin open for multi-turn).
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose");
        cmd.arg("--input-format").arg("stream-json");
        Self::apply_max_turns(&mut cmd, config);

        Self::spawn_child(cmd, session_id).await
    }

    /// Start in non-streaming mode (single JSON result).
    pub async fn start_oneshot(
        working_dir: &Path,
        prompt: &str,
        config: &SpawnConfig,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let mut cmd = Self::build_command(working_dir, config, api_key, &session_id);

        // Print mode + non-streaming JSON output.
        cmd.arg("-p").arg(prompt);
        cmd.arg("--output-format").arg("json");
        Self::apply_max_turns(&mut cmd, config);

        Self::spawn_child(cmd, session_id).await
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
        // For resume, reuse the existing session id (don't pre-mint a fresh one).
        let mut cmd = Self::build_command(working_dir, config, api_key, session_id);

        // Resume mode — official CLI flag.
        cmd.arg("--resume").arg(session_id);
        cmd.arg("-p").arg(prompt);
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose");
        Self::apply_max_turns(&mut cmd, config);

        Self::spawn_child(cmd, session_id.to_string()).await
    }

    /// Internal: spawn the child process and extract streams.
    async fn spawn_child(mut cmd: Command, session_id: String) -> Result<Self> {
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

        let stderr_buf = Arc::new(AsyncMutex::new(String::new()));
        if let Some(err) = stderr {
            let buf = stderr_buf.clone();
            tokio::spawn(drain_stderr(err, buf));
        }

        Ok(Self {
            child,
            stdout_reader: BufReader::new(stdout),
            stderr_buf,
            stdin_writer: stdin,
            session_id,
        })
    }

    /// Returns the pre-minted session id used for `--session-id`.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Best-effort snapshot of the last few KiB of stderr.
    pub async fn stderr_snapshot(&self) -> String {
        self.stderr_buf.lock().await.clone()
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
        // The upstream-reported session id from system/init. Used to detect
        // mismatches against our pre-minted id (logged at warn).
        let mut upstream_session_id = String::new();
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
                        upstream_session_id = sid;
                        system_model = model;
                        if !upstream_session_id.is_empty()
                            && upstream_session_id != self.session_id
                        {
                            tracing::warn!(
                                preminted = %self.session_id,
                                upstream = %upstream_session_id,
                                "claude reported a different session_id than the one we requested"
                            );
                        }
                        tracing::debug!(
                            session_id = %self.session_id,
                            model = %system_model,
                            "claude session started"
                        );
                    }
                    Ok(ClaudeMessage::Assistant {
                        text,
                        usage,
                        session_id: sid,
                    }) => {
                        if !upstream_session_id.is_empty() && sid != upstream_session_id {
                            tracing::warn!(expected = %upstream_session_id, got = %sid, "session_id mismatch");
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
                            let stderr_tail = self.stderr_snapshot().await;
                            return Err(AgentError::Provider {
                                provider: "claude-code".into(),
                                message: format!(
                                    "agent returned error: {text}{}",
                                    fmt_stderr_tail(&stderr_tail)
                                ),
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
            Ok(Err(mut e)) => {
                // Augment provider errors with stderr tail.
                if let AgentError::Provider { message, .. } = &mut e {
                    let tail = self.stderr_snapshot().await;
                    if !tail.is_empty() && !message.contains("stderr:") {
                        message.push_str(&fmt_stderr_tail(&tail));
                    }
                }
                return Err(e);
            }
            Err(_) => {
                let stderr_tail = self.stderr_snapshot().await;
                let _ = self.shutdown().await;
                return Err(AgentError::Provider {
                    provider: "claude-code".into(),
                    message: format!(
                        "timed out after {}s{}",
                        timeout.as_secs(),
                        fmt_stderr_tail(&stderr_tail)
                    ),
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
            cost: AgentCost { input_tokens, output_tokens, cache_read_tokens: 0, cache_creation_tokens: 0, total_usd: total_cost_usd },
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
                let stderr_tail = self.stderr_snapshot().await;
                let _ = self.shutdown().await;
                Err(AgentError::Provider {
                    provider: "claude-code".into(),
                    message: format!(
                        "timed out after {}s{}",
                        timeout.as_secs(),
                        fmt_stderr_tail(&stderr_tail)
                    ),
                })
            }
        }
    }

    /// Send a prompt for multi-turn mode (writes to stdin).
    /// The subprocess must be started with `start_interactive()`.
    pub async fn send_query(&mut self, prompt: &str) -> Result<()> {
        if let Some(ref mut stdin) = self.stdin_writer {
            // Documented stream-input shape:
            //   {"type":"user","message":{"role":"user","content":[{"type":"text","text":"…"}]},"session_id":"…"}
            let msg = serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": prompt}
                    ]
                },
                "session_id": self.session_id,
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

        // SIGTERM on unix for graceful shutdown; fall back to start_kill().
        #[cfg(unix)]
        {
            if let Some(pid) = self.child.id() {
                // SAFETY: passing a valid PID; libc::kill is safe to call.
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
            } else {
                let _ = self.child.start_kill();
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.start_kill();
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

/// Format an optional stderr tail for inclusion in error messages.
fn fmt_stderr_tail(tail: &str) -> String {
    if tail.is_empty() {
        String::new()
    } else {
        format!(" (stderr: {})", tail.trim())
    }
}

/// Background task: drain the child's stderr into a rolling 4 KiB buffer.
async fn drain_stderr(
    stderr: tokio::process::ChildStderr,
    buf: Arc<AsyncMutex<String>>,
) {
    let mut reader = BufReader::new(stderr);
    let mut chunk = vec![0u8; 1024];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                let s = String::from_utf8_lossy(&chunk[..n]).to_string();
                let mut guard = buf.lock().await;
                guard.push_str(&s);
                let len = guard.len();
                if len > STDERR_BUFFER_BYTES {
                    // Keep only the last STDERR_BUFFER_BYTES.
                    let drop_to = len - STDERR_BUFFER_BYTES;
                    // Find a UTF-8 char boundary at or after drop_to.
                    let mut idx = drop_to;
                    while idx < len && !guard.is_char_boundary(idx) {
                        idx += 1;
                    }
                    *guard = guard[idx..].to_string();
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_mode_to_cli_mapping() {
        // Legacy aliases preserved.
        assert_eq!(permission_mode_to_cli(PermissionMode::AcceptEdits), "acceptEdits");
        assert_eq!(
            permission_mode_to_cli(PermissionMode::BypassPermissions),
            "bypassPermissions"
        );
        assert_eq!(permission_mode_to_cli(PermissionMode::RejectAll), "plan");
        assert_eq!(permission_mode_to_cli(PermissionMode::Prompt), "default");
        // New variants.
        assert_eq!(permission_mode_to_cli(PermissionMode::DontAsk), "dontAsk");
        assert_eq!(permission_mode_to_cli(PermissionMode::Auto), "default");
    }

    #[test]
    fn dont_ask_does_not_collide_with_plan() {
        // Regression guard: DontAsk must NOT map to "plan".
        let mapped = permission_mode_to_cli(PermissionMode::DontAsk);
        assert_ne!(mapped, "plan");
        assert_eq!(mapped, "dontAsk");
    }

    #[test]
    fn fmt_stderr_tail_empty_is_empty() {
        assert_eq!(fmt_stderr_tail(""), "");
    }

    #[test]
    fn fmt_stderr_tail_includes_content() {
        let out = fmt_stderr_tail("boom\n");
        assert!(out.contains("boom"));
        assert!(out.contains("stderr:"));
    }
}

/// Serialize a [`HookConfig`] into Claude Code's `settings.json` `hooks` shape.
pub(crate) fn hook_config_to_settings_json(cfg: &HookConfig) -> serde_json::Value {
    fn handler_to_entry(h: &nucel_agent_core::HookHandler) -> serde_json::Value {
        let mut hook = serde_json::json!({
            "type": "command",
            "command": h.command,
        });
        if let Some(t) = h.timeout_seconds {
            hook["timeout"] = serde_json::json!(t);
        }
        let mut entry = serde_json::json!({ "hooks": [hook] });
        if let Some(m) = &h.matcher {
            entry["matcher"] = serde_json::json!(m);
        }
        entry
    }
    let mut hooks = serde_json::Map::new();
    if let Some(h) = &cfg.pre_tool_use {
        hooks.insert("PreToolUse".into(), serde_json::json!([handler_to_entry(h)]));
    }
    if let Some(h) = &cfg.post_tool_use {
        hooks.insert("PostToolUse".into(), serde_json::json!([handler_to_entry(h)]));
    }
    if let Some(h) = &cfg.on_stop {
        hooks.insert("Stop".into(), serde_json::json!([handler_to_entry(h)]));
    }
    if let Some(h) = &cfg.user_prompt_submit {
        hooks.insert("UserPromptSubmit".into(), serde_json::json!([handler_to_entry(h)]));
    }
    serde_json::json!({ "hooks": hooks })
}

