//! Codex provider — wraps OpenAI's `codex` CLI.
//!
//! Based on the official [Codex CLI reference](https://developers.openai.com/codex/cli/reference/).
//!
//! - CLI: `codex exec --json "<prompt>"`
//! - Protocol: JSONL with event types
//!   `thread.started → turn.started → item.completed → turn.completed`
//! - Sandbox modes: `read-only`, `workspace-write`, `danger-full-access`
//! - Approval: `--ask-for-approval <policy>` (`--full-auto` is deprecated upstream).
//!
//! Each `query()` re-invokes `codex exec` rather than keeping a long-lived
//! subprocess: multi-turn is achieved by passing the previous `session_id`
//! to `resume()`.
//!
//! # Minimal example
//!
//! ```rust,no_run
//! use nucel_agent_codex::CodexExecutor;
//! use nucel_agent_core::{AgentExecutor, SpawnConfig};
//! use std::path::Path;
//!
//! # async fn run() -> nucel_agent_core::Result<()> {
//! let executor = CodexExecutor::new();
//! let session = executor.spawn(
//!     Path::new("/my/repo"),
//!     "Refactor the cost calculation in src/cost.rs",
//!     &SpawnConfig { model: Some("gpt-5-codex".into()), ..Default::default() },
//! ).await?;
//!
//! // Save the session id and resume later:
//! let sid = session.session_id.clone();
//! session.close().await?;
//!
//! let resumed = executor.resume(
//!     Path::new("/my/repo"), &sid, "Now add tests.", &SpawnConfig::default(),
//! ).await?;
//! println!("{}", resumed.query("Did the tests pass?").await?.content);
//! # Ok(()) }
//! ```
//!
//! See also: [workspace README](https://github.com/nucel-dev/agent-sdk#readme)
//! and the runnable example `crates/unified/examples/codex_resume.rs`.

#![cfg_attr(docsrs, feature(doc_cfg))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use nucel_agent_core::{
    AgentCapabilities, AgentCost, AgentError, AgentExecutor, AgentResponse, AgentSession,
    AvailabilityStatus, EventStream, ExecutorType, MessageEvent, PermissionMode, Result,
    SessionImpl, SpawnConfig,
};

/// Default timeout for Codex queries (10 minutes).
const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Maximum bytes of stderr to keep in the rolling buffer for error context.
const STDERR_BUFFER_BYTES: usize = 4096;

/// Codex executor — spawns `codex exec` CLI subprocess.
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
/// Official event types: thread.started, turn.started, item.completed, turn.completed, error
pub(crate) fn parse_codex_line(line: &str) -> Result<Option<CodexEvent>> {
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|e| AgentError::Provider {
            provider: "codex".into(),
            message: format!("JSON parse error: {e}"),
        })?;

    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        "thread.started" => {
            let thread_id = v
                .get("thread_id")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            Ok(Some(CodexEvent::ThreadStarted { thread_id }))
        }
        "turn.started" => Ok(Some(CodexEvent::TurnStarted)),
        "item.completed" => {
            let item = &v["item"];
            let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match item_type {
                "agent_message" => {
                    let text = item
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    Ok(Some(CodexEvent::Message(text)))
                }
                "reasoning" | "command_execution" | "file_change" | "mcp_tool_call" => {
                    tracing::debug!(item_type = %item_type, "codex item completed");
                    Ok(Some(CodexEvent::Other))
                }
                _ => Ok(Some(CodexEvent::Other)),
            }
        }
        "turn.completed" => {
            // Canonical key per Codex source is `usage`. Fall back to legacy
            // `token_usage` if the CLI version is older.
            let usage = v
                .get("usage")
                .or_else(|| v.get("token_usage"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
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
        "turn.failed" => {
            let error_msg = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string();
            Ok(Some(CodexEvent::Error(error_msg)))
        }
        "error" => {
            let error_msg = v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string();
            Ok(Some(CodexEvent::Error(error_msg)))
        }
        _ => Ok(Some(CodexEvent::Other)),
    }
}

#[derive(Debug)]
pub(crate) enum CodexEvent {
    ThreadStarted { thread_id: String },
    TurnStarted,
    Message(String),
    TurnCompleted {
        input_tokens: u64,
        output_tokens: u64,
    },
    Error(String),
    Other,
}

/// Map our PermissionMode to Codex sandbox/approval flags.
pub(crate) fn permission_to_codex_args(cmd: &mut Command, mode: Option<PermissionMode>) {
    match mode {
        Some(PermissionMode::BypassPermissions) => {
            cmd.arg("--dangerously-bypass-approvals-and-sandbox");
        }
        Some(PermissionMode::AcceptEdits) => {
            // `--full-auto` is deprecated upstream (prints a warning).
            // Equivalent: workspace-write sandbox.
            cmd.arg("--sandbox").arg("workspace-write");
        }
        Some(PermissionMode::RejectAll) => {
            cmd.arg("--sandbox").arg("read-only");
        }
        Some(PermissionMode::DontAsk) => {
            // Treat "don't ask, deny" as read-only sandbox.
            cmd.arg("--sandbox").arg("read-only");
        }
        Some(PermissionMode::Auto) | Some(PermissionMode::Prompt) | None => {
            // Default: workspace-write sandbox.
            cmd.arg("--sandbox").arg("workspace-write");
        }
        // Forward-compat: unknown future variants fall back to safe default.
        Some(_) => {
            cmd.arg("--sandbox").arg("workspace-write");
        }
    }
}

/// Output of a single `codex exec` invocation.
struct CodexRunOutput {
    content: String,
    cost: AgentCost,
    thread_id: String,
}

/// Run a codex exec command and collect response.
async fn run_codex(
    working_dir: &Path,
    prompt: &str,
    config: &SpawnConfig,
    api_key: Option<&str>,
    resume_thread_id: Option<&str>,
) -> Result<CodexRunOutput> {
    let mut cmd = Command::new("codex");
    cmd.current_dir(working_dir);
    cmd.arg("exec");

    // Resume must be the subcommand-position arg per `codex exec resume`.
    if let Some(tid) = resume_thread_id {
        cmd.arg("resume").arg(tid);
    }

    cmd.arg("--json"); // Official flag for JSONL output.
    cmd.arg("--skip-git-repo-check");
    // Keep ANSI escapes out of stderr (helps stderr capture stay readable).
    cmd.arg("--color").arg("never");

    // Model.
    if let Some(model) = &config.model {
        cmd.arg("--model").arg(model);
    }

    // Sandbox/approval mode.
    permission_to_codex_args(&mut cmd, config.permission_mode);

    // Working directory override.
    cmd.arg("--cd").arg(working_dir);

    // The prompt.
    cmd.arg(prompt);

    // Environment — OPENAI_API_KEY is the official env var for codex exec.
    if let Some(key) = api_key {
        cmd.env("OPENAI_API_KEY", key);
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

    // Drain stderr into a rolling buffer so we can include the tail in errors
    // (and so the child doesn't block on a full stderr pipe).
    let stderr_buf: Arc<AsyncMutex<String>> = Arc::new(AsyncMutex::new(String::new()));
    if let Some(err) = child.stderr.take() {
        let buf = stderr_buf.clone();
        tokio::spawn(drain_stderr(err, buf));
    }

    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut content = String::new();
    let mut cost = AgentCost::default();
    let mut thread_id = String::new();
    let mut had_error = false;
    let mut error_msg = String::new();

    let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);

    let read_loop = async {
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
                Ok(Some(CodexEvent::ThreadStarted { thread_id: tid })) => {
                    thread_id = tid;
                    tracing::debug!(thread_id = %thread_id, "codex thread started");
                }
                Ok(Some(CodexEvent::TurnStarted)) => {
                    tracing::debug!("codex turn started");
                }
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
                Ok(Some(CodexEvent::Error(msg))) => {
                    had_error = true;
                    error_msg = msg;
                }
                Ok(Some(CodexEvent::Other)) => {}
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse codex line");
                }
            }
        }
        Ok::<(), AgentError>(())
    };

    let result = tokio::time::timeout(timeout, read_loop).await;

    match result {
        Ok(Ok(())) => {
            // Process finished normally — wait for clean exit.
            let _ = child.wait().await;
        }
        Ok(Err(e)) => {
            let _ = child.kill().await;
            return Err(e);
        }
        Err(_) => {
            // Kill the child so we don't hang on its wait().
            let _ = child.kill().await;
            let _ = child.wait().await;
            let tail = stderr_buf.lock().await.clone();
            return Err(AgentError::Provider {
                provider: "codex".into(),
                message: format!(
                    "timed out after {}s{}",
                    timeout.as_secs(),
                    fmt_stderr_tail(&tail)
                ),
            });
        }
    }

    if had_error {
        let tail = stderr_buf.lock().await.clone();
        return Err(AgentError::Provider {
            provider: "codex".into(),
            message: format!("codex error: {error_msg}{}", fmt_stderr_tail(&tail)),
        });
    }

    Ok(CodexRunOutput {
        content,
        cost,
        thread_id,
    })
}

/// Format a stderr tail for inclusion in error messages.
fn fmt_stderr_tail(tail: &str) -> String {
    if tail.is_empty() {
        String::new()
    } else {
        format!(" (stderr: {})", tail.trim())
    }
}

/// Background task: drain the child's stderr into a rolling buffer.
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
                    let drop_to = len - STDERR_BUFFER_BYTES;
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

/// Internal session implementation for Codex.
struct CodexSessionImpl {
    cost: Arc<Mutex<AgentCost>>,
    budget: f64,
    working_dir: PathBuf,
    config: SpawnConfig,
    api_key: Option<String>,
    /// Codex thread id — used as the resume key for follow-up queries.
    thread_id: Arc<Mutex<String>>,
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

        let resume_id = {
            let g = self.thread_id.lock().unwrap();
            if g.is_empty() {
                None
            } else {
                Some(g.clone())
            }
        };

        let out = run_codex(
            &self.working_dir,
            prompt,
            &self.config,
            self.api_key.as_deref(),
            resume_id.as_deref(),
        )
        .await?;

        // Persist updated thread id if the server picked a new one.
        if !out.thread_id.is_empty() {
            let mut g = self.thread_id.lock().unwrap();
            *g = out.thread_id;
        }

        {
            let mut c = self.cost.lock().unwrap();
            c.input_tokens += out.cost.input_tokens;
            c.output_tokens += out.cost.output_tokens;
            c.total_usd += out.cost.total_usd;
        }

        Ok(AgentResponse {
            content: out.content,
            cost: out.cost,
            ..Default::default()
        })
    }

    async fn query_stream(&self, prompt: &str) -> Result<EventStream> {
        {
            let c = self.cost.lock().unwrap();
            if c.total_usd >= self.budget {
                return Err(AgentError::BudgetExceeded {
                    limit: self.budget,
                    spent: c.total_usd,
                });
            }
        }
        let resume_id = {
            let g = self.thread_id.lock().unwrap();
            if g.is_empty() { None } else { Some(g.clone()) }
        };
        let working_dir = self.working_dir.clone();
        let config = self.config.clone();
        let api_key = self.api_key.clone();
        let cost_handle = self.cost.clone();
        let thread_handle = self.thread_id.clone();
        let budget = self.budget;
        let prompt_owned = prompt.to_string();

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<MessageEvent>>(64);

        tokio::spawn(async move {
            stream_codex(
                &working_dir,
                &prompt_owned,
                &config,
                api_key.as_deref(),
                resume_id.as_deref(),
                budget,
                cost_handle,
                thread_handle,
                tx,
            )
            .await;
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Box::pin(stream))
    }

    async fn total_cost(&self) -> Result<AgentCost> {
        Ok(self.cost.lock().unwrap().clone())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// Run codex exec and emit MessageEvents as JSONL lines arrive.
#[allow(clippy::too_many_arguments)]
async fn stream_codex(
    working_dir: &Path,
    prompt: &str,
    config: &SpawnConfig,
    api_key: Option<&str>,
    resume_thread_id: Option<&str>,
    budget: f64,
    cost_handle: Arc<Mutex<AgentCost>>,
    thread_handle: Arc<Mutex<String>>,
    tx: tokio::sync::mpsc::Sender<Result<MessageEvent>>,
) {
    let mut cmd = Command::new("codex");
    cmd.current_dir(working_dir);
    cmd.arg("exec");
    if let Some(tid) = resume_thread_id {
        cmd.arg("resume").arg(tid);
    }
    cmd.arg("--json");
    cmd.arg("--skip-git-repo-check");
    cmd.arg("--color").arg("never");
    if let Some(model) = &config.model {
        cmd.arg("--model").arg(model);
    }
    permission_to_codex_args(&mut cmd, config.permission_mode);
    cmd.arg("--cd").arg(working_dir);
    cmd.arg(prompt);
    if let Some(key) = api_key {
        cmd.env("OPENAI_API_KEY", key);
    }
    for (k, v) in &config.env {
        cmd.env(k, v);
    }

    let mut child = match cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let err = if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::CliNotFound { cli_name: "codex".into() }
            } else {
                AgentError::Io(e)
            };
            let _ = tx.send(Err(err)).await;
            return;
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = tx
                .send(Err(AgentError::Provider {
                    provider: "codex".into(),
                    message: "failed to capture stdout".into(),
                }))
                .await;
            return;
        }
    };

    // Drain stderr
    let stderr_buf: Arc<AsyncMutex<String>> = Arc::new(AsyncMutex::new(String::new()));
    if let Some(err) = child.stderr.take() {
        let buf = stderr_buf.clone();
        tokio::spawn(drain_stderr(err, buf));
    }

    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut input_tokens = 0_u64;
    let mut output_tokens = 0_u64;
    let mut content = String::new();
    let mut thread_id_local = String::new();
    let mut had_error: Option<String> = None;

    let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);

    let result = tokio::time::timeout(timeout, async {
        loop {
            line.clear();
            let n = match reader.read_line(&mut line).await {
                Ok(n) => n,
                Err(e) => {
                    let _ = tx.send(Err(AgentError::Io(e))).await;
                    return;
                }
            };
            if n == 0 { break; }
            let trimmed = line.trim();
            if trimmed.is_empty() { continue; }
            match parse_codex_line(trimmed) {
                Ok(Some(CodexEvent::ThreadStarted { thread_id })) => {
                    thread_id_local = thread_id;
                }
                Ok(Some(CodexEvent::TurnStarted)) => {}
                Ok(Some(CodexEvent::Message(text))) => {
                    if !content.is_empty() { content.push('\n'); }
                    content.push_str(&text);
                    let _ = tx.send(Ok(MessageEvent::TextChunk { text })).await;
                }
                Ok(Some(CodexEvent::TurnCompleted { input_tokens: i, output_tokens: o })) => {
                    input_tokens = i;
                    output_tokens = o;
                }
                Ok(Some(CodexEvent::Error(msg))) => {
                    had_error = Some(msg);
                }
                Ok(Some(CodexEvent::Other)) | Ok(None) => {}
                Err(_) => {}
            }
        }
    }).await;

    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        let tail = stderr_buf.lock().await.clone();
        let _ = tx
            .send(Err(AgentError::Provider {
                provider: "codex".into(),
                message: format!("stream timed out after {}s{}", timeout.as_secs(), fmt_stderr_tail(&tail)),
            }))
            .await;
        return;
    } else {
        let _ = child.wait().await;
    }

    if let Some(msg) = had_error {
        let tail = stderr_buf.lock().await.clone();
        let _ = tx
            .send(Err(AgentError::Provider {
                provider: "codex".into(),
                message: format!("codex error: {msg}{}", fmt_stderr_tail(&tail)),
            }))
            .await;
        return;
    }

    let cost = AgentCost {
        input_tokens,
        output_tokens,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        total_usd: 0.0,
    };
    {
        let mut c = cost_handle.lock().unwrap();
        c.input_tokens += cost.input_tokens;
        c.output_tokens += cost.output_tokens;
        c.total_usd += cost.total_usd;
    }
    if !thread_id_local.is_empty() {
        let mut g = thread_handle.lock().unwrap();
        *g = thread_id_local;
    }
    if cost.total_usd > budget {
        let _ = tx.send(Err(AgentError::BudgetExceeded { limit: budget, spent: cost.total_usd })).await;
        return;
    }
    let _ = tx
        .send(Ok(MessageEvent::ResultDone { cost, content, is_error: false }))
        .await;
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
        let cost = Arc::new(Mutex::new(AgentCost::default()));
        let budget = config.budget_usd.unwrap_or(f64::MAX);

        if budget <= 0.0 {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: 0.0,
            });
        }

        let out = run_codex(working_dir, prompt, config, self.api_key.as_deref(), None).await?;

        if out.cost.total_usd > budget {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: out.cost.total_usd,
            });
        }

        // Use the upstream thread_id as session_id when available so the
        // caller can resume; fall back to a fresh UUID only if codex did not
        // emit a thread.started event.
        let session_id = if out.thread_id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            out.thread_id.clone()
        };

        {
            let mut c = cost.lock().unwrap();
            *c = out.cost.clone();
        }

        let inner = Arc::new(CodexSessionImpl {
            cost: cost.clone(),
            budget,
            working_dir: working_dir.to_path_buf(),
            config: config.clone(),
            api_key: self.api_key.clone(),
            thread_id: Arc::new(Mutex::new(out.thread_id.clone())),
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
        let cost = Arc::new(Mutex::new(AgentCost::default()));
        let budget = config.budget_usd.unwrap_or(f64::MAX);

        if budget <= 0.0 {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: 0.0,
            });
        }

        // `codex exec resume <thread_id> --cd <wd> <prompt>`
        let out = run_codex(
            working_dir,
            prompt,
            config,
            self.api_key.as_deref(),
            Some(session_id),
        )
        .await?;

        if out.cost.total_usd > budget {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: out.cost.total_usd,
            });
        }

        // Resume keeps the same logical session id (the original thread).
        let resolved_thread_id = if out.thread_id.is_empty() {
            session_id.to_string()
        } else {
            out.thread_id.clone()
        };

        {
            let mut c = cost.lock().unwrap();
            *c = out.cost.clone();
        }

        let inner = Arc::new(CodexSessionImpl {
            cost: cost.clone(),
            budget,
            working_dir: working_dir.to_path_buf(),
            config: config.clone(),
            api_key: self.api_key.clone(),
            thread_id: Arc::new(Mutex::new(resolved_thread_id.clone())),
        });

        Ok(AgentSession::new(
            resolved_thread_id,
            ExecutorType::Codex,
            working_dir.to_path_buf(),
            config.model.clone(),
            inner,
        ))
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            // Resume is now implemented via `codex exec resume <thread_id>`.
            session_resume: true,
            token_usage: true,
            mcp_support: false,
            autonomous_mode: true,
            // Structured output via --output-schema is not yet wired; flip
            // back to true once that lands.
            structured_output: false,
            streaming: true,
            hooks: false,
            prompt_caching: false,
            extended_thinking: false,
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

// Touch `Child` to keep the import used in builds without resume usage.
#[allow(dead_code)]
fn _child_type_check(c: &Child) -> Option<u32> {
    c.id()
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
    fn capabilities_after_resume_landing() {
        let caps = CodexExecutor::new().capabilities();
        assert!(caps.autonomous_mode);
        assert!(caps.token_usage);
        assert!(!caps.mcp_support);
        assert!(caps.session_resume, "Codex resume implemented via `codex exec resume`");
        assert!(!caps.structured_output, "structured output not yet wired");
    }

    #[test]
    fn parse_codex_thread_started() {
        let line =
            r#"{"type":"thread.started","thread_id":"019ce6ce-65fd-7530-8e6b-9ccce0436091"}"#;
        let event = parse_codex_line(line).unwrap();
        match event {
            Some(CodexEvent::ThreadStarted { thread_id }) => {
                assert_eq!(thread_id, "019ce6ce-65fd-7530-8e6b-9ccce0436091");
            }
            _ => panic!("expected ThreadStarted"),
        }
    }

    #[test]
    fn parse_codex_turn_started() {
        let line = r#"{"type":"turn.started"}"#;
        let event = parse_codex_line(line).unwrap();
        assert!(matches!(event, Some(CodexEvent::TurnStarted)));
    }

    #[test]
    fn parse_codex_message_event() {
        let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Fixed the bug"}}"#;
        let event = parse_codex_line(line).unwrap();
        match event {
            Some(CodexEvent::Message(text)) => assert_eq!(text, "Fixed the bug"),
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn parse_codex_turn_completed_canonical_usage_key() {
        // Canonical key per Codex source is `usage` (not `token_usage`).
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

    #[test]
    fn parse_codex_turn_completed_legacy_token_usage_fallback() {
        let line = r#"{"type":"turn.completed","token_usage":{"input_tokens":7,"output_tokens":11}}"#;
        let event = parse_codex_line(line).unwrap();
        match event {
            Some(CodexEvent::TurnCompleted {
                input_tokens,
                output_tokens,
            }) => {
                assert_eq!(input_tokens, 7);
                assert_eq!(output_tokens, 11);
            }
            _ => panic!("expected TurnCompleted"),
        }
    }

    #[test]
    fn parse_codex_turn_completed_prefers_usage_over_token_usage() {
        // If both are present, the canonical `usage` key wins.
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":2},"token_usage":{"input_tokens":99,"output_tokens":99}}"#;
        let event = parse_codex_line(line).unwrap();
        match event {
            Some(CodexEvent::TurnCompleted {
                input_tokens,
                output_tokens,
            }) => {
                assert_eq!(input_tokens, 1);
                assert_eq!(output_tokens, 2);
            }
            _ => panic!("expected TurnCompleted"),
        }
    }

    #[test]
    fn parse_codex_error() {
        let line = r#"{"type":"error","message":"Quota exceeded"}"#;
        let event = parse_codex_line(line).unwrap();
        match event {
            Some(CodexEvent::Error(msg)) => assert!(msg.contains("Quota")),
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn parse_codex_turn_failed() {
        let line = r#"{"type":"turn.failed","error":{"message":"Quota exceeded. Check your plan."}}"#;
        let event = parse_codex_line(line).unwrap();
        match event {
            Some(CodexEvent::Error(msg)) => assert!(msg.contains("Quota")),
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn parse_unknown_type_returns_other() {
        let line = r#"{"type":"web_search","query":"test"}"#;
        let event = parse_codex_line(line).unwrap();
        assert!(matches!(event, Some(CodexEvent::Other)));
    }

    #[test]
    fn permission_accept_edits_no_longer_uses_full_auto() {
        // Regression: --full-auto is deprecated upstream; we must use --sandbox workspace-write.
        let mut cmd = Command::new("codex");
        permission_to_codex_args(&mut cmd, Some(PermissionMode::AcceptEdits));
        let dbg = format!("{:?}", cmd.as_std());
        assert!(
            !dbg.contains("--full-auto"),
            "should not pass --full-auto: {dbg}"
        );
        assert!(
            dbg.contains("--sandbox") && dbg.contains("workspace-write"),
            "should pass --sandbox workspace-write: {dbg}"
        );
    }

    #[test]
    fn permission_dont_ask_maps_to_read_only_sandbox() {
        let mut cmd = Command::new("codex");
        permission_to_codex_args(&mut cmd, Some(PermissionMode::DontAsk));
        let dbg = format!("{:?}", cmd.as_std());
        assert!(dbg.contains("--sandbox") && dbg.contains("read-only"), "{dbg}");
    }

    #[test]
    fn permission_bypass_uses_dangerous_flag() {
        let mut cmd = Command::new("codex");
        permission_to_codex_args(&mut cmd, Some(PermissionMode::BypassPermissions));
        let dbg = format!("{:?}", cmd.as_std());
        assert!(
            dbg.contains("--dangerously-bypass-approvals-and-sandbox"),
            "{dbg}"
        );
    }
}
