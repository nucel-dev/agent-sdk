//! OpenCode HTTP client.

use nucel_agent_core::{
    AgentCost, AgentError, AgentResponse, MessageEvent, Result, RetryPolicy, SpawnConfig,
};
use serde_json::json;
use tokio::sync::mpsc::Sender;

/// Default username for OpenCode HTTP basic auth when only a password (api_key)
/// is supplied. Matches upstream defaults.
const DEFAULT_BASIC_AUTH_USERNAME: &str = "opencode";

/// Optional sink for streaming-side observability events (`ApiRetry`). The
/// non-streaming `create_session`/`prompt` paths pass `None`; the SSE path
/// passes the live channel so a retry is observable in `query_stream()`.
type RetrySink<'a> = Option<&'a Sender<Result<MessageEvent>>>;

/// HTTP client for OpenCode server.
///
/// One `OpencodeClient` is meant to live for the duration of an executor +
/// session so the underlying `reqwest::Client` can pool HTTP connections.
#[derive(Clone)]
pub struct OpencodeClient {
    http: reqwest::Client,
    base_url: String,
    api_user: Option<String>,
    api_password: Option<String>,
    directory: Option<String>,
    /// Retry policy for *transient*, pre-side-effect request failures
    /// (connection errors, timeouts, `429`/`502`/`503`/`504` before any
    /// response body is consumed). Defaults to [`RetryPolicy::default`].
    retry: RetryPolicy,
}

impl OpencodeClient {
    /// Build a new client.
    ///
    /// - `api_key`: HTTP basic-auth password (paired with
    ///   `OPENCODE_SERVER_USERNAME` env var or [`DEFAULT_BASIC_AUTH_USERNAME`]).
    ///   Falls back to `OPENCODE_SERVER_PASSWORD` env var when `None`.
    /// - `directory`: scopes server-side file ops; sent as `?directory=<path>`
    ///   query string (v2 contract) AND as the legacy
    ///   `x-opencode-directory` header for back-compat.
    pub fn new(base_url: &str, api_key: Option<&str>, directory: Option<&str>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();

        // Legacy directory header (back-compat with pre-v2 servers).
        if let Some(dir) = directory {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(dir) {
                headers.insert("x-opencode-directory", val);
            }
        }

        let mut builder = reqwest::Client::builder();
        if !headers.is_empty() {
            builder = builder.default_headers(headers);
        }

        // Resolve credentials:
        //   explicit api_key  → use as password
        //   else OPENCODE_SERVER_PASSWORD env var
        let password = api_key
            .map(String::from)
            .or_else(|| std::env::var("OPENCODE_SERVER_PASSWORD").ok());
        let username = std::env::var("OPENCODE_SERVER_USERNAME").ok();

        let (api_user, api_password) = match password {
            Some(pw) => (
                Some(username.unwrap_or_else(|| DEFAULT_BASIC_AUTH_USERNAME.to_string())),
                Some(pw),
            ),
            None => (None, None),
        };

        Self {
            http: builder.build().expect("failed to build reqwest client"),
            base_url: base_url.to_string(),
            api_user,
            api_password,
            directory: directory.map(String::from),
            retry: RetryPolicy::default(),
        }
    }

    /// Override the retry policy for transient, pre-side-effect failures.
    ///
    /// Pass [`RetryPolicy::none`] to disable retrying. Retries only ever apply
    /// to the request-dispatch phase (connection failure, timeout, `429`/`502`/
    /// `503`/`504` *before* any response body is consumed); once a `2xx` body
    /// starts being read, errors are always fatal regardless of this policy.
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Apply credentials and the optional `?directory=…` query.
    fn apply_common(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let (Some(user), Some(pw)) = (self.api_user.as_deref(), self.api_password.as_deref()) {
            req = req.basic_auth(user, Some(pw));
        }
        if let Some(dir) = &self.directory {
            req = req.query(&[("directory", dir.as_str())]);
        }
        req
    }

    /// Dispatch a request with bounded, pre-side-effect retry and decode the
    /// `2xx` JSON body.
    ///
    /// `op` is a short label used in error messages / retry logs. `build_req`
    /// must construct a *fresh* [`reqwest::RequestBuilder`] on every call so
    /// each attempt re-mints the request and nothing is replayed half-sent.
    ///
    /// Classification mirrors the Vertex provider:
    /// - send/connect/timeout failures → transient (no body consumed),
    /// - `429`/`502`/`503`/`504` → transient (server rejected before doing
    ///   work),
    /// - any other non-2xx → fatal,
    /// - `2xx` then JSON-decode failure → fatal (we are past the side-effect
    ///   boundary; never replay).
    async fn send_with_retry<F>(
        &self,
        op: &str,
        retry_sink: RetrySink<'_>,
        build_req: F,
    ) -> Result<serde_json::Value>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        let policy = self.retry;
        let mut retries_done: u32 = 0;
        loop {
            let send_result = build_req().send().await;

            let attempt: Result<serde_json::Value> = match send_result {
                Err(e) => {
                    // No response body consumed → classify by reqwest flags.
                    if e.is_timeout() {
                        Err(AgentError::Timeout { seconds: 300 })
                    } else if e.is_connect() || e.is_request() {
                        Err(AgentError::Io(std::io::Error::new(
                            std::io::ErrorKind::ConnectionReset,
                            format!("HTTP error contacting OpenCode ({op}): {e}"),
                        )))
                    } else {
                        Err(AgentError::Provider {
                            provider: "opencode".into(),
                            message: format!("HTTP error contacting OpenCode ({op}): {e}"),
                        })
                    }
                }
                Ok(response) => {
                    let status = response.status();
                    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        let body = response.text().await.unwrap_or_default();
                        Err(AgentError::RateLimited { message: body })
                    } else if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
                        || status == reqwest::StatusCode::GATEWAY_TIMEOUT
                        || status == reqwest::StatusCode::BAD_GATEWAY
                    {
                        let body = response.text().await.unwrap_or_default();
                        Err(AgentError::RateLimited {
                            message: format!("{status}: {body}"),
                        })
                    } else if !status.is_success() {
                        // 4xx (other than 429) / hard 5xx → fatal.
                        let body = response.text().await.unwrap_or_default();
                        Err(AgentError::Provider {
                            provider: "opencode".into(),
                            message: format!("{op} failed ({status}): {body}"),
                        })
                    } else {
                        // 2xx — PAST the side-effect boundary. A decode failure
                        // here is fatal: do not replay.
                        response
                            .json::<serde_json::Value>()
                            .await
                            .map_err(|e| AgentError::Provider {
                                provider: "opencode".into(),
                                message: format!("failed to parse {op} response: {e}"),
                            })
                    }
                }
            };

            match attempt {
                Ok(body) => return Ok(body),
                Err(err) => {
                    if policy.should_retry(&err, retries_done) {
                        let backoff = policy.backoff_for(retries_done);
                        let attempt_no = retries_done + 1;
                        tracing::warn!(
                            op = op,
                            attempt = attempt_no,
                            backoff_ms = backoff.as_millis() as u64,
                            "opencode transient failure; retrying"
                        );
                        // Make the retry observable on the stream, if any.
                        if let Some(tx) = retry_sink {
                            let _ = tx
                                .send(Ok(MessageEvent::ApiRetry {
                                    attempt: attempt_no,
                                    message: format!("transient {op} failure: {err}"),
                                }))
                                .await;
                        }
                        tokio::time::sleep(backoff).await;
                        retries_done += 1;
                        continue;
                    }
                    return Err(err);
                }
            }
        }
    }

    /// Create a new session on the OpenCode server.
    pub async fn create_session(&self) -> Result<serde_json::Value> {
        self.create_session_inner(None).await
    }

    /// Create a session, forwarding `ApiRetry` events to a streaming sink when
    /// a transient pre-side-effect failure is retried.
    pub(crate) async fn create_session_inner(
        &self,
        retry_sink: RetrySink<'_>,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/session", self.base_url);

        // ── Dispatch with bounded retry ──────────────────────────────────
        //
        // SIDE-EFFECT RULE: only the pre-side-effect window is retried —
        // connection establishment and an outright server rejection
        // (`429`/`502`/`503`/`504`) where *no response body has been consumed*.
        // The instant we begin reading a `2xx` body, errors are fatal and
        // never retried. Session creation is idempotent enough to replay only
        // while it has demonstrably not been accepted.
        self.send_with_retry("create session", retry_sink, || {
            self.apply_common(self.http.post(&url)).json(&json!({}))
        })
        .await
    }

    /// Send a prompt to a session.
    pub async fn prompt(
        &self,
        session_id: &str,
        prompt: &str,
        config: &SpawnConfig,
        budget: f64,
    ) -> Result<AgentResponse> {
        self.prompt_inner(session_id, prompt, config, budget, None)
            .await
    }

    /// Prompt a session, forwarding `ApiRetry` events to a streaming sink when
    /// a transient pre-side-effect failure is retried.
    pub(crate) async fn prompt_inner(
        &self,
        session_id: &str,
        prompt: &str,
        config: &SpawnConfig,
        budget: f64,
        retry_sink: RetrySink<'_>,
    ) -> Result<AgentResponse> {
        let mut body = json!({
            "parts": [
                {
                    "type": "text",
                    "text": prompt,
                }
            ],
        });

        // v2 model contract: { providerID, modelID } — split on "/".
        if let Some(model) = &config.model {
            body["model"] = build_model_body(model);
        }

        // Add system prompt if specified.
        if let Some(system) = &config.system_prompt {
            body["system"] = json!(system);
        }

        let url = format!("{}/session/{}/prompt", self.base_url, session_id);

        // ── Dispatch with bounded retry ──────────────────────────────────
        //
        // SIDE-EFFECT RULE: we retry ONLY the pre-side-effect window — request
        // dispatch and an outright server rejection (`429`/`502`/`503`/`504`)
        // where *no response body has been consumed* and the model therefore
        // did no work. The instant we begin reading a `2xx` body (tokens
        // generated, cost incurred), errors are fatal and never retried. A
        // retry rebuilds the request from `body` each attempt so nothing is
        // duplicated. The user-turn transcript push lives in the caller,
        // OUTSIDE this loop.
        let data: serde_json::Value = self
            .send_with_retry("prompt", retry_sink, || {
                self.apply_common(self.http.post(&url)).json(&body)
            })
            .await?;

        // Extract response text from parts.
        let mut content = String::new();
        if let Some(parts) = data.get("parts").and_then(|p| p.as_array()) {
            for part in parts {
                if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if !content.is_empty() {
                            content.push('\n');
                        }
                        content.push_str(text);
                    }
                }
            }
        }

        // If no parts, try the direct text field.
        if content.is_empty() {
            if let Some(text) = data.get("text").and_then(|t| t.as_str()) {
                content = text.to_string();
            }
        }

        // Extract cost.
        let cost_usd = data
            .get("cost")
            .and_then(|c| c.as_f64())
            .unwrap_or(0.0);

        // Token usage — prefer the new `info.tokens` shape, fall back to the
        // legacy top-level `tokens` shape.
        let (input_tokens, output_tokens) = parse_tokens(&data);

        if cost_usd > budget {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: cost_usd,
            });
        }

        Ok(AgentResponse {
            content,
            cost: AgentCost {
                input_tokens,
                output_tokens,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                total_usd: cost_usd,
            },
            confidence: None,
            requests_escalation: false,
            tool_calls: vec![],
        })
    }



    /// Open an SSE stream against `GET /event` and emit `MessageEvent`s.
    ///
    /// The OpenCode server's `/event` endpoint emits JSON events for the
    /// active session(s). We translate the subset we recognize and forward
    /// the rest as no-ops.
    ///
    /// The caller is responsible for sending the prompt via [`Self::prompt`]
    /// in parallel — `/event` is read-only.
    pub async fn stream_events(
        &self,
        session_id: String,
        prompt: String,
        config: SpawnConfig,
        budget: f64,
        cost_handle: std::sync::Arc<std::sync::Mutex<AgentCost>>,
    ) -> Result<nucel_agent_core::EventStream>
    {
        use futures::StreamExt;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<MessageEvent>>(64);
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let api_user = self.api_user.clone();
        let api_password = self.api_password.clone();
        let directory = self.directory.clone();
        let client_clone = self.clone();

        tokio::spawn(async move {
            // Open SSE stream first.
            let url = format!("{}/event", base_url);
            let mut req = http.get(&url);
            if let (Some(u), Some(pw)) = (api_user.as_deref(), api_password.as_deref()) {
                req = req.basic_auth(u, Some(pw));
            }
            if let Some(d) = &directory {
                req = req.query(&[("directory", d.as_str())]);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(AgentError::Provider {
                        provider: "opencode".into(),
                        message: format!("failed to open SSE stream: {e}"),
                    })).await;
                    return;
                }
            };
            if !resp.status().is_success() {
                let _ = tx.send(Err(AgentError::Provider {
                    provider: "opencode".into(),
                    message: format!("SSE stream rejected: {}", resp.status()),
                })).await;
                return;
            }

            // Fire the prompt request in the background; the response carries
            // the final cost/tokens which we use to emit ResultDone. A retry
            // sink is threaded in so transient retries surface as `ApiRetry`
            // events on this same stream.
            let prompt_tx = tx.clone();
            let retry_tx = tx.clone();
            let session_for_prompt = session_id.clone();
            let prompt_owned = prompt.clone();
            let config_for_prompt = config.clone();
            let prompt_handle = tokio::spawn(async move {
                client_clone
                    .prompt_inner(
                        &session_for_prompt,
                        &prompt_owned,
                        &config_for_prompt,
                        budget,
                        Some(&retry_tx),
                    )
                    .await
            });

            // Parse SSE event-stream from the response body.
            let mut bytes_stream = resp.bytes_stream();
            let mut buffer = String::new();
            let mut data_buf = String::new();
            'outer: while let Some(chunk_res) = bytes_stream.next().await {
                let chunk = match chunk_res {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(Err(AgentError::Provider {
                            provider: "opencode".into(),
                            message: format!("SSE read error: {e}"),
                        })).await;
                        break;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                // Process whole lines.
                while let Some(idx) = buffer.find('\n') {
                    let line = buffer[..idx].trim_end_matches('\r').to_string();
                    buffer.drain(..=idx);
                    if line.is_empty() {
                        // Dispatch event boundary.
                        if !data_buf.is_empty() {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data_buf) {
                                handle_sse_event(&v, &tx).await;
                            }
                            data_buf.clear();
                        }
                        // Check if prompt response arrived to terminate.
                        if prompt_handle.is_finished() {
                            break 'outer;
                        }
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        if !data_buf.is_empty() { data_buf.push('\n'); }
                        data_buf.push_str(rest.trim_start());
                    }
                    // Other prefixes (event:, id:, retry:) ignored.
                }
            }

            // Now finalize from the prompt response.
            let final_resp = prompt_handle.await;
            match final_resp {
                Ok(Ok(resp)) => {
                    // Fold this turn's cost into the session total before
                    // emitting ResultDone, so `total_cost()` and later budget
                    // guards account for streamed spend (parity with `query()`).
                    {
                        let mut c = cost_handle.lock().unwrap();
                        c.input_tokens += resp.cost.input_tokens;
                        c.output_tokens += resp.cost.output_tokens;
                        c.cache_read_tokens += resp.cost.cache_read_tokens;
                        c.cache_creation_tokens += resp.cost.cache_creation_tokens;
                        c.total_usd += resp.cost.total_usd;
                    }
                    let _ = prompt_tx.send(Ok(MessageEvent::ResultDone {
                        cost: resp.cost.clone(),
                        content: resp.content,
                        is_error: false,
                    })).await;
                }
                Ok(Err(e)) => {
                    let _ = prompt_tx.send(Err(e)).await;
                }
                Err(_join) => {
                    let _ = prompt_tx.send(Err(AgentError::Provider {
                        provider: "opencode".into(),
                        message: "prompt task panicked".into(),
                    })).await;
                }
            }
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Box::pin(stream))
    }
}

async fn handle_sse_event(v: &serde_json::Value, tx: &tokio::sync::mpsc::Sender<Result<nucel_agent_core::MessageEvent>>) {
    use nucel_agent_core::MessageEvent;
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let props = v.get("properties").unwrap_or(v);
    match kind {
        "message.part.updated" | "message.updated" => {
            if let Some(part) = props.get("part") {
                let pt = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match pt {
                    "text" => {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            let _ = tx.send(Ok(MessageEvent::TextChunk { text: text.to_string() })).await;
                        }
                    }
                    "tool" => {
                        let name = part.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let id = part.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                        let input = part.get("input").cloned().unwrap_or(serde_json::Value::Null);
                        let _ = tx.send(Ok(MessageEvent::ToolUse { id, name, input })).await;
                    }
                    "reasoning" => {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            let _ = tx.send(Ok(MessageEvent::Thinking { text: text.to_string() })).await;
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

impl OpencodeClient {
    /// Best-effort abort of an active session.
    pub async fn abort(&self, session_id: &str) -> Result<()> {
        let url = format!("{}/session/{}/abort", self.base_url, session_id);
        let req = self.apply_common(self.http.post(&url));
        match req.send().await {
            Ok(_) => Ok(()),
            Err(e) => {
                tracing::debug!(error = %e, session_id = %session_id, "opencode abort failed (best-effort)");
                Ok(())
            }
        }
    }
}

/// Split `provider/model` into `{ "providerID": …, "modelID": … }`.
/// If no `/`, omit `providerID` and let the server pick a default provider.
pub(crate) fn build_model_body(model: &str) -> serde_json::Value {
    match model.split_once('/') {
        Some((provider, model_id)) if !provider.is_empty() && !model_id.is_empty() => {
            json!({ "providerID": provider, "modelID": model_id })
        }
        _ => json!({ "modelID": model }),
    }
}

/// Parse tokens from either `info.tokens.{input,output}` (v2) or the
/// top-level `tokens.{input,output}` (legacy) shape.
fn parse_tokens(data: &serde_json::Value) -> (u64, u64) {
    let tokens = data
        .get("info")
        .and_then(|i| i.get("tokens"))
        .or_else(|| data.get("tokens"));

    match tokens {
        Some(t) => {
            let input = t
                .get("input")
                .or_else(|| t.get("input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output = t
                .get("output")
                .or_else(|| t.get("output_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            (input, output)
        }
        None => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_model_body_with_provider() {
        let b = build_model_body("anthropic/claude-sonnet-4");
        assert_eq!(b["providerID"], "anthropic");
        assert_eq!(b["modelID"], "claude-sonnet-4");
    }

    #[test]
    fn build_model_body_without_provider() {
        let b = build_model_body("claude-sonnet-4");
        assert_eq!(b["modelID"], "claude-sonnet-4");
        assert!(
            b.get("providerID").is_none(),
            "providerID must be omitted when model has no '/': {b:?}"
        );
    }

    #[test]
    fn build_model_body_empty_provider_segment_is_treated_as_no_provider() {
        // "/claude-sonnet-4" → no provider → no providerID.
        let b = build_model_body("/claude-sonnet-4");
        assert!(b.get("providerID").is_none(), "{b:?}");
    }

    #[test]
    fn parse_tokens_v2_info_shape() {
        let data = json!({
            "info": { "tokens": { "input": 12, "output": 34 } }
        });
        let (i, o) = parse_tokens(&data);
        assert_eq!(i, 12);
        assert_eq!(o, 34);
    }

    #[test]
    fn parse_tokens_legacy_top_level_shape() {
        let data = json!({
            "tokens": { "input": 7, "output": 9 }
        });
        let (i, o) = parse_tokens(&data);
        assert_eq!(i, 7);
        assert_eq!(o, 9);
    }

    #[test]
    fn parse_tokens_legacy_underscored_keys() {
        let data = json!({
            "tokens": { "input_tokens": 1, "output_tokens": 2 }
        });
        let (i, o) = parse_tokens(&data);
        assert_eq!(i, 1);
        assert_eq!(o, 2);
    }

    #[test]
    fn parse_tokens_missing_returns_zero() {
        let data = json!({});
        assert_eq!(parse_tokens(&data), (0, 0));
    }

    #[test]
    fn client_constructs_with_api_key_password() {
        let c = OpencodeClient::new("http://example.com", Some("secret"), None);
        assert_eq!(c.api_password.as_deref(), Some("secret"));
        // Default username when only password is provided.
        assert_eq!(c.api_user.as_deref(), Some("opencode"));
    }

    #[test]
    fn client_constructs_without_credentials() {
        // We don't want env from the test runner leaking into this; just
        // assert that passing None doesn't blow up.
        let c = OpencodeClient::new("http://example.com", None, None);
        let _ = c.base_url;
    }
}
